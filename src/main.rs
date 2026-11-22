mod app;
mod claude;
mod codex;
mod demo;
mod event;
mod fx;
mod health;
mod picker;
mod pricing;
mod report;
mod state;
mod svg;
mod ui;
mod util;
mod watch;

use app::{App, Mode, Panel, SPEEDS};
use clap::{Parser, Subcommand};
use claude::{ClaudeSource, SessionInfo};
use event::Source;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    self as term_event, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::{DefaultTerminal, Terminal};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Watch Claude Code and Codex agents and subagents interact, live or as a replay.
#[derive(Parser)]
#[command(version, about, args_conflicts_with_subcommands = true)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Session transcript (.jsonl, or a Codex .jsonl.zst). Without it, a session list opens.
    session: Option<PathBuf>,
    /// Open the most recent session.
    #[arg(long)]
    latest: bool,
    /// Follow the session live, even if it looks finished.
    #[arg(long, conflicts_with = "replay")]
    live: bool,
    /// Replay the session from its start, even if it is still running.
    #[arg(long)]
    replay: bool,
    /// Replay speed multiplier: 1, 2, 4, 8, 16, 32, 64 or 128.
    #[arg(long, default_value_t = 8.0)]
    speed: f64,
    /// Play an endless synthetic swarm. Needs no sessions.
    #[arg(long, conflicts_with_all = ["session", "latest"])]
    demo: bool,
    /// Use the 256-color palette instead of 24-bit color.
    #[arg(long)]
    ansi256: bool,
    /// Print one frame as text after this many seconds of replay, then exit.
    #[arg(long, value_name = "SECONDS")]
    snapshot: Option<f64>,
    /// With --snapshot: write the frame to this SVG file instead of stdout.
    #[arg(long, value_name = "FILE", requires = "snapshot")]
    svg: Option<PathBuf>,
    /// With --svg: number of frames in a looping animated SVG.
    #[arg(long, default_value_t = 1, requires = "svg")]
    frames: usize,
    /// Frame size for --snapshot.
    #[arg(long, default_value = "140x40", value_name = "WxH")]
    size: String,
    /// JSON file of token prices (USD per million) that replace or extend the
    /// built-in table used for cost estimates.
    #[arg(long, value_name = "FILE", global = true)]
    prices: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Print a summary of a session as Markdown or JSON, without opening the UI.
    Report {
        /// Session transcript (.jsonl). Defaults to the most recent session.
        session: Option<PathBuf>,
        /// Print JSON instead of Markdown.
        #[arg(long)]
        json: bool,
    },
    /// Follow a session without a UI and print one line per alert: a failed
    /// agent, a looping, erroring or stalled one, a passed budget.
    Watch {
        /// Session transcript (.jsonl). Defaults to the most recent session.
        session: Option<PathBuf>,
        #[command(flatten)]
        opts: watch::Opts,
    },
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if let Some(file) = &args.prices {
        if let Err(e) = pricing::load(file) {
            if matches!(args.command, Some(Command::Watch { .. })) {
                eprintln!("error: {e:#}");
                std::process::exit(watch::EXIT_ERROR);
            }
            return Err(e);
        }
    }
    if let Some(Command::Watch { session, opts }) = &args.command {
        std::process::exit(watch_session(session.as_deref(), opts));
    }
    if let Some(Command::Report { session, json }) = &args.command {
        return report(session.as_deref(), *json);
    }
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    fx::set_truecolor(
        args.svg.is_some()
            || !args.ansi256 && (colorterm.contains("truecolor") || colorterm.contains("24bit")),
    );

    if let Some(secs) = args.snapshot {
        return snapshot(&args, secs);
    }
    let mut term = ratatui::init();
    let result = run(&mut term, &args);
    ratatui::restore();
    result
}

/// Sessions of every supported agent tool, most recent first.
fn all_sessions() -> Vec<SessionInfo> {
    let mut sessions = claude::sessions();
    sessions.extend(codex::sessions());
    sessions.sort_by_key(|s| std::cmp::Reverse(s.mtime_ms));
    sessions
}

/// The adapter for a session file, chosen by its name or first line.
fn source(path: &Path) -> Box<dyn Source> {
    if codex::is_rollout(path) {
        Box::new(codex::CodexSource::open(path))
    } else {
        Box::new(ClaudeSource::open(path))
    }
}

