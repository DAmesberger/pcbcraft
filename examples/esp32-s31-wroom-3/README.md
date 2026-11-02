# ESP32-S31-WROOM-3 (draft part)

The first library part made with the datasheet assistant ([docs/datasheet-assistant.md](../../docs/datasheet-assistant.md)). Unreviewed drafts: nothing here is published.

- `esp32-s31-wroom-3.pcbcraft`: the project of drafts (component with 99 pins, IO MUX/LP/analog functions and eight Rhai pin rules; symbol; the LGA module package with its antenna keep-out and generated 3D model; footprint; binding).
- `esp32-s31-wroom-3.review.md` / `.review.json`: the review report: every value with its datasheet page and table or figure, confidence and open questions.
- `marking.json`: the module's display-only marking, as `library.set_marking` takes it: the model name etched on the can (`can_top`, laser etch on metal), a placeholder logo box and a certification placeholder (original artwork, CC0-1.0; supply your own logo in your project, Pcbcraft ships no vendor artwork), a QR code of the instance's designator, and the designator in white silkscreen on the board beyond the can (`pcb_top`). The `.pcbcraft` file carries it on the component draft; after the replay regenerates the file, apply it again with `PCBCRAFT_WRITE_EXAMPLE=1 cargo test -p pcbcraft-automation write_the_example_marking -- --ignored`.
- `wroom3.replay.jsonl`: the tool calls, replayable without network:

```sh
pcbcraft-cli design-part --replay examples/esp32-s31-wroom-3/wroom3.replay.jsonl \
  --datasheet esp32-s31-wroom-3_wroom-3u_datasheet_en.pdf \
  --datasheet esp32-s31_datasheet_en.pdf \
  --datasheet esp-hardware-design-guidelines-en-master-esp32s31.pdf \
  --name ESP32-S31-WROOM-3 --out new.pcbcraft
```

- `run1/`: the first live Claude API run (`claude-opus-5-5`, $7.23): its recorded responses (`run1.recording.jsonl`, replayed by the `pcbcraft-assist` tests) and the project it produced. [docs/datasheet-assistant-run1.md](../../docs/datasheet-assistant-run1.md) compares it with the reference above; `pcbcraft-cli review-diff run1/esp32-s31-wroom-3.pcbcraft esp32-s31-wroom-3.pcbcraft run1 reference` lists the differences.

Provenance: the values come from Espressif's public documentation (ESP32-S31-WROOM-3 & -3U datasheet v0.7, ESP32-S31 series datasheet v0.5, ESP32-S31 hardware design guidelines v0.3, all pre-release), downloaded from documentation.espressif.com. No EDA library data was used. The replay was written from those datasheets by the agent that built the assistant, because no API key was available; it is not a recording of a Claude API run and its usage figures are zero. The PDFs are not redistributed here.
