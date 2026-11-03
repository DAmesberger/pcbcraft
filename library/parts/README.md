# Built-in part files

Parametric SolidCraft designs that packages use as their 3D body ([lean parts](../../docs/part-templates.md#11-lean-parts-contract)). Each follows Pcbcraft's naming conventions; `pcbcraft-cli part check FILE` checks them.

| File | Built-in name | What it is | Provenance | Licence |
|---|---|---|---|---|
| `esp32-s31-wroom-3.solidcraft` | `esp32-s31-wroom-3` | Shielded radio module with four pad rows, a ground array and corner pads: board, ENIG pads, nickel-silver can, printed antenna. Defaults are the ESP32-S31-WROOM-3's. | Original work, authored through SolidCraft's commands by `cargo test -p pcbcraft-solidcraft --test author_parts -- --ignored`. Dimensions from Espressif's public ESP32-S31-WROOM-3 datasheet v0.7 (see `examples/esp32-s31-wroom-3/`); no CAD or EDA library data was copied. | MIT OR Apache-2.0 |
