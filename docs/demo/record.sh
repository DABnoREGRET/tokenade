#!/usr/bin/env bash
# Records docs/assets/demo.mp4 + demo.gif from demo.tape in a clean container.
# Needs Docker and a Claude Code login: the credentials file is mounted
# read-only as a copy and never appears on screen (IS_DEMO=1 hides the account).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${1:-$here/../assets}"
creds="${CLAUDE_CREDENTIALS:-$HOME/.claude/.credentials.json}"
mkdir -p "$out"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
cp "$creds" "$tmp/.credentials.json"
docker build -q --build-arg UID="$(id -u)" -t tokenade-demo "$here" >/dev/null
docker run --rm -v "$here/demo.tape:/demo.tape:ro" -v "$out:/out" \
  -v "$tmp/.credentials.json:/home/dev/.claude/.credentials.json" \
  --entrypoint vhs tokenade-demo /demo.tape
# Directory listings want a 1080p MP4 under 10 MB and a light GIF.
ffmpeg -y -loglevel error -i "$out/demo.mp4" -c:v libx264 -preset slow -crf 26 \
  -pix_fmt yuv420p -movflags +faststart -an "$out/demo.tmp.mp4"
mv "$out/demo.tmp.mp4" "$out/demo.mp4"
ffmpeg -y -loglevel error -i "$out/demo.mp4" \
  -vf "fps=8,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=64[p];[b][p]paletteuse=dither=none" \
  "$out/demo.gif"
ls -la "$out"/demo.*
