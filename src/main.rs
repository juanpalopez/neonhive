mod app;
mod claude;
mod demo;
mod event;
mod fx;
mod picker;
mod pricing;
mod report;
mod state;
mod svg;
mod ui;
mod util;

use app::{App, Mode, SPEEDS};
use clap::{Parser, Subcommand};
use claude::{ClaudeSource, SessionInfo};
use event::Source;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{self as term_event, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{DefaultTerminal, Terminal};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Watch Claude Code agents and subagents interact, live or as a replay.
#[derive(Parser)]
#[command(version, about, args_conflicts_with_subcommands = true)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Session transcript (.jsonl). Without it, a session list opens.
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
    /// Play an endless synthetic swarm. Needs no Claude Code sessions.
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
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if let Some(file) = &args.prices {
        pricing::load(file)?;
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
    let mut app = App::new(Box::new(ClaudeSource::open(path)), mode, &project, &title);
    app.speed_idx = SPEEDS
        .iter()
        .position(|s| *s >= args.speed)
        .unwrap_or(SPEEDS.len() - 1);
    app
}

/// Prints a session report to stdout. No UI, no animation.
fn report(session: Option<&Path>, json: bool) -> anyhow::Result<()> {
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
        Some(s) if !s.title.is_empty() => format!("{} // {}", s.project, s.title),
        Some(s) => s.project.clone(),
        None => path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let mut world = state::World::new(&title);
    for ev in ClaudeSource::open(&path).poll() {
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

fn demo() -> App {
    let source = Box::new(demo::DemoSource::new(7));
    App::new(source, Mode::Live, "demo", "demo // synthetic agent swarm")
}

fn run(term: &mut DefaultTerminal, args: &Args) -> anyhow::Result<()> {
    if args.demo {
        view(term, demo())?;
        return Ok(());
    }
    let sessions = claude::sessions();
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
            .ok_or_else(|| anyhow::anyhow!("no Claude Code sessions found"))?;
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
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(false)
            }
            KeyCode::Char('q') => return Ok(false),
            KeyCode::Esc if app.help => app.help = false,
            KeyCode::Esc | KeyCode::Backspace => return Ok(true),
            KeyCode::Char('?' | 'h') => app.help = !app.help,
            KeyCode::Up | KeyCode::Char('k') => app.select(-1),
            KeyCode::Down | KeyCode::Char('j') => app.select(1),
            KeyCode::PageUp => app.select(-10),
            KeyCode::PageDown => app.select(10),
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
    }
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
