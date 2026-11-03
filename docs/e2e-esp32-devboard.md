# End-to-end run: ESP32-S31 dev board (release plan step 5)

On 3 November 2026 an agent designed an ESP32-S31-WROOM-3 development board, from components to Gerbers, through Pcbcraft's command engine. It worked as a CLI client would, sending JSON lines to `pcbcraft-cli stdio`. The base was `component-model` at 5e0922b, the build was the `ci` profile, and the machine was a macOS arm64 laptop. This note is the friction and gap log. The board is [examples/esp32-s31-devboard](../examples/esp32-s31-devboard/README.md).

The agent used the tools as a user would. It added no features and made no code fixes, because nothing stopped the run entirely; every workaround is listed below. Estimates are agent hours.

## Outcome

- **Board:**
  - 40 × 64 mm, 4 layers (signal / GND / power / signal), 1.74 mm.
  - 20 parts in 15 BOM lines, every line resolved to a manufacturer Part.
  - Two schematic sheets: 133 labels and power ports, 18 named nets.
  - 187 routed steps, 55 vias, 4 fills.
  - A 90 Ω USB pair routed with `layout.route_pair`.
  - Four board mounting holes and the module's antenna keep-out.
- **Checks:**
  - ERC is clean.
  - DRC and the production check are clean under the built-in *Standard 6/6* profile with edge-clearance overrides.
  - The remaining warnings: 43 starved thermals, 5 envelope-only 3D models, 1 legend over a pad opening, and the pair skew discussed in gap 2.
- **Outputs:** Gerber X2 (4 copper layers, 2 masks, paste, legend, profile), plated and non-plated Excellon, drill map and table, job file, top pick-and-place, both BOMs, fabrication and assembly PDFs, and the schematic PDF.
- **Reproducibility:** the replay (655 requests, 204 KB) rebuilds a byte-identical `project.json`. It took 20.7 s and 52 s on a heavily loaded machine (load average in the hundreds, shared with other agents); about half of the requests' time is serialising the 0.5 MB snapshot each mutation returns (gap 1).
- **Independent Gerber check:**
  - PyGerber 2.4.3 (MIT, not an EDA tool) parsed every Gerber layer and rendered it (`images/`).
  - Copper bounding boxes lie 0.305 mm inside the 40 × 64 board, as the 0.3 mm edge rule requires. The renders show the antenna keep-out empty on every layer, the rounded corners, the mounting-hole clearances and the buses.
  - A separate structural script parsed both Excellon files and matched every hit to the project: all 55 vias, all 34 footprint holes (28 header pins, 4 plated shell slots with length and width, 2 NPTH pegs) and the 4 mounting holes. It found none missing.
  - The copper's X2 `.N` net attributes carry the net names.
  - The legend's bounding box extends beyond the board (gap 7).
- **Datasheet assistant:**
  - One live run, on the TLV75733PDBVR: `claude-opus-5-5`, effort medium, 13 requests, **$1.06** of a $6 cap.
  - It produced the right pinout, read TI's DBV0005A land pattern (0.6 × 1.1 mm pads) and wrote an honest review with four open questions.
  - Three of its `library_package_plan` calls were refused before one succeeded.
  - Its footprint was better than the one the agent first made from the starter table, which the board then corrected to match.
  - The board's LDO was still authored through the same commands by hand, so the replay needs no network.

## Gaps by severity

