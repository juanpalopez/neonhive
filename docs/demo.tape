# VHS tape for the README recording (docs/demo.gif).
# Regenerate with scripts/record-demo.sh; see CONTRIBUTING.md.
# Syntax: https://github.com/charmbracelet/vhs (or `vhs manual`).
#
# Only `neonhive --demo` is recorded: a synthetic, seeded swarm, never a real
# session. The demo's event order is fixed, so the same keys land on the same
# moments every run (the header clock shows the time of recording).

Output target/demo-recording/frames/

Require neonhive

Set Shell "bash"
Set FontFamily "Menlo"
Set FontSize 20
Set Width 1530
Set Height 874
Set Padding 12
Set Framerate 24
Set TypingSpeed 120ms
# Background matches neonhive's own (fx::BG), so the padding blends in.
Set Theme { "name": "neonhive", "background": "#070810", "foreground": "#bcd0e4", "cursor": "#00f0ff", "selection": "#3a4c6c", "black": "#070810", "red": "#ff3850", "green": "#39ff8c", "yellow": "#fcee0a", "blue": "#00a0ff", "magenta": "#ff2abe", "cyan": "#00f0ff", "white": "#bcd0e4", "brightBlack": "#3a4c6c", "brightRed": "#ff3850", "brightGreen": "#39ff8c", "brightYellow": "#fcee0a", "brightBlue": "#00a0ff", "brightMagenta": "#ff2abe", "brightCyan": "#00f0ff", "brightWhite": "#f0faff" }

Hide
Type "clear; COLORTERM=truecolor neonhive --demo"
Enter
Sleep 300ms
Show

# 0-7 s: the main agent fans out, tools run, a subagent finishes.
Sleep 7s

# Select a subagent (top of the roster, then down to the second one): the
# inspector shows its task, model, tokens and cost. It is the demo's looping
# agent, so its loop flag appears a few seconds later.
PageUp
Sleep 400ms
Down 2
Sleep 2.5s

# Full-size the graph, then back.
Type "z"
Sleep 2.5s
Type "z"
Sleep 1s

# Search the event stream for the loop flag, then show only the matches.
Type "/loop"
Sleep 400ms
Enter
Sleep 1.2s
Type "&"
Sleep 2.5s
Escape
Sleep 1s
