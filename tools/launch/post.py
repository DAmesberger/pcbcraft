#!/usr/bin/env python3
"""Post-process Pcbcraft launch takes with ffmpeg (stdlib only).

  post.py take TAKE_DIR [--width 1920] [--stage 1920x1080]
                                              raw.mov + track.json -> TAKE_DIR/final.mp4; --stage floats
                                              the window on a 16:9 backdrop
  post.py reel OUT.mp4 TAKE_DIR...            join finished takes with short crossfades
  post.py hero IN.mp4 OUT_STEM --from S --to S [--width 1200]
                                              looping OUT_STEM.webp and OUT_STEM.gif for the README

A take directory holds what take.sh records: raw.mov (window-only capture), started.json (the wall
clock of the first recorded frame), window.json (window size in points and backing scale) and
track.json (the shot's pointer moves, clicks, captions, zooms and marks, wall-clock stamped).

`take` trims to the shot's `mark in` / `mark out` (or its whole length), converts to constant
60 fps, draws a cursor halo and click ripples from the track, applies the shot's `zoom` push-ins,
burns in its `caption`s in IBM Plex Sans (SIL OFL, shipped with Solidcraft's UI) and scales to the
output width. ffmpeg comes from nix: `nix shell nixpkgs#ffmpeg-full`, or FFMPEG=/path/to/ffmpeg.
"""

import glob
import json
import os
import subprocess
import sys

FPS = 60
FFMPEG = os.environ.get("FFMPEG", "ffmpeg")
RAMP = 0.7  # seconds for a zoom to ease in or out
HALO = 44  # cursor halo diameter in window points
RING = 64  # click ripple diameter in window points


def die(message):
    sys.stderr.write(message + "\n")
    sys.exit(1)


def run(args):
    sys.stderr.write("+ " + " ".join(a if len(a) < 200 else a[:200] + "…" for a in args) + "\n")
    subprocess.run(args, check=True)


def font():
    if os.environ.get("LAUNCH_FONT"):
        return os.environ["LAUNCH_FONT"]
    found = sorted(glob.glob(os.path.expanduser(
        "~/.cargo/git/checkouts/solidcraft-*/*/assets/fonts/ibm-plex-sans/IBMPlexSans-Variable.ttf")))
    if not found:
        die("IBM Plex Sans not found; set LAUNCH_FONT to a TTF with a permissive licence")
    return found[-1]


def sprites(directory, scale):
    """A soft halo for the pointer and a ring for clicks, drawn by ffmpeg itself."""
    halo = os.path.join(directory, "halo.png")
    ring = os.path.join(directory, "ring.png")
    h = int(HALO * scale) // 2 * 2
    r = int(RING * scale) // 2 * 2
    run([FFMPEG, "-y", "-loglevel", "error", "-f", "lavfi", "-i", f"color=c=black@0:s={h}x{h},format=rgba",
         "-frames:v", "1", "-vf",
         f"geq=r='255':g='225':b='120':a='255*0.42*clip(1-hypot(X-{h/2},Y-{h/2})/({h/2}),0,1)^0.8'", halo])
    run([FFMPEG, "-y", "-loglevel", "error", "-f", "lavfi", "-i", f"color=c=black@0:s={r}x{r},format=rgba",
         "-frames:v", "1", "-vf",
         f"geq=r='255':g='255':b='255':a='230*clip(1-abs(hypot(X-{r/2},Y-{r/2})-{r/2 - 3*scale})/({2*scale}),0,1)'",
         ring])
    return halo, ring, h, r


def smooth(a, b, clock="it"):
    """ffmpeg expression: 0 before a, eases to 1 over RAMP, holds, eases back to 0 ending at b."""
    up = f"clip(({clock}-{a:.3f})/{RAMP},0,1)"
    down = f"clip(({b:.3f}-{clock})/{RAMP},0,1)"
    e = f"min({up},{down})"
    return f"({e}*{e}*(3-2*{e}))"


def escape_text(text):
    return text.replace("\\", "\\\\").replace(":", "\\:").replace("'", "’").replace("%", "\\%")