| # | Stage | Severity | What happened | What a user would expect | Suggested fix | Estimate |
|---|---|---|---|---|---|---|
| 1 | Automation (MCP) | **blocker** for MCP clients | Every mutation reply carries the whole document snapshot: 0.5 MB per call on this board and 1.7 MB after `fill.refill`. MCP sends it twice, as text and as `structuredContent`. `tools/list` is 607 KB, of which `project_apply`'s schema is 319 KB and `project_query`'s 199 KB. An LLM MCP client would exhaust its context on the tool list, or after one placement. The run used `stdio` with its own client, which discarded the replies. | Compact replies by default: outcome, new and changed ids, revision, and findings for what changed. A tool list an agent can load. | Use the datasheet assistant's compact reply shape as the default for MCP and stdio, with `verbose: true` returning the snapshot. Split `project_apply` and `project_query` into per-editor tools, or serve operation schemas on demand. | 3–4 h replies, 4–6 h tool schemas |
| 2 | Diff pair / length | major | Pair skew and uncoupled length use the total copper of each net, branches included. A USB-C receptacle must join both plug orientations (A6–B6, A7–B7 through a via jumper), so the check reports a 4.735 mm skew. The receptacle-to-module paths are equal by hand: 23.575 mm each. Tuning to satisfy the check would create real skew, so the warning was left. | Length along the pad-to-pad path the pair connects, with stubs and branches reported separately. Pair endpoints settable. | Measure each half along the path between the pair's chosen endpoints, list branch stubs as their own lines in the length report, and exclude them from skew. | 4–6 h |
| 3 | 3D / native | minor (measure again) | `project.export_native` of the routed board took **9.8 s of CPU** (169 s wall: the machine's load average was 350–650, with other agents building). The shipped showcase took **4.6 s of CPU** (99 s wall) in the same conditions; it is documented at 2.3 s wall on an idle machine. So the board costs about twice the showcase, which suggests about 5 s wall when idle. The memory target is that the 3D view opens instantly. The wall times measured here are not representative. | A 3D view that opens in about a second for a board this size. | Measure on an idle machine with `build_timings`, then look at the module part file (110 bodies) and the inner fills around 55 vias. | 1 h to measure, then according to findings |
| 4 | Library | major | A part made in another project, such as the WROOM-3 drafts in their example file, cannot be brought into a board project except through a library folder. A library folder mints random lineage ids (`getrandom`), so a replay that places from a folder is not reproducible. The run therefore built the board *inside* the module's project file, which is why it is named "Untitled" (gap 15). | Import a part (with its closure) from another `.pcbcraft` file, and deterministic ids in replays. | Add `library.import_from_project {path, root}` as one undo step. Let the folder writer take caller-supplied ids, or have replays map recorded ids. | 4–6 h |
| 5 | Components | major | There is no package family for chip LEDs, SMD tact switches or USB-C receptacles, only pin headers among connectors. Workarounds: <br>• LED: a chip-capacitor row with the LED symbol, lands edited to the datasheet, an envelope model and an explicit binding (pad 1 cathode). <br>• Switch: a 1206 resistor row with the push-button symbol, then edited lands and graphics. <br>• USB4105: the whole footprint written as JSON (12 SMD pads, 4 plated oblong slots, 2 NPTH pegs). <br>The 3D falls back to envelope boxes, and the production check warns. | A connector family (USB-C 16-pin with shell slots), a chip LED form with polarity, an SMD switch form, and a "footprint from a pad table" command. | Add the forms with generated models. Add `library.footprint_from_pads`, which takes pad rows, slots and holes and adds courtyard and silkscreen. | 2–3 h for `footprint_from_pads`, 6–10 h per family |
| 6 | Rules | major | The default profile and *Standard 6/6* leave copper-to-board-edge clearance at 0, meaning unchecked. The first export put fill copper 5 µm from the edge, and the production check passed. Fixed here with overrides (0.3 mm for tracks, pads, vias and fills). | A built-in fabrication profile that is fabricable as shipped. | Give the built-in profiles a non-zero copper-to-edge clearance (0.25–0.3 mm), with the profile tests updated. | 1 h |
| 7 | Legend / DRC | major | With `silk_edge_clearance` set to 0.2 mm, the legend still extends to x −0.23 and y 65.47 on a 40 × 64 board, from J2's designator (rotated to the edge) and legend at the antenna end, and nothing reports it. No command moves or turns a designator per instance, so a header rotated to the board edge prints its designator off the board. | Legend outside the outline flagged, and designators movable per instance. | Check legend items against the outline in the production check. Add a per-instance text placement (`instance.set_text_placement`) honoured by 2D, the legend Gerber and 3D. | 2 h check, 3–4 h placement |
| 8 | Library | major | Published drafts stay open as drafts, and a project holds at most 64. A 12-part library hit "a project holds at most 64 drafts" before its Parts were drafted. The run discarded 77 drafts, one operation and one undo step each. | Publishing closes the drafts it published, or one command discards a set. | Add `close: true` (the default) to `library.publish`, or a batch `library.discard_drafts`. | 1–2 h |
| 9 | Mechanical | major | No SMD standoff or mechanical mounting part is available in this base; part-placement had not landed. Four board-level NPTH mounting holes (2.7 mm) were used. | Choose an SMD standoff (for example a Würth WA-SMSI) as a part. | The part-placement agent's area. | — |
| 10 | Purchasing | minor | `library.resolve_groups` gives a field-less component (LED, switch, connector, ICs) the query `kind:part`, which matches every Part. With the default search limit the right Part was not among the hits, and the first hit (a capacitor) was offered for the LEDs. `instance.assign_part` refused it atomically, which is good. | A group's query finds that group's Parts. | Add `implements:<component id/rev>` to the generated query. | 1 h |
| 11 | Library | minor | `component.check_drafts` reported a binding as publishable, but `library.publish` refused it ("Footprint r1 is missing from the project library"), because the template-created footprint draft was not in the set. The footprint id appears only in the `package_from_template` plan. | The check reports the missing closure, or publish includes the binding's drafts. | Run a closure check in `check_drafts`, or auto-include drafts the set references. | 1 h |
| 12 | Diff pair | minor | `layout.route_pair` ending on a 45° leg into 0.85 mm-pitch pads was refused with "Too close to a track on USB_DP: needs 0.2 mm", which names neither the place nor the cause (the pair's own fan-in). A short straight final leg fixed it. | Fan-in from any approach, or a message with a location and a suggestion. | Bend the fan-in to the pad axis, and add the location to the error. | 2 h |
| 13 | Rules / fills | minor | A net-class clearance also applies to pads and tracks. Setting 0.35 mm on the USB class (to keep the top pour off the pair) made the 0.5 mm-pitch receptacle pads unroutable. The top fill's own clearance (0.4 mm) was used instead, which pushes the pour back from everything on top. | A class's clearance to fills set separately. | Add `fill_clearance` to `NetClassSpec`. | 2 h |
| 14 | Routing | minor | A walkaround `layout.route_step` succeeds with any detour. VBUS took a loop round the receptacle shell to avoid a via, which was visible only in a render. | The CLI result says the path is much longer than the straight line, or the client can bound it. | Return `detour_factor` in the plan, and accept a `max_detour` in the request. | 2 h |
| 15 | Output | minor | The project name ("Untitled", from the module project) cannot be set by any command. The Gerber `.ProjectId`, the drill table and the PDF titles show it. `project.set_properties` sets the schematic title block only. | Name the project, or use the title-block title. | Add `project.rename` (or use the title in fabrication outputs). | 1 h |
| 16 | Output | minor | `pcbcraft-cli fab --profile std-6-6` printed only "6 unwaived errors". Finding them needed a `fabrication.preflight` query with the whole profile object, read from `rules.profiles`. | The CLI lists the blocking findings, and the query takes a profile id. | Print the error titles, and accept `profile: "std-6-6"` in the query. | 0.5–1 h |
| 17 | Fills | minor | 43 `fill.thermal_starved` warnings, mostly for vias under the module and in the EPAD squares, where reliefs cannot fit and are not wanted. | No noise for vias in pads, or for vias whose net already joins solidly. | Exempt vias inside same-net pads, and allow a solid via connection per fill. | 1–2 h |
| 18 | Fields | minor | A new project has no quantity kinds or fields. Resistance, capacitance and voltage were authored as six definitions, then each component's fields edited as JSON. | Standard SI fields available in one step. | Add a built-in "standard fields" bundle that `library.import` can take. | 2–3 h |
| 19 | Symbols | minor | A pinout import always draws an IC box, and no command restyles an existing symbol draft to an LED or push-button kind. That is why the LED and switch went through chip rows. | A pinout-imported 2-pin part can use a 2-terminal symbol. | Add `library.restyle_symbol {draft, kind}`. | 1–2 h |
| 20 | Templates | minor | Starter-table lands are generic estimates: SOT-23-5 pads 0.6 × 1.0 against TI's 0.6 × 1.1, and SOT-23-3 1.0 × 0.6 at a 2.2 mm row span against TI DBZ's 1.3 × 0.6 at 2.1. The provenance says "verify", but there is no land-pattern field to set from the datasheet; the pads were edited. | Enter the datasheet land pattern in the form. | Add land size and span overrides to the tabbed form. | 2 h |
| 21 | Board shape | minor | `board.edit` does not report created sketch ids. The ids of the mounting-hole centre points were read back by parsing a JSON string inside `board_shape.sketch.sketch`. | The outcome lists what the edit created. | Return the created `SketchRef`s in the outcome. | 1 h |
| 22 | Schematic | polish | Wiring by labels attached to every pin is quick through the API, but at the module's 2.5 mm pin pitch the labels and ports overlap and read poorly (sheet 2). | Readable label placement, or auto-orientation by pin direction. | Orient attached labels and ports away from the pin, and offset ports on dense rows. | 3 h |
| 23 | Automation | polish | The 4.4 MB `commands` dump is hard to navigate. Editor tags are PascalCase (`"editor": "Library"`) beside dotted lowercase command ids. Outcomes name ids differently (`id`, `class`, `pair`, `ids`, `sheet`), and class and pair id 0 is easy to mistake for "none". Stored values show float noise (10 µF as `9.999999999999999e-06`). | One small example per command, consistent outcome keys, ids from 1. | Generate examples into `commands`, and normalise outcome id keys. | 2–3 h |
| 24 | Build | polish | A cold `ci` build of `pcbcraft-cli` took 27 minutes, and the desktop app built for a further 40+ minutes in this worktree. The machine was shared with other agents' builds, so these times are inflated; a fresh worktree still builds everything from scratch. | A prebuilt CLI for agents, or a shared target cache across worktrees. | Shared `CARGO_TARGET_DIR` or sccache in the dev shell, and published CLI binaries. | 1–2 h |
| 25 | 3D | minor | An envelope 3D model is centred on the footprint origin. For a connector whose origin is the PCB edge (USB4105), half the box hung off the board, and nothing flagged it. | The envelope placed over the courtyard or pads, or a check that the body lies on the board. | Default the envelope to the courtyard centre, and warn when a body extends past the board outline. | 1–2 h |

## Choices the run made (adjust if they are wrong)

- **USB path.** USB-C goes to the module's USB Serial/JTAG pins (IO33/IO34, pins 42/43), which the ROM uses for download and console, not to the HS OTG pins (40/41). It is still routed and checked as a 90 Ω pair.
- **LDO and capacitors.**
  - The TLV75733PDBVR is rated 1 A against the module's ≥ 0.6 A (Table 6-2).
  - 10 µF on the input. TI requires ≥ 1 µF.
  - 22 µF and 100 nF on the output, from the module's peripheral schematic. TI allows 1–200 µF.
  - The 22 µF is placed at the LDO. The 100 nF is at the module pins.
- **ESD.** TI TPD2E009DBZR (SOT-23-3), placed between the receptacle and the pair. ST's USBLC6-2SC6 datasheet could not be downloaded.
- **Headers.**
  - J2: VBUS, 3V3, GND and IO12…IO4.
  - J3: GND and IO42…IO52.
  - J4: UART0 (GND, TX0, RX0, 3V3).
  - The GPIO order follows the module pin order, so the bottom-layer buses need no crossings.
- **Boot.** BOOT pulls IO61 low; IO60 stays on its internal pull-up (joint download, Table 4-3). RESET pulls EN low, with 10 kΩ and 1 µF on EN as the datasheet recommends.
- **Mounting.** Four NPTH M2.5 holes (gap 9). The module is at the top edge with the antenna over the board edge. The keep-out is empty on all layers. Espressif's 15 mm host clearance guideline is not met beyond the module outline, which is a common trade-off on dev boards.

## What worked well

- **One engine end to end.** 655 requests covered parts, schematic, board, rules, routing, fills and fabrication, all through `project.apply`/`project.query`. Every refused request left the document unchanged, so the client could try again with other values; every refusal in the first passes was recoverable. The replay rebuilds the board byte-identically.
- **Parts from datasheets.**
  - `library.pinout_preview`/`pinout_apply` followed by `package_from_template` made the LDO and ESD parts in a few calls, with their terminals kept by number.
  - The datasheet assistant did the same for $1.06 and read the real land pattern.
- **Purchasing.** Typed fields, `instance.set_properties` text parsing (`5.1k`, `22u`, `10V`), `resolve_groups`, `library.search` and an atomic `assign_part` gave a fully resolved BOM with values and MPNs.
- **Schematic.** Power ports, global labels across two sheets, the title block, no-connect marking and ERC behaved as documented. ERC was clean on the first pass.
- **High speed.**
  - `impedance.table` derived the pair geometry from the stackup with both inner planes as references: 0.266/0.152 mm on top.
  - `diff_pair.suggest` found USB_DP/USB_DN.
  - `route_pair` coupled the pair with mitred corners and fan-in.
  - `layout.tune_preview` planned an exact +1.000 mm trombone on the pair (not applied; see gap 2).
- **Routing.** 187 `route_step`s in walkaround mode routed the buses, power and control nets. Class minimums were enforced: 0.25 mm stubs on GND were refused under the Power class's 0.3 mm minimum.
- **Fills and keep-outs.** The module footprint's antenna keep-out kept the fills on all four layers clear without any board-level keep-out.
- **Rules.** Previewing and switching the profile (*Standard 6/6*) and the per-rule overrides worked through the API.
- **Fabrication.** The export is one command taking 0.14 s. Its outputs parse in an independent reader. The drill files match the project hole for hole. The BOM, pick-and-place and drawings are consistent.

## Desktop check

The agent opened the saved board in the desktop app (`target/nix/ci/pcbcraft`) with its own recovery, config and cache directories, used it, and closed it by its pid. The founder's windows were not touched. The screenshots are in the example's `images/`.

- **PCB 2D.** It opens on the board with 0 airwires and the 44 warnings in the Issues badge. The Board card shows the 4-layer stackup, the Standard 6/6 profile, 2 net classes, 1 pair and the sketched outline with `width`, `height` and `corner_r` editable. The Browser lists the 11 components with their Parts, both sheets and the 20 instances.
- **PCB 3D.**
  - The integrated 3D view shows the module's part-file body with its marking, the generated passives, SOT packages and headers, the copper, mask and legend, and the mounting holes.
  - Under the load described in gap 3 it took about 2–3 minutes to show "Preparing 3D model…" and then the model.
  - The first look showed the USB-C envelope box centred on the footprint origin, which is the PCB edge, so half of it hung off the board. The fix was a package placement offset of 3.675 mm in the replay. The 3D-position tools (`library.fit_leads`, `centre_on_pads`) work on typed leads, which an envelope does not have.
- **Not exercised in the UI:** routing, tuning, the Stackup and Production dialogs. The command engine covered these.