fn open(path: &Path, info: Option<&SessionInfo>, args: &Args) -> App {
    let recent = info.is_some_and(|s| util::now_ms() - s.mtime_ms < picker::LIVE_WINDOW_MS);
    let mode = if args.replay {
        Mode::Replay
    } else if args.live || recent {
        Mode::Live
    } else {
        Mode::Replay
    };
    let project = info
        .map(|s| s.project.clone())
        .unwrap_or_else(|| "session".into());
    let title = match info {
        Some(s) if !s.title.is_empty() => format!("{} // {}", s.project, s.title),
        Some(s) => s.project.clone(),
        None => path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let mut app = App::new(source(path), mode, &project, &title);
    app.speed_idx = SPEEDS
        .iter()
        .position(|s| *s >= args.speed)
        .unwrap_or(SPEEDS.len() - 1);
    app
}

/// Prints a session report to stdout. No UI, no animation.
fn report(session: Option<&Path>, json: bool) -> anyhow::Result<()> {
    let sessions = all_sessions();
    let (path, info) = match session {
        Some(p) => {
            anyhow::ensure!(p.is_file(), "no such session file: {}", p.display());
            let p = p.canonicalize()?;
            let info = sessions.iter().find(|s| s.path == p);
            (p, info)
        }
        None => {
            let first = sessions
                .first()
                .ok_or_else(|| anyhow::anyhow!("no Claude Code or Codex sessions found"))?;
            (first.path.clone(), Some(first))
        }
    };
    let title = match info {
        Some(s) if !s.title.is_empty() => format!("{} // {}", s.project, s.title),
        Some(s) => s.project.clone(),
        None => path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let mut world = state::World::new(&title);
    for ev in source(&path).poll() {
        world.apply(&ev, false);
    }
    let mut text = if json {
        report::json(&world)
    } else {
        report::markdown(&world)
    };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    // A reader that closes the pipe early (`| head`) is not an error.
    match std::io::Write::write_all(&mut std::io::stdout().lock(), text.as_bytes()) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(()),
    }
}

/// Runs `neonhive watch` and returns the process exit code.
fn watch_session(session: Option<&Path>, opts: &watch::Opts) -> i32 {
    match watch_loop(session, opts) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            watch::EXIT_ERROR
        }
    }
}

