#!/usr/bin/env bash
# Check the post-processing chain without the app or any screen capture: a synthetic take
# (ffmpeg's test pattern) gets the cursor halo, a click ripple, a zoom and a caption.
#   tools/launch/selftest.sh [OUT_DIR]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="${1:-${TMPDIR:-/tmp}/sparksmith-launch-selftest}"
mkdir -p "$out"
export FFMPEG="${FFMPEG:-$(nix build nixpkgs#ffmpeg-full.bin --print-out-paths --no-link)/bin/ffmpeg}"

"$FFMPEG" -y -loglevel error -f lavfi -i testsrc2=s=1380x900:r=30:d=6 -c:v libx264 -pix_fmt yuv420p "$out/raw.mov"
echo '{"started": 1000.0}' > "$out/started.json"
echo '{"width": 690, "height": 450, "scale": 2}' > "$out/window.json"
cat > "$out/track.json" <<'EOF'
[{"t": 1000.1, "kind": "begin", "x": 690, "y": 450},
 {"t": 1000.2, "kind": "mark", "text": "in"},
 {"t": 1000.5, "kind": "move", "x": 100, "y": 100},
 {"t": 1001.5, "kind": "move", "x": 400, "y": 300},
 {"t": 1001.6, "kind": "down", "x": 400, "y": 300, "text": "left"},
 {"t": 1001.7, "kind": "up", "x": 400, "y": 300, "text": "left"},
 {"t": 1001.0, "kind": "caption", "text": "Shove: one click, one undo step", "duration": 3},
 {"t": 1002.0, "kind": "zoom", "x": 400, "y": 300, "duration": 2.5, "zoom": 2.0},
 {"t": 1005.5, "kind": "mark", "text": "out"},
 {"t": 1005.6, "kind": "end"}]
EOF
python3 "$here/post.py" take "$out" --width 1280
"$FFMPEG" -loglevel error -y -ss 3.2 -i "$out/final.mp4" -frames:v 1 "$out/frame.png"
python3 "$here/post.py" hero "$out/final.mp4" "$out/hero" --from 0.5 --to 3.5 --width 640
echo "selftest ok: $out/final.mp4, $out/frame.png, $out/hero.webp"