def take(directory, width, stage=None):
    raw = os.path.join(directory, "raw.mov")
    track = json.load(open(os.path.join(directory, "track.json")))
    started = json.load(open(os.path.join(directory, "started.json")))["started"]
    window = json.load(open(os.path.join(directory, "window.json")))
    scale = window["scale"]
    events = [dict(e, t=e["t"] - started) for e in track]

    marks = {e["text"]: e["t"] for e in events if e["kind"] == "mark"}
    t0 = marks.get("in", max(0.0, events[0]["t"]))
    t1 = marks.get("out", events[-1]["t"] + 0.5)
    duration = t1 - t0
    rel = [dict(e, t=e["t"] - t0) for e in events if t0 - 1 <= e["t"] <= t1 + 1]

    halo, ring, h, r = sprites(directory, scale)

    # Pointer and click ripple positions as overlay commands, at most one per frame.
    cmds = []
    last_frame = -1
    for e in rel:
        if e.get("x") is None or e["kind"] == "begin":
            continue
        frame = int(max(0, e["t"]) * FPS)
        x = e["x"] * scale - h / 2
        y = e["y"] * scale - h / 2
        if e["kind"] == "move" and frame == last_frame:
            continue
        last_frame = frame
        cmds.append(f"{max(0, e['t']):.4f} overlay@halo x {x:.1f}, overlay@halo y {y:.1f};")
        if e["kind"] == "down":
            rx, ry = e["x"] * scale - r / 2, e["y"] * scale - r / 2
            cmds.append(f"{max(0, e['t']):.4f} overlay@ring x {rx:.1f}, overlay@ring y {ry:.1f};")
            cmds.append(f"{max(0, e['t']) + 0.32:.4f} overlay@ring x -4000, overlay@ring y -4000;")
    cmds.sort(key=lambda c: float(c.split(" ", 1)[0]))
    cmd_file = os.path.join(directory, "cursor.cmd")
    with open(cmd_file, "w") as f:
        f.write("\n".join(cmds) + "\n")
    has_pointer = any("halo" in c for c in cmds)

    # Zoom push-ins: z(t) = 1 + sum (z_i - 1) * env_i(t), centred on each zoom's point.
    zooms = [e for e in rel if e["kind"] == "zoom"]
    W = int(window["width"] * scale) // 2 * 2
    H = int(window["height"] * scale) // 2 * 2
    z_expr = "1"
    cx_expr = f"{W/2}"
    cy_expr = f"{H/2}"
    for e in zooms:
        env = smooth(e["t"], e["t"] + e["duration"])
        z_expr += f"+{e['zoom'] - 1:.3f}*{env}"
        cx_expr += f"+{e['x'] * scale - W/2:.1f}*{env}"
        cy_expr += f"+{e['y'] * scale - H/2:.1f}*{env}"

    out_w = width
    out_h = int(round(H * width / W / 2)) * 2
    if stage:
        # The window floats on a dark 16:9 backdrop with a soft shadow, whatever its own aspect.
        stage_w, stage_h = stage
        win_h = int(stage_h * 0.9) // 2 * 2
        win_w = int(win_h * W / H) // 2 * 2
        if win_w > stage_w * 0.94:
            win_w = int(stage_w * 0.94) // 2 * 2
            win_h = int(win_w * H / W) // 2 * 2
        out_w, out_h = stage_w, stage_h
    fnt = font()
    caption_filters = []
    size = int(out_h * 0.034)
    for e in rel:
        if e["kind"] != "caption":
            continue
        a, b = e["t"], e["t"] + e.get("duration", 3)
        alpha = f"min(clip((t-{a:.3f})/0.35,0,1),clip(({b:.3f}-t)/0.35,0,1))"
        caption_filters.append(
            f"drawtext=fontfile='{fnt}':text='{escape_text(e['text'])}':fontsize={size}:fontcolor=white:"
            f"box=1:boxcolor=0x101418@0.72:boxborderw={int(size*0.7)}|{int(size*1.1)}:"
            f"x=(w-text_w)/2:y=h-text_h-{int(out_h*0.075)}:alpha='{alpha}':enable='between(t,{a:.3f},{b:.3f})'")

    graph = [f"[0:v]fps={FPS},trim=start={t0:.4f}:duration={duration:.4f},setpts=PTS-STARTPTS,"
             f"scale={W}:{H}:in_color_matrix=bt709:in_range=tv,format=rgba[base]"]
    current = "base"
    if has_pointer:
        # sendcmd sits before the overlays so each frame's commands reach them in time.
        graph.append("[1:v]format=rgba,loop=-1:1:0[h]")
        graph.append("[2:v]format=rgba,loop=-1:1:0[r]")
        graph.append(f"[base]sendcmd=f='{cmd_file}'[bs]")
        graph.append("[bs][h]overlay@halo=x=-4000:y=-4000:eval=frame:shortest=1[c1]")
        graph.append("[c1][r]overlay@ring=x=-4000:y=-4000:eval=frame:shortest=1[c2]")
        current = "c2"
    if zooms:
        # zoompan with one output frame per input frame acts as an animated crop-and-scale.
        # Its clock is `it`, the input frame's time.
        graph.append(
            f"[{current}]zoompan=z='{z_expr}':"
            f"x='clip(({cx_expr})-iw/zoom/2,0,iw-iw/zoom)':y='clip(({cy_expr})-ih/zoom/2,0,ih-ih/zoom)':"
            f"d=1:s={W}x{H}:fps={FPS}[z]")
        current = "z"
    if stage:
        wx, wy = (stage_w - win_w) // 2, (stage_h - win_h) // 2 - int(stage_h * 0.012)
        graph.append(f"[{current}]scale={win_w}:{win_h}:flags=lanczos[win]")
        graph.append(f"gradients=s={stage_w}x{stage_h}:c0=0x1a2230:c1=0x07090d:x0={stage_w//2}:y0=0:"
                     f"x1={stage_w//2}:y1={stage_h}:speed=0:r={FPS},format=rgba[bg]")
        graph.append(f"color=c=black@0:s={stage_w}x{stage_h}:r={FPS},format=rgba,"
                     f"drawbox=x={wx}:y={wy + int(stage_h*0.012)}:w={win_w}:h={win_h}:color=black@0.75:t=fill,"
                     f"boxblur={int(stage_h*0.02)}[shadow]")
        graph.append("[bg][shadow]overlay=shortest=0[bgs]")
        graph.append(f"[bgs][win]overlay=x={wx}:y={wy}:shortest=1[staged]")
        current = "staged"
        tail = f"[{current}]null"
    else:
        tail = f"[{current}]scale={out_w}:{out_h}:flags=lanczos"
    if caption_filters:
        tail += "," + ",".join(caption_filters)
    tail += ",scale=out_color_matrix=bt709:out_range=tv,format=yuv420p[out]"
    graph.append(tail)

    script = os.path.join(directory, "graph.txt")
    with open(script, "w") as f:
        f.write(";\n".join(graph))
    out = os.path.join(directory, "final.mp4")
    inputs = ["-i", raw]
    if has_pointer:
        inputs += ["-i", halo, "-i", ring]
    run([FFMPEG, "-y", "-loglevel", "error", *inputs, "-/filter_complex", script, "-map", "[out]",
         "-r", str(FPS), "-c:v", "libx264", "-preset", "slow", "-crf", "17", "-pix_fmt", "yuv420p",
         "-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709",
         "-movflags", "+faststart", "-an", out])
    print(out)


