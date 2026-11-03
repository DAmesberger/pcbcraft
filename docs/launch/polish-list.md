# Launch polish list

A critique of the proof takes (3 November 2026; `route-shove`, `board-3d` and `component-3d` from
[`tools/launch`](../../tools/launch/README.md), recorded on the current `component-model` build).
App changes are listed for their owners; the launch work changes no app code.

**Verdict.** The 3D shots already look good: the studio light, the lacquered mask, the sectioned
edge and the etched WROOM-3 can hold up at 1080p. The 2D routing shot does not look cool yet. The
push-and-shove is real but hard to see, and the orange ground pour swamps the frame. Fix the
first group before the final recording.

## Must fix before the final take

| # | What the take shows | Suggested change | Owner area |
|---|---|---|---|
| 1 | **The shove is barely visible.** The route preview and the pushed tracks are drawn as hairline outlines, so the moment the bundle moves reads as a flicker. | Draw the preview at full width as translucent copper. Draw shoved tracks solid in their new place, with their old place as a faint ghost during the drag. | Route tool (`route-tool`, `push-shove`) |
| 2 | **The bottom ground pour dominates 2D.** The whole board is a saturated orange slab, and the top tracks read olive on orange. Routing GND turns the whole board bright blue. | Dim and desaturate inactive layers, draw fills lighter than tracks, and highlight a net's fill by its outline and hatch, not a solid flood. | Layer focus (`layer-focus`), PCB 2D |
| 3 | **The Component 3D triad covers the markings** (it sits on the etched name), and a right-drag that starts on it edits the placement: the first take moved the module off its lands. | Hide the triad until the model is selected (or add a View toggle), and never start a placement edit from a right-drag. | Component 3D (`markings-3d`, `part-placement`) |
| 4 | **The toast shows a full local path:** "Opened /Users/…/.claude/worktrees/…/target…". It looks unfinished and leaks the folder layout. | Show the file name only. Keep the full path in File › Messages. | Shell feedback |
| 5 | **The canvas hints sit on the picture:** "Shift + two fingers to orbit…" over the 3D board, and "Triad snaps…" in Component 3D. | Fade the hints after the first interaction, or add a presentation mode that hides hints, toasts and the status line. | Shell, viewport |
| 6 | **The first 3D model can take minutes under load.** At a load average of about 500 (other agents building), "Preparing 3D model…" stayed for over a minute with no progress. The showcase builds in about 1 s on an idle machine. | Show progress or a stage name. Record the final take on an idle machine; the pipeline's `--prelude` and `--settle` cover the wait. | Native build, recording plan |

## Should fix

| # | Observation | Suggestion |
|---|---|---|
| 7 | No scripted access to the running desktop. Takes need HID pointer events (macOS drops pointer events posted to a background app), so a take brings its window to the front and moves the real pointer. | Add a desktop automation socket that accepts the same typed commands plus view commands (`view.orbit_to`, `view.fit`, workspace and tool switches). Takes could then run in the background, and storyboard shot 12 could show an MCP agent editing the live window. |
| 8 | View keys (1–7, Fit) jump without easing, so smooth camera moves have to be faked with synthetic drags. | Ease camera transitions over about 400 ms. Then scripts can use the app's own commands for camera moves. |
| 9 | On this machine the window is resized to 1500 × 1235 pt as soon as it opens (probably the terminal's window tiling; Accessibility resize is refused). That aspect is not 16:9. | `post.py --stage 1920x1080` floats the window on a dark backdrop. A `--window-size` launch flag would help on machines without tiling. |
| 10 | The theme follows the system, so a take's look depends on the person's appearance setting. | Add a `--theme dark|light` flag (or a setting) for recordings. |
| 11 | UI text is small at 1080p, because the whole 1500 pt window is shown. | Use push-ins (`zoom`) on details, as the takes do. Consider a UI scale setting (⌘+/⌘−) for recordings. |
| 12 | The Browser root reads "Untitled" for `showcase.pcbcraft`. | Name the root after the file. |
| 13 | 3D showcase: the header pins look flat yellow, the floor is a flat dark grey, and no silkscreen shows on top. | Brushed or plated look for pins, a subtle floor reflection or gradient, and the legend layer in 3D. The Realistic mode and material graphs in progress are the natural place for this. |
| 14a | In the staged 1080p frame the 3D board fills only about a third of the picture: the Browser and Properties take the sides, and the canvas leaves wide margins around the fitted board. | Hide both panels for 3D shots (a single "focus canvas" command), fit tighter (the 8 % margin is generous for video), and crop the hero to the canvas (`post.py hero --crop`, as the proof hero does). |
| 14 | The orbit in `component-3d` drifts the module towards the left edge, because the orbit pivots on the point under the pointer. | Orbit about the selection or model centre when the drag starts on empty canvas, or add `view.orbit_about_selection`. |

## Recording conditions for the final take

- Use an idle machine (no parallel builds), macOS in Dark mode and the default window size, or `--stage`.
- Wait for the ESP32 dev board, the Realistic mode and the mechanical parts ([storyboard dependencies](video-storyboard.md#dependencies-record-the-final-take-after-these-land)).
- Re-measure shot coordinates after UI changes; the scripts use window points.
- Captions must stay true. A draft caption in the proof take claimed a pin-1 mark that the WROOM-3 marking does not have; it was corrected to "laser etch and a QR code" before rendering.