fn watch_loop(session: Option<&Path>, opts: &watch::Opts) -> anyhow::Result<i32> {
    use std::io::Write;
    let sessions = claude::sessions();
    let (path, info) = match session {
        Some(p) => {
            anyhow::ensure!(p.is_file(), "no such session file: {}", p.display());
            let p = p.canonicalize()?;
            let info = sessions.iter().find(|s| s.path == p);
            (p, info)
        }
        None => {
            let first = sessions
                .first()
                .ok_or_else(|| anyhow::anyhow!("no Claude Code sessions found"))?;
            (first.path.clone(), Some(first))
        }
    };
    let title = match info {
        Some(s) => s.project.clone(),
        None => path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let mut world = state::World::new(&title);
    let mut watcher = watch::Watcher::new(opts.max_tokens, opts.max_cost);
    let mut source = ClaudeSource::open(&path);
    let mut idle = watch::Idle::new(opts.idle_exit, util::now_ms());
    let mut fired = false;
    let mut out = std::io::stdout().lock();
    loop {
        let events = source.poll();
        let clock = (!opts.once).then(util::now_ms);
        let mut closed = false;
        for alert in watcher.poll(&mut world, &events, clock) {
            fired |= alert.kind.is_problem();
            // A reader that closes the pipe early (`| head`) ends the watch.
            let wrote = writeln!(out, "{}", alert.render(opts.json)).and_then(|()| out.flush());
            match wrote {
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => closed = true,
                Err(e) => return Err(e.into()),
                Ok(()) => {}
            }
        }
        if opts.once || closed {
            break;
        }
        // The first poll returns the history, which counts as activity.
        idle.note(events.len(), util::now_ms());
        if idle.expired(util::now_ms()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(opts.interval));
    }
    Ok(if fired {
        watch::EXIT_ALERTS
    } else {
        watch::EXIT_OK
    })
}

fn demo() -> App {
    let source = Box::new(demo::DemoSource::new(7));
    App::new(source, Mode::Live, "demo", "demo // synthetic agent swarm")
}

fn run(term: &mut DefaultTerminal, args: &Args) -> anyhow::Result<()> {
    if args.demo {
        view(term, demo())?;
        return Ok(());
    }
    let sessions = all_sessions();
    let find = |path: &Path| sessions.iter().find(|s| s.path == path);

    if let Some(path) = &args.session {
        anyhow::ensure!(path.is_file(), "no such session file: {}", path.display());
        let path = path.canonicalize()?;
        view(term, open(&path, find(&path), args))?;
        return Ok(());
    }
    if args.latest {
        let first = sessions
            .first()
            .ok_or_else(|| anyhow::anyhow!("no Claude Code or Codex sessions found"))?;
        view(term, open(&first.path, Some(first), args))?;
        return Ok(());
    }
    let mut at = 0;
    while let Some(i) = picker::pick(term, &sessions, at)? {
        at = i;
        if !view(term, open(&sessions[i].path, Some(&sessions[i]), args))? {
            break;
        }
    }
    Ok(())
}

/// Runs the viewer. Returns true to go back to the session list.
fn view(term: &mut DefaultTerminal, mut app: App) -> anyhow::Result<bool> {
    let mut last = Instant::now();
    loop {
        let dt = last.elapsed().as_secs_f64();
        last = Instant::now();
        app.tick(dt.min(0.25));
        term.draw(|f| ui::draw(f, &mut app))?;

        if !term_event::poll(Duration::from_millis(33))? {
            continue;
        }
        let Event::Key(key) = term_event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match handle_key(&mut app, key) {
            Flow::Stay => {}
            Flow::Back => return Ok(true),
            Flow::Quit => return Ok(false),
        }
    }
}

/// What the viewer does after a key.
#[derive(Debug, PartialEq)]
enum Flow {
    Stay,
    /// Back to the session list.
    Back,
    Quit,
}

/// Maps one key press to an app change. No terminal needed, so tests can
/// drive it. Arrows, `j`/`k` and page keys act on the focused panel: they
/// select agents in the graph and roster and scroll the stream and inspector.
fn handle_key(app: &mut App, key: KeyEvent) -> Flow {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('c') {
        return Flow::Quit;
    }
    if app.panes.input {
        // The query line takes every key, so typing `q` does not quit.
        match key.code {
            KeyCode::Esc => app.search_clear(),
            KeyCode::Enter => app.search_confirm(),
            KeyCode::Backspace => {
                app.panes.query.pop();
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                app.panes.query.push(c);
            }
            _ => {}
        }
        return Flow::Stay;
    }
    let focus = app.panes.focus;
    // Move the focused panel by `rows`: change the selection or scroll.
    let mv = |app: &mut App, rows: i64| match focus {
        Panel::Stream => app.scroll_stream(rows),
        Panel::Inspector => app.scroll_inspector(rows),
        Panel::Graph | Panel::Roster => app.select(rows.clamp(-10, 10) as i32),
    };
    let page = match focus {
        Panel::Stream => app.stream_page(),
        Panel::Inspector => app.panes.inspect_rows.saturating_sub(1).max(1) as i64,
        _ => 10,
    };
    match key.code {
        KeyCode::Char('q') => return Flow::Quit,
        // Esc unwinds one layer at a time: help, full size, search, then back.
        KeyCode::Esc | KeyCode::Backspace => {
            if app.help {
                app.help = false;
            } else if app.panes.maximised {
                app.panes.maximised = false;
            } else if !app.panes.query.is_empty() {
                app.search_clear();
            } else {
                return Flow::Back;
            }
        }
        KeyCode::Char('?' | 'h') => app.help = !app.help,
        KeyCode::Tab => app.cycle_focus(1),
        KeyCode::BackTab => app.cycle_focus(-1),
        KeyCode::Char('z') => app.panes.maximised = !app.panes.maximised,
        KeyCode::Char('[') => app.resize_stream(-1),
        KeyCode::Char(']') => app.resize_stream(1),
        KeyCode::Char('<') => app.resize_side(-2),
        KeyCode::Char('>') => app.resize_side(2),
        KeyCode::Up | KeyCode::Char('k') => mv(app, -1),
        KeyCode::Down | KeyCode::Char('j') => mv(app, 1),
        KeyCode::PageUp => mv(app, -page),
        KeyCode::PageDown => mv(app, page),
        // Ends of the stream, or of the inspector when that has focus.
        KeyCode::Home | KeyCode::Char('g') if focus == Panel::Inspector => {
            app.scroll_inspector(i64::MIN);
        }
        KeyCode::End | KeyCode::Char('G') if focus == Panel::Inspector => {
            app.scroll_inspector(i64::MAX);
        }
        KeyCode::Home | KeyCode::Char('g') => app.stream_top(),
        KeyCode::End | KeyCode::Char('G') => app.stream_tail(),
        KeyCode::Char('/') => app.search_open(),
        KeyCode::Char('n') => app.search_step(true),
        KeyCode::Char('N') => app.search_step(false),
        KeyCode::Char('&') => app.panes.filter = !app.panes.filter,
        KeyCode::Char('f') => app.follow = !app.follow,
        KeyCode::Char('c') => app.cost_view = !app.cost_view,
        KeyCode::Char(' ') => app.paused = !app.paused,
        KeyCode::Char('+' | '=') => {
            app.speed_idx = (app.speed_idx + 1).min(SPEEDS.len() - 1);
        }
        KeyCode::Char('-') => app.speed_idx = app.speed_idx.saturating_sub(1),
        KeyCode::Char('r') => app.restart(),
        KeyCode::Char('l') => app.jump_to_end(),
        _ => {}
    }
    Flow::Stay
}

/// Headless render of one frame, for tests, screenshots and bug reports.
fn snapshot(args: &Args, secs: f64) -> anyhow::Result<()> {
    let (w, h) = args
        .size
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .ok_or_else(|| anyhow::anyhow!("--size must look like 140x40"))?;
    let mut app = if args.demo {
        demo()
    } else {
        let path = args
            .session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("--snapshot needs a session file or --demo"))?;
        anyhow::ensure!(path.is_file(), "no such session file: {}", path.display());
        open(path, None, args)
    };
    let mut term = Terminal::new(TestBackend::new(w, h))?;
    term.draw(|f| ui::draw(f, &mut app))?; // sizes the graph before layout
    let frames = (secs * 30.0).ceil() as usize;
    for _ in 0..frames.max(1) {
        app.tick(1.0 / 30.0);
    }
    term.draw(|f| ui::draw(f, &mut app))?;
    if let Some(file) = &args.svg {
        // Extra frames are captured 0.2 s apart and loop at 5 fps.
        let mut frames = vec![term.backend().buffer().clone()];
        for _ in 1..args.frames.max(1) {
            for _ in 0..6 {
                app.tick(1.0 / 30.0);
            }
            term.draw(|f| ui::draw(f, &mut app))?;
            frames.push(term.backend().buffer().clone());
        }
        std::fs::write(file, svg::document(&frames, 5.0))?;
        return Ok(());
    }
    let buf = term.backend().buffer();
    for y in 0..h {
        let row: String = (0..w).map(|x| buf[(x, y)].symbol()).collect();
        println!("{}", row.trim_end());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_app() -> App {
        let mut app = App::new(
            Box::new(demo::DemoSource::new(3)),
            Mode::Live,
            "demo",
            "demo",
        );
        for _ in 0..900 {
            app.tick(1.0 / 30.0);
        }
        assert!(app.world.log.len() > 40, "demo should have logged lines");
        app
    }

    fn press(app: &mut App, code: KeyCode) -> Flow {
        handle_key(app, KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn typed(app: &mut App, s: &str) {
        for c in s.chars() {
            assert_eq!(press(app, KeyCode::Char(c)), Flow::Stay);
        }
    }

    /// Text of the bottom line the stream shows.
    fn bottom_text(app: &App) -> String {
        let list = app.stream_list();
        let b = app.stream_bottom(&list).unwrap();
        app.world.log[list[b]].text.clone()
    }

    #[test]
    fn tab_cycles_focus_and_skips_hidden_panels() {
        let mut app = demo_app();
        assert_eq!(app.panes.focus, Panel::Graph);
        let order = [Panel::Inspector, Panel::Roster, Panel::Stream, Panel::Graph];
        for want in order {
            press(&mut app, KeyCode::Tab);
            assert_eq!(app.panes.focus, want);
        }
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.panes.focus, Panel::Stream);
        // Narrow terminal: no side column, so only graph and stream.
        app.panes.side_shown = false;
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.panes.focus, Panel::Graph);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.panes.focus, Panel::Stream);
        // Full size can show any panel.
        app.panes.maximised = true;
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.panes.focus, Panel::Inspector);
    }

    #[test]
    fn query_line_takes_every_key() {
        let mut app = demo_app();
        assert_eq!(press(&mut app, KeyCode::Char('/')), Flow::Stay);
        assert!(app.panes.input);
        assert_eq!(app.panes.focus, Panel::Stream);
        let (sel, follow, speed) = (app.selected, app.follow, app.speed_idx);
        typed(&mut app, "q?jkfcrlz+-n&G[<");
        assert_eq!(app.panes.query, "q?jkfcrlz+-n&G[<");
        assert!(!app.help && !app.panes.maximised && !app.panes.filter);
        assert_eq!(
            (app.selected, app.follow, app.speed_idx),
            (sel, follow, speed)
        );
        assert_eq!(press(&mut app, KeyCode::Backspace), Flow::Stay);
        assert_eq!(app.panes.query, "q?jkfcrlz+-n&G[");
        // Esc cancels and clears; it does not leave the view.
        assert_eq!(press(&mut app, KeyCode::Esc), Flow::Stay);
        assert!(!app.panes.input && app.panes.query.is_empty());
        // Ctrl-C still quits from the query line.
        press(&mut app, KeyCode::Char('/'));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_key(&mut app, ctrl_c), Flow::Quit);
        assert_eq!(press(&mut app, KeyCode::Esc), Flow::Stay);
        assert_eq!(press(&mut app, KeyCode::Char('q')), Flow::Quit);
    }

    #[test]
    fn esc_unwinds_one_layer_at_a_time() {
        let mut app = demo_app();
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Char('/'));
        typed(&mut app, "read");
        // Esc on the open query line drops only the query.
        press(&mut app, KeyCode::Esc);
        assert!(!app.panes.input && app.panes.query.is_empty() && app.panes.maximised);
        press(&mut app, KeyCode::Char('/'));
        typed(&mut app, "read");
        press(&mut app, KeyCode::Enter);
        assert!(!app.panes.input);
        assert_eq!(app.panes.query, "read");
        app.help = true;
        let steps: [fn(&App) -> bool; 3] = [
            |a| !a.help && a.panes.maximised,
            |a| !a.panes.maximised && a.panes.query == "read",
            |a| a.panes.query.is_empty(),
        ];
        for done in steps {
            assert_eq!(press(&mut app, KeyCode::Esc), Flow::Stay);
            assert!(done(&app));
        }
        assert_eq!(press(&mut app, KeyCode::Esc), Flow::Back);
        assert_eq!(press(&mut app, KeyCode::Backspace), Flow::Back);
    }

    #[test]
    fn arrows_act_on_the_focused_panel() {
        let mut app = demo_app();
        app.follow = true;
        let sel = app.selected;
        press(&mut app, KeyCode::Up);
        assert_eq!(app.selected, sel.saturating_sub(1));
        assert!(!app.follow);
        // Stream: scrolls, selection stays.
        app.panes.focus = Panel::Stream;
        let sel = app.selected;
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.selected, sel);
        assert!(app.panes.anchor.is_some());
        // Inspector: scrolls within what the renderer measured.
        app.panes.focus = Panel::Inspector;
        app.panes.inspect_max = 5;
        app.panes.inspect_rows = 3;
        press(&mut app, KeyCode::Down);
        assert_eq!(app.panes.inspect_scroll, 1);
        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.panes.inspect_scroll, 3);
        press(&mut app, KeyCode::End);
        assert_eq!(app.panes.inspect_scroll, 5);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.panes.inspect_scroll, 5);
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.panes.inspect_scroll, 0);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.panes.inspect_scroll, 0);
        assert_eq!(app.selected, sel);
    }

    #[test]
    fn stream_scroll_is_clamped_at_both_ends() {
        let mut app = demo_app();
        app.panes.focus = Panel::Stream;
        let rows = app.panes.stream_rows;
        press(&mut app, KeyCode::Down);
        assert_eq!(app.panes.anchor, None, "already at the tail");
        press(&mut app, KeyCode::Char('g'));
        let list = app.stream_list();
        assert_eq!(
            app.stream_bottom(&list),
            Some(rows - 1),
            "top fills the panel"
        );
        for _ in 0..3 {
            press(&mut app, KeyCode::Up);
            press(&mut app, KeyCode::PageUp);
        }
        assert_eq!(app.stream_bottom(&list), Some(rows - 1));
        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.stream_bottom(&list), Some(rows - 1 + rows - 1));
        press(&mut app, KeyCode::End);
        assert_eq!(app.panes.anchor, None);
        // Scrolling down to the newest line follows again.
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        assert!(app.panes.anchor.is_some());
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.panes.anchor, None);
    }

    #[test]
    fn scrolled_back_stream_pauses_and_counts_new_lines() {
        let mut app = demo_app();
        app.panes.focus = Panel::Stream;
        press(&mut app, KeyCode::PageUp);
        let seen = bottom_text(&app);
        let anchor = app.panes.anchor;
        assert!(anchor.is_some());
        assert_eq!(app.new_lines(), 0);
        let before = app.world.log_seq;
        for _ in 0..300 {
            app.tick(1.0 / 30.0);
        }
        assert!(app.world.log_seq > before, "demo keeps logging");
        assert_eq!(app.panes.anchor, anchor, "paused view does not move");
        assert_eq!(bottom_text(&app), seen);
        assert_eq!(app.new_lines(), app.world.log_seq - before);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.panes.anchor, None);
        assert_eq!(app.new_lines(), 0);
    }

    #[test]
    fn restart_and_jump_to_end_drop_the_scroll_position() {
        let mut app = App::new(Box::new(demo::DemoSource::new(4)), Mode::Replay, "d", "d");
        for _ in 0..1800 {
            app.tick(1.0 / 30.0);
        }
        app.panes.focus = Panel::Stream;
        press(&mut app, KeyCode::Char('g'));
        assert!(app.panes.anchor.is_some());
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.panes.anchor, None);
        assert_eq!(app.new_lines(), 0);
        for _ in 0..600 {
            app.tick(1.0 / 30.0);
        }
        press(&mut app, KeyCode::PageUp);
        assert!(app.panes.anchor.is_some());
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.panes.anchor, None);
        // Still scrollable afterwards, with no stale offset.
        press(&mut app, KeyCode::Char('k'));
        let list = app.stream_list();
        assert_eq!(app.stream_bottom(&list), Some(list.len() - 2));
    }

    #[test]
    fn n_and_shift_n_walk_matches_and_wrap() {
        let mut app = demo_app();
        press(&mut app, KeyCode::Char('/'));
        typed(&mut app, "READ");
        press(&mut app, KeyCode::Enter);
        let matches = app.matches();
        assert!(matches.len() >= 2, "demo reads files");
        let no = |app: &App, k: usize| app.log_no(matches[k]);
        let last = matches.len() - 1;
        assert_eq!(
            app.panes.current,
            Some(no(&app, last)),
            "enter: newest match"
        );
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.panes.current, Some(no(&app, last - 1)));
        press(&mut app, KeyCode::Char('N'));
        press(&mut app, KeyCode::Char('N'));
        assert_eq!(app.panes.current, Some(no(&app, 0)), "wraps to the oldest");
        // The match is on screen.
        let list = app.stream_list();
        let b = app.stream_bottom(&list).unwrap();
        let pos = list.iter().position(|&i| i == matches[0]).unwrap();
        assert!(pos <= b && pos + app.panes.stream_rows > b);
        // Grep mode lists only the matches.
        press(&mut app, KeyCode::Char('&'));
        assert!(app.filtering());
        assert_eq!(app.stream_list(), matches);
        // No match: nothing current, and keys still do not panic.
        press(&mut app, KeyCode::Char('/'));
        typed(&mut app, "zzz no such text");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.panes.current, None);
        assert!(app.stream_list().is_empty());
        press(&mut app, KeyCode::Char('k'));
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('n'));
    }

    #[test]
    fn resize_keys_stay_within_bounds() {
        let mut app = demo_app();
        app.panes.term = (140, 40);
        for _ in 0..100 {
            press(&mut app, KeyCode::Char(']'));
            press(&mut app, KeyCode::Char('>'));
        }
        assert_eq!(app.panes.stream_h, 40 - 2 - app::BODY_MIN_H);
        assert_eq!(app.panes.side_w, 140 - app::GRAPH_MIN_W);
        for _ in 0..100 {
            press(&mut app, KeyCode::Char('['));
            press(&mut app, KeyCode::Char('<'));
        }
        assert_eq!(app.panes.stream_h, app::STREAM_MIN_H);
        assert_eq!(app.panes.side_w, app::SIDE_MIN_W);
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(app.panes.stream_h, app::STREAM_MIN_H + 1);
    }
}
