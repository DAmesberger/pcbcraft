#!/bin/sh
# Rebuild the ESP32-S31 dev board from its replay and write every output.
# Run from the repository root:  sh examples/esp32-s31-devboard/regenerate.sh
#
# 1. The replay (570 JSON-lines requests through `pcbcraft-cli stdio
#    --operator`, the same command engine the desktop and MCP use) opens the
#    module example, authors the other parts, draws the schematic, lays out
#    and routes the board and saves esp32-s31-devboard.pcbcraft. Any refused
#    request fails.
# 2. `fab` writes the production files (byte-identical: --date none).
# 3. The schematic PDF and the native Solidcraft model are written beside it.
set -eu
dir=examples/esp32-s31-devboard
cli=${PCBCRAFT_CLI:-target/nix/ci/pcbcraft-cli}
[ -x "$cli" ] || cli=target/ci/pcbcraft-cli
[ -x "$cli" ] || { echo "build first: cargo build --locked --profile ci -p pcbcraft-cli" >&2; exit 1; }

replies=$(mktemp)
# --operator: your own replay, not a model's (relative paths, ungated).
"$cli" stdio --operator < "$dir/esp32-s31-devboard.replay.jsonl" > "$replies"
if grep -q '"ok":false' "$replies"; then
    grep -n '"ok":false' "$replies" | head -5 >&2
    echo "the replay was refused; nothing else written" >&2
    rm -f "$replies"
    exit 1
fi
echo "replay: $(wc -l < "$replies") requests accepted"
rm -f "$replies"

rm -rf "$dir/fab" "$dir/esp32-s31-devboard-schematic.pdf" "$dir/esp32-s31-devboard.solidcraft"
"$cli" fab "$dir/esp32-s31-devboard.pcbcraft" "$dir/fab" --date none > /dev/null
"$cli" export-schematic-pdf "$dir/esp32-s31-devboard.pcbcraft" "$dir/esp32-s31-devboard-schematic.pdf" > /dev/null
if [ "${NATIVE:-0}" = 1 ]; then
    # About 10 s of CPU (see docs/e2e-esp32-devboard.md, gap 3).
    "$cli" export "$dir/esp32-s31-devboard.pcbcraft" "$dir/esp32-s31-devboard.solidcraft" > /dev/null
fi
echo "wrote $dir/esp32-s31-devboard.pcbcraft, $dir/fab/ and the schematic PDF"