def probe_duration(path):
    out = subprocess.run([FFMPEG, "-i", path], capture_output=True, text=True).stderr
    for line in out.splitlines():
        if "Duration:" in line:
            hms = line.split("Duration:")[1].split(",")[0].strip().split(":")
            return float(hms[0]) * 3600 + float(hms[1]) * 60 + float(hms[2])
    die(f"no duration for {path}")


def reel(out, dirs, fade=0.5):
    files = [os.path.join(d, "final.mp4") if os.path.isdir(d) else d for d in dirs]
    if len(files) == 1:
        run([FFMPEG, "-y", "-i", files[0], "-c", "copy", out])
        return
    inputs = []
    for f in files:
        inputs += ["-i", f]
    graph = []
    offset = 0.0
    prev = "0:v"
    for i in range(1, len(files)):
        offset += probe_duration(files[i - 1]) - fade
        label = f"x{i}"
        graph.append(f"[{prev}][{i}:v]xfade=transition=fade:duration={fade}:offset={offset:.3f}[{label}]")
        prev = label
    run([FFMPEG, "-y", "-loglevel", "warning", *inputs, "-filter_complex", ";".join(graph), "-map", f"[{prev}]",
         "-c:v", "libx264", "-preset", "slow", "-crf", "17", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
         out])
    print(out)


def hero(source, stem, start, end, width):
    span = ["-ss", f"{start}", "-t", f"{end - start}"]
    run([FFMPEG, "-y", "-loglevel", "warning", *span, "-i", source, "-vf",
         f"fps=30,scale={width}:-2:flags=lanczos", "-loop", "0", "-c:v", "libwebp_anim", "-quality", "80",
         "-compression_level", "6", stem + ".webp"])
    run([FFMPEG, "-y", "-loglevel", "warning", *span, "-i", source, "-vf",
         f"fps=20,scale={int(width*0.8)}:-2:flags=lanczos,split[a][b];[a]palettegen=max_colors=200:stats_mode=diff[p];"
         f"[b][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle", "-loop", "0", stem + ".gif"])
    print(stem + ".webp", stem + ".gif")


def main():
    args = sys.argv[1:]

    def opt(name, default):
        if name in args:
            i = args.index(name)
            value = args[i + 1]
            del args[i:i + 2]
            return value
        return default

    if not args:
        die(__doc__)
    cmd = args.pop(0)
    if cmd == "take":
        width = int(opt("--width", "1920"))
        stage = opt("--stage", None)
        take(args[0], width, tuple(int(v) for v in stage.split("x")) if stage else None)
    elif cmd == "reel":
        reel(args[0], args[1:])
    elif cmd == "hero":
        start = float(opt("--from", "0"))
        end = float(opt("--to", "6"))
        width = int(opt("--width", "1200"))
        hero(args[0], args[1], start, end, width)
    else:
        die(__doc__)


if __name__ == "__main__":
    main()
