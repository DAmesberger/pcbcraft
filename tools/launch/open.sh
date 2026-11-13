#!/usr/bin/env bash
# Start the app on a project with the private launch state (for measuring shot coordinates),
# print its pid and window frame. Close it with: kill PID
#   tools/launch/open.sh PROJECT.sparksmith [STILL.png]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
bin="${SPARKSMITH_BIN:-$repo/target/nix/ci/sparksmith-desktop}"
[ -x "$bin" ] || bin="$repo/target/ci/sparksmith-desktop"
kit="$repo/target/launch/launchkit"
[ -x "$kit" ] || swiftc -O -o "$kit" "$here/launchkit.swift"
state="$repo/target/launch/state"
rm -rf "$state"; mkdir -p "$state/recovery" "$state/config" "$state/cache" "$state/home"
export SPARKSMITH_RECOVERY_DIR="$state/recovery" SPARKSMITH_CONFIG_DIR="$state/config" SPARKSMITH_CACHE_DIR="$state/cache"
HOME="$state/home" "$bin" "$1" > "$repo/target/launch/open.log" 2>&1 &
pid=$!
for _ in $(seq 1 100); do "$kit" window "$pid" > /dev/null 2>&1 && break; sleep 0.2; done
echo "pid $pid"
"$kit" window "$pid"
if [ $# -gt 1 ]; then
  sleep 5
  id="$("$kit" window "$pid" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
  screencapture -x -o -l "$id" "$2"
  echo "still $2"
fi
