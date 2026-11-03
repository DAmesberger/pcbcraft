# Launch recording pipeline

Reproducible takes of the desktop app for the README video ([storyboard](../../docs/launch/video-storyboard.md)).
macOS only (ScreenCaptureKit, macOS 15 or later).

| File | Role |
| --- | --- |
| `launchkit.swift` | `window` (frame and scale of a pid's window), `record` (that window only, 60 fps, backing resolution, H.264 `.mov`), `play` (a shot script as input events posted to that pid only, with a logged pointer track). |
| `shots/*.shot` | One script per storyboard shot: timed keys, pointer glides, drags, scrolls, plus `caption`, `zoom` and `mark in/out` cues for post. |
| `take.sh` | One take: a fresh app with private recovery, settings and cache folders, wait for the window, record, play, stop, close the app by its pid, post-process. |
| `post.py` | ffmpeg post: trim to the marks, constant 60 fps, cursor halo and click ripples from the track, push-in zooms, captions, MP4; `reel` joins takes with crossfades; `hero` writes a looping WebP and GIF. |
| `selftest.sh` | Checks the post chain on ffmpeg's test pattern, with no app and no capture. |

## Run a take

```sh
nix develop -c cargo build --locked --profile ci -p pcbcraft -p pcbcraft-cli
target/nix/ci/pcbcraft-cli showcase target/launch/showcase.pcbcraft

tools/launch/take.sh route-shove target/launch/showcase.pcbcraft OUT/route-shove --activate
tools/launch/take.sh board-3d    target/launch/showcase.pcbcraft OUT/board-3d    --activate
python3 tools/launch/post.py reel OUT/reel.mp4 OUT/route-shove OUT/board-3d
python3 tools/launch/post.py hero OUT/reel.mp4 OUT/hero --from 2 --to 8
```

Keep `OUT` outside the repository (large media are not committed). Each take directory holds
`raw.mov` (the untouched capture), `track.json`, `window.json`, `started.json`, the app's log and
`final.mp4`.

- **Window and theme.** The app opens at its default 1380 × 900 pt; shot coordinates are window
  points for that size. The app follows the system appearance: switch macOS to Dark (or Light)
  before a take; the storyboard uses Dark.
- **`--activate`** brings the take's own window to the front for the duration of the take (posted
  events are still addressed to its pid only). Without it, input goes to a background window,
  which egui does not always accept. Do not use the mouse or keyboard during a take.
- **`--settle S`** waits S seconds after the window appears (default 6) so the 3D model is built.
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
