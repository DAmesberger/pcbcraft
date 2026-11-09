# ESP32-S31-WROOM-3 (draft part)

The first library part made with the datasheet assistant ([docs/datasheet-assistant.md](../../docs/datasheet-assistant.md)). Unreviewed drafts: nothing here is published.

- `esp32-s31-wroom-3.pcbcraft`: the project of drafts (component with 99 pins, IO MUX/LP/analog functions and eight Rhai pin rules; symbol; the LGA module package with its antenna keep-out; footprint; binding; and the module body's part file as a Model draft).
- `esp32-s31-wroom-3.review.md` / `.review.json`: the review report: every value with its datasheet page and table or figure, confidence and open questions.
- `marking.json`: the module's display-only marking, as `library.set_marking` takes it: the model name etched on the can (`can_top`, laser etch on metal), a placeholder logo box and a certification placeholder (original artwork, CC0-1.0; supply your own logo in your project, Pcbcraft ships no vendor artwork), a QR code of the instance's designator, and the designator in white silkscreen on the board beyond the can (`pcb_top`). The `.pcbcraft` file carries it on the component draft; after the replay regenerates the file, apply it again with `PCBCRAFT_WRITE_EXAMPLE=1 cargo test -p pcbcraft-automation write_the_example_marking -- --ignored`.
- **3D body.** The package's model is the built-in part file `library/parts/esp32-s31-wroom-3.solidcraft` ([lean parts](../../docs/part-templates.md#11-lean-parts-contract)): an ordinary parametric SolidCraft design, built at the values the replay's step 06b gives from the drawing (`width`, `length`, `board_thickness`, `can_width`, `can_length`, `can_height`). Its marking surfaces (`can_top`, `pcb_top`, …) are the file's `marking-*` construction planes. It builds exactly what the module builder built before (110 bodies, bounding boxes and volumes equal). After the replay regenerates the file, the switch is applied with `PCBCRAFT_WRITE_EXAMPLE=1 cargo test -p pcbcraft-automation write_the_example_part_file -- --ignored`, before the marking.
- `wroom3.replay.jsonl`: the tool calls, replayable without network:

```sh
pcbcraft-cli design-part --replay examples/esp32-s31-wroom-3/wroom3.replay.jsonl \
  --datasheet esp32-s31-wroom-3_wroom-3u_datasheet_en.pdf \
  --datasheet esp32-s31_datasheet_en.pdf \
  --datasheet esp-hardware-design-guidelines-en-master-esp32s31.pdf \
  --name ESP32-S31-WROOM-3 --out new.pcbcraft
```

- `run1/`: the first live Claude API run (`claude-opus-5-5`): its recorded responses (`run1.recording.jsonl`, in the neutral recording format since 5 November 2026, replayed by the `pcbcraft-assist` tests) and the project it produced. Its pinout rows carry no source pages, so the review check (`library.review_check`, which the in-app runner's `submit_review` now runs) refuses its report and the replay ends refused. [docs/datasheet-assistant-run1.md](../../docs/datasheet-assistant-run1.md) compares it with the reference above; `pcbcraft-cli review-diff run1/esp32-s31-wroom-3.pcbcraft esp32-s31-wroom-3.pcbcraft run1 reference` lists the differences.

## CAD cross-check

Espressif publishes a footprint and a STEP model of the module for use in other tools. They were used like a datasheet, as a dimension source, and recorded as sources `ESPFP` and `ESPSTEP` in the review. They are not redistributed here: their licence is CC-BY-SA 4.0, and `cargo xtask clean-room` rejects EDA-format files.

| File | Source | Revision | Licence | SHA-256 |
|---|---|---|---|---|
| `ESP32-S31-WROOM-3.kicad_mod` | https://github.com/espressif/kicad-libraries/blob/cfb91e91cc6d6dfa96e0ac3264382b747229e8e7/footprints/Espressif.pretty/ESP32-S31-WROOM-3.kicad_mod | commit cfb91e9, 2026-07-24 | CC-BY-SA 4.0 (repository `LICENSE.md`) | `ac1bd57c…c7495` |
| `ESP32-S31-WROOM-3_V1_1.STEP` | https://github.com/espressif/kicad-libraries/blob/cfb91e91cc6d6dfa96e0ac3264382b747229e8e7/3dmodels/espressif.3dshapes/ESP32-S31-WROOM-3_V1_1.STEP | same commit; exported 2026-06-10 | CC-BY-SA 4.0 | `a0178d5b…ef81a5a` |

What they settle:

- **Edge pads.** 0.8 × 0.4 mm on every row, turned on the top and bottom rows, at 0.85 mm pitch. Confirmed.
- **Rows.** Pad 1 is at (−10.3, 7.2), 1.1 mm below corner pad 99, and pad 94 is 1.375 mm from that corner. The rows sit on the 22 × 24 mm pad body centred at y = −3, 0.3 mm inside the outline. Confirmed.
- **EPAD (pin 95).** Nine 1.8 mm pads turned 45°, at 2.35 mm pitch with 0.55 mm gaps, centred at (0, −3). Confirmed. The pad nearest the antenna has a 0.5 mm chamfer: the land pattern's two "1.3" dimensions are its shortened edges, the STEP cuts 0.5 mm and the footprint 0.45 mm. Pcbcraft draws this pad square.
- **Corner pads 96–99.** 0.8 mm. Confirmed.
- **Board.** 1.004 mm thick in the STEP, so the 1.0 mm board is confirmed; 3.538 mm overall. The can is 21 × 23 × 2.5 mm, centred at (0, −3).
- **Marks.** The only mark on the can is a Ø0.5 hole in the lid at (−9.5, −13.5), by pins 25 and 26. It is not a pin-1 mark, so the can gets none. Espressif's footprint has no pin-1 silkscreen mark; Pcbcraft's footprint adds its own at pad 1.
- **Antenna keep-out.** Confirmed: a keep-out over the 22 × 6 mm antenna area that excludes tracks, vias, pads, pours and parts.

The drafts were already right. Only the review changed: the confidences, the sources and the open questions, through the replay's `submit_review` call.

Provenance: the values come from Espressif's public documentation (ESP32-S31-WROOM-3 & -3U datasheet v0.7, ESP32-S31 series datasheet v0.5, ESP32-S31 hardware design guidelines v0.3, all pre-release), downloaded from documentation.espressif.com. Espressif's own published footprint and STEP model were used afterwards to check the dimensions ([above](#cad-cross-check)). No EDA tool's library data was used. The replay was written from those datasheets by the agent that built the assistant, because no API key was available; it is not a recording of a Claude API run and its usage figures are zero. The PDFs are not redistributed here.
