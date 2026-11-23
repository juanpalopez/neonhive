#!/usr/bin/env bash
# Re-record the README demo (docs/demo.gif) from docs/demo.tape.
# Usage: scripts/record-demo.sh
# Needs: vhs (https://github.com/charmbracelet/vhs, which brings ttyd) and ffmpeg.
#   brew install vhs
# Records only `neonhive --demo`, the synthetic swarm, never a real session.
# VHS writes lossless PNG frames; ffmpeg turns them into a 10 fps GIF with a
# 64-colour palette. From lossless frames, unchanged pixels stay transparent
# between GIF frames, which keeps the file near 3 MB.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

for tool in vhs ffmpeg; do
  if ! command -v "$tool" > /dev/null 2>&1; then
    echo "error: $tool is not on PATH. Install it with: brew install vhs (brings ttyd; ffmpeg is separate)" >&2
    exit 1
  fi
done

out=target/demo-recording
cargo build --release --quiet
rm -rf "$out/frames"
mkdir -p "$out"

# The tape runs `neonhive`; use the binary just built.
PATH="$PWD/target/release:$PATH" vhs docs/demo.tape

ffmpeg -v error -y -framerate 24 -i "$out/frames/frame-text-%05d.png" \
  -filter_complex "fps=10,split[a][b];[a]palettegen=max_colors=64:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" \
  -loop 0 docs/demo.gif

size=$(wc -c < docs/demo.gif | tr -d ' ')
echo "docs/demo.gif: ${size} bytes"
if [[ "$size" -gt 3500000 ]]; then
  echo "warning: docs/demo.gif is over 3.5 MB; shorten the tape or lower the frame rate" >&2
fi
