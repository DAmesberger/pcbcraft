# Launch recording pipeline

Reproducible takes of the desktop app for the README video ([storyboard](../../docs/launch/video-storyboard.md)).
macOS only (ScreenCaptureKit, macOS 15 or later).

| File | Role |
| --- | --- |
| `launchkit.swift` | `window` (frame and scale of a pid's window), `record` (that window only, 60 fps, backing resolution, H.264 `.mov`; stops on a stop file, SIGINT/SIGTERM or `--seconds`), `play` (a shot script as input events, with a logged pointer track), `resize` (Accessibility resize, where the window manager allows it). |
| `shots/*.shot` | One script per storyboard shot: timed keys, pointer glides, drags, scrolls, plus `caption`, `zoom` and `mark in/out` cues for post. |
| `take.sh` | One take: a fresh app with private recovery, settings, cache and home folders, wait for the window, an optional unrecorded `--prelude` shot, record, play, stop, close the app by its pid, post-process. |
| `open.sh` | Start the app with the same private state and print its pid and window frame (and a still), for measuring shot coordinates. |
| `post.py` | ffmpeg post: trim to the marks, constant 60 fps, cursor halo and click ripples from the track, push-in zooms, captions, MP4 (`--stage 1920x1080` floats the window on a 16:9 backdrop); `reel` joins takes with crossfades; `hero` writes a looping WebP and GIF. |
| `selftest.sh` | Checks the post chain on ffmpeg's test pattern, with no app and no capture. |

## Run a take

```sh
nix develop -c cargo build --locked --profile ci -p sparksmith -p sparksmith-cli
target/nix/ci/sparksmith showcase target/launch/showcase.sparksmith

cp examples/esp32-s31-wroom-3/esp32-s31-wroom-3.sparksmith target/launch/wroom3.sparksmith

tools/launch/take.sh route-shove  target/launch/showcase.sparksmith OUT/route-shove  --activate --no-post
tools/launch/take.sh board-3d     target/launch/showcase.sparksmith OUT/board-3d     --activate --no-post \
  --prelude board-3d.prelude --settle 20
tools/launch/take.sh component-3d target/launch/wroom3.sparksmith   OUT/component-3d --activate --no-post
for t in route-shove board-3d component-3d; do python3 tools/launch/post.py take OUT/$t --stage 1920x1080; done
python3 tools/launch/post.py reel OUT/reel.mp4 OUT/route-shove OUT/board-3d OUT/component-3d
python3 tools/launch/post.py hero OUT/board-3d/final.mp4 OUT/hero --from 0.6 --to 6.6 --width 960
```

Keep `OUT` outside the repository (large media are not committed). Each take directory holds
`raw.mov` (the untouched capture), `track.json`, `window.json`, `started.json`, the app's log and
`final.mp4`.

- **Window and theme.** The app asks for 1380 × 900 pt. On the machine the proof takes were made on,
  the window is resized to 1500 × 1235 pt as it opens (the terminal's window tiling; Accessibility
  resize is refused), and the shot scripts' coordinates are window points for that size. Check
  `window.json` of a take; re-measure with `open.sh` if the size differs. The app follows the
  system appearance: switch macOS to Dark before a take, as the storyboard does.
- **Input.** Keys are posted to the take's pid only and work with the window in the background.
  macOS drops pointer events posted to a background app, so pointer moves, clicks, drags and
  scrolls need `--activate`: the take's window comes to the front and pointer events go through
  the HID tap, which **moves the real pointer** during the take. Before every pointer event the
  player checks that the frontmost window under that point is the take's own, and aborts the take
  otherwise, so no click can reach another app. At the end it puts the pointer back and
  re-activates the app that was in front. Do not use the mouse or keyboard during a take.
- **`--prelude SHOT`** plays an unrecorded setup shot (for example the switch to 3D) before
  recording; **`--settle S`** then waits S seconds (default 6) so background builds finish. On a
  heavily loaded machine the first 3D model can take minutes.
- **Repeatability.** Every take starts from the same file and a blank app state. Pointer moves are
  eased at 120 Hz, so two takes of a script differ only by the app's own timing (background model
  builds and fills).

## Permissions (once)

- **Screen Recording** for the terminal app that runs `take.sh` (System Settings › Privacy &
  Security › Screen & System Audio Recording). Without it `record` exits with "ScreenCaptureKit
  refused" and the take stops before anything is played.
- **Accessibility** for the same terminal (System Settings › Privacy & Security › Accessibility),
  so posted key and mouse events reach the app. `play` warns when it is missing.

## Rules the pipeline keeps

- It records one window, the take's own app, never the whole display: other apps' content
  cannot appear in a take.
- It closes only the app it started, by pid, also on failure (no broad `pkill`).
- Captions use IBM Plex Sans (SIL Open Font License), found in Solidcraft's pinned checkout; set
  `LAUNCH_FONT` to use another font with a permissive licence. There is no music track.
