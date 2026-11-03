#!/usr/bin/env bash
# Record one storyboard shot of the Pcbcraft desktop app, window only, then post-process it.
#
#   tools/launch/take.sh SHOT PROJECT.pcbcraft OUT_DIR [--activate] [--settle SECONDS] [--prelude SHOT] [--no-post]
#
# SHOT is a name in tools/launch/shots/ (without .shot) or a path. The app is started fresh with
# its own recovery, settings and cache folders under target/launch/, so takes do not depend on the
# person's settings and never touch their recovery snapshots. Only the started app's window is
# recorded, and only that pid receives input. The app is closed by its pid at the end, also when
# the take fails.
#
# Environment: PCBCRAFT_BIN (default target/nix/ci/pcbcraft, then target/ci/pcbcraft),
# FFMPEG (default: ffmpeg-full from nixpkgs), LAUNCH_FONT (caption font).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
shot="$1"; project="$2"; out="$3"; shift 3
activate=""; settle=6; post=1; prelude=""
while [ $# -gt 0 ]; do
  case "$1" in
    --activate) activate="--activate" ;;
    --settle) settle="$2"; shift ;;
    --no-post) post=0 ;;
    --prelude) prelude="$2"; shift ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
  shift
done
[ -f "$shot" ] || shot="$here/shots/$shot.shot"
[ -f "$shot" ] || { echo "no shot script $shot" >&2; exit 2; }
mkdir -p "$out"
out="$(cd "$out" && pwd)"
project="$(cd "$(dirname "$project")" && pwd)/$(basename "$project")"

# Tools.
kit="$repo/target/launch/launchkit"
if [ ! -x "$kit" ] || [ "$here/launchkit.swift" -nt "$kit" ]; then
  mkdir -p "$repo/target/launch"
  swiftc -O -o "$kit" "$here/launchkit.swift"
fi
bin="${PCBCRAFT_BIN:-}"
for candidate in "$repo/target/nix/ci/pcbcraft" "$repo/target/ci/pcbcraft"; do
  [ -n "$bin" ] || { [ -x "$candidate" ] && bin="$candidate"; }
done
[ -x "$bin" ] || { echo "build the app first: cargo build --locked --profile ci -p pcbcraft" >&2; exit 2; }
export FFMPEG="${FFMPEG:-$(nix build nixpkgs#ffmpeg-full.bin --print-out-paths --no-link)/bin/ffmpeg}"

# A fresh, private app state for every take.
state="$repo/target/launch/state"
rm -rf "$state"; mkdir -p "$state/recovery" "$state/config" "$state/cache" "$state/home"
export PCBCRAFT_RECOVERY_DIR="$state/recovery" PCBCRAFT_CONFIG_DIR="$state/config" PCBCRAFT_CACHE_DIR="$state/cache"

app_pid=""; rec_pid=""
cleanup() {
  if [ -n "$rec_pid" ]; then touch "$out/stop"; wait "$rec_pid" 2>/dev/null || true; fi
  if [ -n "$app_pid" ] && kill -0 "$app_pid" 2>/dev/null; then
    kill -TERM "$app_pid" 2>/dev/null || true
    for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$app_pid" 2>/dev/null || break; sleep 0.3; done
    kill -KILL "$app_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT

HOME="$state/home" "$bin" "$project" > "$out/app.log" 2>&1 &
app_pid=$!
echo "$app_pid" > "$out/app.pid"
echo "app pid $app_pid" >&2
for _ in $(seq 1 100); do
  "$kit" window "$app_pid" > "$out/window.json" 2>/dev/null && break
  sleep 0.2
done
[ -s "$out/window.json" ] || { echo "the app opened no window" >&2; exit 1; }
if [ -n "$prelude" ]; then
  # Unrecorded setup (e.g. switch to 3D), then the settle time lets background builds finish.
  [ -f "$prelude" ] || prelude="$here/shots/$prelude.shot"
  sleep 2
  "$kit" play "$app_pid" "$prelude" "$out/prelude-track.json" $activate
fi
sleep "$settle"   # the model builds in the background; let the first frames settle
"$kit" window "$app_pid" > "$out/window.json"
cat "$out/window.json" >&2

rm -f "$out/started.json" "$out/raw.mov" "$out/stop"
"$kit" record "$app_pid" "$out/raw.mov" --fps 60 --started "$out/started.json" --stop "$out/stop" 2> "$out/record.log" &
rec_pid=$!
for _ in $(seq 1 50); do [ -s "$out/started.json" ] && break; sleep 0.1; done
[ -s "$out/started.json" ] || { cat "$out/record.log" >&2; echo "recording did not start (Screen Recording permission?)" >&2; exit 1; }
sleep 0.5
"$kit" play "$app_pid" "$shot" "$out/track.json" $activate
sleep 0.5
touch "$out/stop"; wait "$rec_pid" || true; rec_pid=""
cleanup; app_pid=""
trap - EXIT

if [ "$post" = 1 ]; then
  python3 "$here/post.py" take "$out"
fi
echo "take done: $out" >&2
