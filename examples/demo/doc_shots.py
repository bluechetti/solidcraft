#!/usr/bin/env python3
"""doc_shots.py [--list] [SHOT...]: capture the user guide's screenshots and short clips from the
real app, so every figure can be made again with one command when the UI changes.

    python3 examples/demo/doc_shots.py                       # every shot
    python3 examples/demo/doc_shots.py ui-tour/window        # one shot
    python3 examples/demo/doc_shots.py --list

Each shot starts a fresh SolidCraft (its own settings folder, so your preferences never show) in
an Xvfb screen, drives it over the control channel the way a person would (enclosure_ui_mcp.py's
Demo: the pointer glides to a button and clicks it, values are typed), and writes
docs/book/src/media/<page>/<name>.png (a still, with numbered callouts where the shot asks for
them) or .mp4 (a clip, with the drawn pointer and click ripples of overlay.py, idle stretches cut by
trim_idle.py). Shots are the functions below; the page that shows a shot explains it. --light makes
the stills again in the light theme, as <name>.light.png; the book shows the one that matches its
own theme (docs/book/theme/media.css).

The scan-clean shots read openly licensed scans from SCAN_CORPUS (see CORPUS below); the scans
are not in the repository. Needs Xvfb, ffmpeg (x11grab) and Pillow, and a release build of the app (`--app`, default
$CARGO_TARGET_DIR/release/solidcraft, else target/release/solidcraft). Every new file needs an
ATTRIBUTION row (attribution/pending/).
"""

import argparse
import json
import os
import secrets
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))
from enclosure_ui_mcp import Demo  # noqa: E402
from overlay import cursor_sprite, ripple_sprite  # noqa: E402

# The window every shot is taken in: the book column shows it at about 60 %.
SIZE = (1280, 800)
FPS = 30
MEDIA = ROOT / "docs/book/src/media"
FONT = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
# Callout disc colours (an original blue with a white rim, legible on dark and light).
CALLOUT_FILL = (40, 120, 230)
CALLOUT_RIM = (255, 255, 255)

SHOTS = {}
# --check DIR: clips go to DIR with a contact sheet each, never into the book's media.
CHECK = None


def shot(name, kind, start="empty", setup=None):
    """Register a shot: `kind` is "still" or "clip"; `start` is "empty" (a new design) or
    "sample" (the built-in sample plate); `setup(d)`, when given, runs first and is not
    recorded."""

    def register(f):
        SHOTS[name] = (kind, start, f, setup)
        return f

    return register


# -- the app ------------------------------------------------------------------------------------


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class App:
    """A fresh SolidCraft on DISPLAY with its own settings, the control channel on a free port."""

    def __init__(self, exe, display, start, work):
        self.port = free_port()
        self.token = secrets.token_hex(32)
        os.environ["SOLIDCRAFT_CONTROL_TOKEN"] = self.token
        settings = Path(work) / "settings"
        settings.mkdir(parents=True, exist_ok=True)
        env = dict(
            os.environ,
            DISPLAY=display,
            SOLIDCRAFT_SETTINGS_DIR=str(settings),
            XDG_DATA_HOME=str(Path(work) / "data"),
            XDG_CONFIG_HOME=str(Path(work) / "config"),
            XDG_CACHE_HOME=str(Path(work) / "cache"),
            SOLIDCRAFT_RENDERER="cpu",
            LANG="en_US.UTF-8",
        )
        # On a Wayland desktop the app would open its window there, not on the Xvfb screen.
        env.pop("WAYLAND_DISPLAY", None)
        argv = [exe, "--control", str(self.port)] + (["--sample"] if start == "sample" else [])
        self.log = open(Path(work) / "app.log", "w")
        self.p = subprocess.Popen(argv, env=env, stdout=self.log, stderr=subprocess.STDOUT)
        self.wait_ready()

    def wait_ready(self):
        from enclosure_mcp import control

        for _ in range(1800):
            if self.p.poll() is not None:
                raise RuntimeError(f"the app exited ({self.p.returncode}); see {self.log.name}")
            try:
                r = control(self.port, "ui.inspect")
                if r.get("ok") and r.get("result", {}).get("viewport"):
                    return
            except OSError:
                pass
            time.sleep(0.1)
        raise RuntimeError("the app did not answer on its control port within 3 minutes")

    def close(self):
        from enclosure_mcp import control

        try:
            control(self.port, "app.quit", {"force": True})
            self.p.wait(10)
        except Exception:
            self.p.kill()
            self.p.wait()
        self.log.close()


class Shot(Demo):
    """Demo, plus what a doc shot needs: engine commands, the crop and the callouts."""

    def __init__(self, port, log):
        super().__init__(port, None, log, speed=1.0, minimum_caption=0)
        self.crop = None  # (x, y, w, h) in window points; None: the whole window
        self.callouts = []  # [(x, y)] in window points, numbered from 1 in order
        self.tour = None  # a tour's steps (see tour_step), written beside the clip with --check
        self.keep_pauses = False  # a clip whose pauses matter (hovering) keeps them

    def execute(self, command, params=None):
        self.settle_camera()
        r = self.ui("engine.execute", {"command": command, "params": params or {}})
        self.settle_camera()
        return r

    def enter(self):
        """Enter as two input frames: pressed and released in one frame, a sketch value box
        misses it."""
        self.ui("ui.key", {"key": "Enter", "pressed": True})
        time.sleep(0.1)
        self.ui("ui.key", {"key": "Enter", "pressed": False})
        self.wait(0.4)
        self.settle_camera()

    def type(self, text, enter=True):
        self.ui("ui.text", {"text": text})
        self.wait(0.4)
        if enter:
            self.enter()

    def sketch_scale(self):
        """Points per millimetre in the open sketch."""
        a, b = self.at(sketch=[0, 0]), self.at(sketch=[10, 0])
        return abs(b[0] - a[0]) / 10

    def frame_sketch(self, centre, scale):
        """Look at `centre` (mm on a sketch on XY) at `scale` points per mm, keeping the view's
        direction."""
        cam = self.ui("ui.inspect")["camera"]
        self.ui("ui.camera", {"target": [centre[0], centre[1], 0], "distance": cam["distance"] * self.sketch_scale() / scale})
        self.settle_camera()

    def handle(self, name):
        """The handle's screen point, or None when it isn't drawn."""
        return next(((h["x"], h["y"]) for h in self.ui("ui.handles") if h["name"] == name), None)

    def prepare(self):
        """The same starting window for every shot: SIZE, the default layout, no start page."""
        self.ui("ui.resize", {"width": SIZE[0], "height": SIZE[1]})
        for _ in range(100):
            vp = self.ui("ui.inspect").get("viewport") or [0, 0, 0, 0]
            if vp[0] + vp[2] >= SIZE[0] - 10:
                break
            time.sleep(0.1)
        if self.ui("ui.home").get("open"):
            self.ui("ui.home", {"open": False})
        self.pos = (SIZE[0] * 0.6, SIZE[1] * 0.55)
        self.settle_camera()


# -- capture ------------------------------------------------------------------------------------


def run_shot(name, exe, display, keep, light=False):
    kind, start, f, setup = SHOTS[name]
    work = Path(tempfile.mkdtemp(prefix="doc-shot-"))
    out = MEDIA / f"{name}{'.light' if light else ''}.{'png' if kind == 'still' else 'mp4'}"
    if CHECK:
        out = CHECK / f"{name.replace('/', '-')}.{'png' if kind == 'still' else 'mp4'}"
    out.parent.mkdir(parents=True, exist_ok=True)
    app = App(exe, display, start, work)
    d = None
    try:
        log = open(work / "steps.log", "w")
        d = Shot(app.port, log)
        d.prepare()
        if light:
            d.ui("ui.set", {"dark": False})
            d.settle_camera()
        if setup:
            setup(d)
            d.settle_camera()
            log.seek(0)
            log.truncate()
        if kind == "still":
            f(d)
            d.settle_camera()
            raw = work / "window.png"
            r = d.ui("ui.screenshot", {"path": str(raw)})
            if r and r.get("hidden"):
                raise RuntimeError("the window was not presented")
            still(raw, out, d.crop, d.callouts)
        else:
            try:
                clip(d, f, display, work, out)
            finally:
                if CHECK and d.tour is not None:
                    text = tour_text(name, d.tour)
                    out.with_suffix(".tour.txt").write_text(text)
                    for line in text.splitlines():
                        if "!!" in line or line.startswith("--"):
                            print(f"  {line.strip()}", file=sys.stderr)
            if CHECK:
                print(f"{sheet(work, out.with_suffix('.png'))}: the steps as stills", file=sys.stderr)
    except Exception:
        if CHECK and d:
            # Where it stopped, to look at instead of the log.
            failed = out.with_suffix(".failed.png")
            try:
                d.ui("ui.screenshot", {"path": str(failed)})
                print(f"{failed}: the window when it failed", file=sys.stderr)
            except Exception:
                pass
        raise
    finally:
        app.close()
        if keep:
            print(f"  work files: {work}", file=sys.stderr)
        else:
            shutil.rmtree(work, ignore_errors=True)
    shown = out.relative_to(ROOT) if out.is_relative_to(ROOT) else out
    print(f"{shown}: {out.stat().st_size // 1024} KB")


def still(raw, out, crop, callouts):
    from PIL import Image, ImageDraw, ImageFont

    img = Image.open(raw).convert("RGB")
    lo, hi = img.getextrema()[0]
    if hi - lo < 8:
        raise RuntimeError(f"{raw}: the screenshot is blank")
    draw = ImageDraw.Draw(img)
    font = ImageFont.truetype(FONT, 17)
    for i, (x, y) in enumerate(callouts, 1):
        r = 15
        draw.ellipse([x - r - 2, y - r - 2, x + r + 2, y + r + 2], fill=CALLOUT_RIM)
        draw.ellipse([x - r, y - r, x + r, y + r], fill=CALLOUT_FILL)
        draw.text((x, y), str(i), font=font, fill=CALLOUT_RIM, anchor="mm")
    if crop:
        x, y, w, h = crop
        img = img.crop((x, y, x + w, y + h))
    img.save(out, optimize=True)
    # Screen captures are mostly flat colour: a 256-colour palette keeps them sharp at a third of
    # the size. Keep it only when it is smaller.
    q = out.with_suffix(".q.png")
    img.quantize(256, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE).save(q, optimize=True)
    if q.stat().st_size < out.stat().st_size:
        q.replace(out)
    else:
        q.unlink()


def clip(d, f, display, work, out):
    raw = work / "raw.mp4"
    ff = subprocess.Popen(
        ["ffmpeg", "-loglevel", "info", "-nostats", "-y", "-f", "x11grab", "-draw_mouse", "0", "-video_size", f"{SIZE[0]}x{SIZE[1]}", "-framerate", str(FPS), "-i", display, "-c:v", "libx264", "-preset", "veryfast", "-crf", "12", "-pix_fmt", "yuv444p", str(raw)],
        stderr=open(work / "ffmpeg.log", "w"),
    )
    try:
        time.sleep(1.0)
        d.mark(start=True)
        f(d)
        d.settle_camera()
        time.sleep(0.8)
    finally:
        ff.send_signal(2)
        ff.wait(30)
    log = (work / "ffmpeg.log").read_text()
    start = float(log.split("start: ", 1)[1].split(",", 1)[0])
    (work / "start.txt").write_text(str(start))
    composite(work, raw, start, d.crop, out, d.keep_pauses)


def sheet(work, out, width=640):
    """One still per captioned step (the moment before the next caption, so each shows that
    step's result) and the last frame, two to a row: a clip checked with one small picture."""
    from PIL import Image

    full = work / "full.mp4"
    start = float((work / "start.txt").read_text())
    marks = [json.loads(line) for line in open(work / "steps.log") if line.strip()]
    length = float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(full)], capture_output=True, text=True, check=True).stdout)
    caps = [m["t"] - start for m in marks if m.get("caption")]
    times = [max(t - 0.25, 0.0) for t in caps[1:]] + [length - 0.1]
    if len(times) < 2:
        times = [length * k / 4 for k in (1, 2, 3)] + [length - 0.1]
    frames = []
    for i, t in enumerate(times):
        png = work / f"sheet-{i}.png"
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-ss", f"{t:.2f}", "-i", str(full), "-frames:v", "1", "-vf", f"scale={width}:-2", str(png)], check=True)
        frames.append(Image.open(png).convert("RGB"))
    w, h = frames[0].size
    rows = (len(frames) + 1) // 2
    img = Image.new("RGB", (w * min(2, len(frames)), h * rows), (0, 0, 0))
    for i, fr in enumerate(frames):
        img.paste(fr, ((i % 2) * w, (i // 2) * h))
    img.save(out, optimize=True)
    return out


def composite(work, raw, start, crop, out, keep_pauses=False):
    """The drawn pointer and click ripples over the recording, cropped, idle cut, H.264."""
    marks = [json.loads(line) for line in open(work / "steps.log") if line.strip()]
    cursor_sprite(str(work / "cursor.png"))
    ripple_sprite(str(work / "ripple.png"))
    pts, clicks = [], []
    for i, m in enumerate(marks):
        t = m["t"] - start
        if "cursor" in m:
            pts.append((t, *m["cursor"]))
            if m.get("click"):
                clicks.append((t, *m["cursor"]))
        elif "drag_start" in m:
            end = next((n for n in marks[i + 1 :] if "drag_end" in n), None)
            if end:
                (x0, y0), (x1, y1), t1 = m["drag_start"], end["drag_end"], end["t"] - start
                n = max(2, int((t1 - t) * FPS))
                pts += [(t + (t1 - t) * k / n, x0 + (x1 - x0) * k / n, y0 + (y1 - y0) * k / n) for k in range(n + 1)]
    pts.sort()
    cmds = [f"{t:.3f} [enter] overlay@cur x {x:.1f}, [enter] overlay@cur y {y:.1f};" for t, x, y in pts]
    # Each ripple moved onto its click and off again by command: an enable expression with one term
    # per click grows past what ffmpeg takes on a long tour.
    cmds += [f"{t:.3f} [enter] overlay@rip x {x - 22:.1f}, [enter] overlay@rip y {y - 22:.1f};" for t, x, y in clicks]
    cmds += [f"{t + 0.28:.3f} [enter] overlay@rip x -100, [enter] overlay@rip y -100;" for t, _, _ in clicks]
    (work / "cursor.cmd").write_text("\n".join(sorted(cmds, key=lambda c: float(c.split()[0]))) + "\n")
    cx, cy = (pts[0][1], pts[0][2]) if pts else (-100, -100)
    x, y, w, h = crop or (0, 0, *SIZE)
    w, h = w // 2 * 2, h // 2 * 2
    graph = f"[0:v][2:v]overlay@rip=x=-100:y=-100:shortest=1[v1];[v1][1:v]overlay@cur=x={cx}:y={cy}:shortest=1,sendcmd=f={work / 'cursor.cmd'},crop={w}:{h}:{x}:{y}"
    if labels(work, marks, start, w, h):
        graph += f",ass={work / 'labels.ass'}"
    graph += "[v]"
    full = work / "full.mp4"
    subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-y", "-i", str(raw), "-loop", "1", "-i", str(work / "cursor.png"), "-loop", "1", "-i", str(work / "ripple.png"), "-filter_complex", graph, "-map", "[v]", "-c:v", "libx264", "-crf", "14", "-pix_fmt", "yuv420p", str(full)],
        check=True,
    )
    if keep_pauses:
        subprocess.run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(full), "-c:v", "libx264", "-crf", "26", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(out)], check=True)
    else:
        # Idle stretches shortened, not removed: a reader needs a moment on each result.
        subprocess.run([sys.executable, str(HERE / "trim_idle.py"), str(full), str(out), "--crf", "26", "--hold", "0.8"], check=True)


def labels(work, marks, start, w, h):
    """The shot's d.caption() labels (which button does what) in the clip's top-left corner,
    each until the next one. False when the shot has none."""
    caps = [m for m in marks if "caption" in m]
    if not caps:
        return False

    def ts(t):
        t = max(t, 0.0)
        return f"{int(t // 3600)}:{int(t % 3600 // 60):02}:{t % 60:05.2f}"

    head = f"""[Script Info]
ScriptType: v4.00+
PlayResX: {w}
PlayResY: {h}

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Label,DejaVu Sans,24,&H00FFFFFF,&H00FFFFFF,&H40000000,&H40000000,1,0,0,0,100,100,0,0,3,8,0,7,24,24,24,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
"""
    end = marks[-1]["t"] - start + 5
    lines = []
    for i, m in enumerate(caps):
        a = m["t"] - start
        b = caps[i + 1]["t"] - start if i + 1 < len(caps) else end
        if m["caption"]:
            lines.append(f"Dialogue: 0,{ts(a)},{ts(b)},Label,,0,0,0,,{m['caption']}")
    (work / "labels.ass").write_text(head + "\n".join(lines) + "\n")
    return True


# -- the shots ----------------------------------------------------------------------------------
# ui-tour.md


@shot("ui-tour/window", "still", start="sample")
def ui_tour_window(d):
    """The window, numbered in the order ui-tour.md describes it: 1 title bar, 2 toolbar,
    3 browser, 4 viewport, 5 axis view, 6 navigation bar, 7 timeline, 8 command search."""
    d.key("Escape")
    d.ui("ui.move", {"x": 5, "y": 400})
    names = {h["name"]: (h["x"], h["y"]) for h in d.ui("ui.handles")}
    vp = d.ui("ui.inspect")["viewport"]
    # Each callout sits in empty space beside what it names.
    want = [
        ("titlebar:assistant", (-46, 0)),
        ("ribbon:add_tab", (36, 0)),
        ("browser:header", (0, 0)),
        (None, (vp[0] + vp[2] * 0.33, vp[1] + vp[3] * 0.8)),
        ("axis_view:-x", (-44, 0)),
        ("nav:slot:orbit", (-40, 0)),
        ("history:play", (0, -36)),
        ("ribbon:search", (-100, 0)),
    ]
    for name, (dx, dy) in want:
        if name is None:
            d.callouts.append((dx, dy))
        elif name in names:
            x, y = names[name]
            d.callouts.append((x + dx, y + dy))
        else:
            raise RuntimeError(f"no handle {name}; the window's handles changed")


@shot("ui-tour/navigate", "clip", start="sample")
def ui_tour_navigate(d):
    """Orbit (right drag), pan (middle drag), zoom (wheel) and back with the axis view's Home."""
    vp = d.ui("ui.inspect")["viewport"]
    # The viewport right of the browser, down to the navigation bar.
    d.crop = (262, vp[1] + 126, SIZE[0] - 262, 590)
    c = (vp[0] + vp[2] * 0.62, vp[1] + vp[3] * 0.5)
    d.glide(c, 0.6)
    d.caption("Right-drag: orbit")
    d.orbit(240, 0, steps=70)
    d.orbit(0, 40, steps=30)
    d.wait(0.6)
    d.caption("Middle-drag: pan")
    d.drag(d.pos, (d.pos[0] - 160, d.pos[1] - 40), button="middle", steps=50)
    d.wait(0.6)
    d.caption("Wheel: zoom at the pointer")
    for delta in (40, 40, 40, -40, -40, -40):
        d.ui("ui.scroll", {"x": d.pos[0], "y": d.pos[1], "delta": delta})
        time.sleep(0.25)
    d.settle_camera()
    d.wait(0.6)
    d.caption("Home: back to the starting view")
    d.click(d.at(handle="axis_view:home"), dur=0.6, settle=0.3)
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()
    d.wait(1.0)


# sketching.md: one sketch on the Top (XY) plane, a 60 x 40 rectangle from the origin and a
# 12 mm hole placed 20 mm from two sides, step by step. Each clip starts where the last ends.
RECT = (60, 40)
HOLE = (20, 20, 12)


def top_sketch(d):
    """A sketch open on the Top plane, framed on where the rectangle will go."""
    d.execute("sketch.create", {"plane": "XY"})
    d.ui("ui.view", {"view": "top"})
    d.settle_camera()
    d.frame_sketch((RECT[0] / 2, RECT[1] / 2 - 4), 7.5)


def draw_rectangle(d):
    d.tool("sketch.rectangle.two_point")
    d.click(d.at(sketch=[0, 0]), dur=0.6, settle=0.3)
    d.glide(d.at(sketch=[RECT[0] * 0.7, RECT[1] * 0.7]), 0.8)
    d.wait(0.3)
    d.type(str(RECT[0]), enter=False)
    d.key("Tab")
    d.type(str(RECT[1]), enter=False)
    # A click places the shape at the typed size, wherever the pointer is.
    d.click(d.pos, dur=0.1, settle=0.4)
    d.key("Escape")


def draw_circle(d):
    # Off its final place, so the dimensions visibly move it there.
    d.tool("sketch.circle.center")
    d.click(d.at(sketch=[HOLE[0] - 4, HOLE[1] + 5]), dur=0.6, settle=0.3)
    # Down and right: the diameter box opens above the pointer, and over it the tool can't see
    # the pointer move.
    d.glide(d.at(sketch=[HOLE[0] + 4, HOLE[1] - 1]), 0.5)
    d.type(str(HOLE[2]), enter=False)
    d.click(d.pos, dur=0.1, settle=0.4)
    d.key("Escape")


def drawn(d):
    top_sketch(d)
    draw_rectangle(d)
    draw_circle(d)
    if len(d.execute("sketch.inspect")["curves"]) != 5:
        raise RuntimeError("the rectangle and the circle were not drawn")


def dimension_hole(d):
    """Dimension the hole's centre from the left side, then from the bottom."""
    d.tool("sketch.dimension")
    centre = lambda: d.at(sketch=d.hole_centre())  # noqa: E731
    d.click(centre(), dur=0.6, settle=0.2)
    d.click(d.at(sketch=[0, RECT[1] * 0.85]), dur=0.5, settle=0.2)
    d.click(d.at(sketch=[HOLE[0] / 2, RECT[1] + 7]), dur=0.5, settle=0.3)
    d.type(str(HOLE[0]))
    d.click(centre(), dur=0.6, settle=0.2)
    d.click(d.at(sketch=[RECT[0] * 0.8, 0]), dur=0.5, settle=0.2)
    d.click(d.at(sketch=[RECT[0] * 0.75, HOLE[1] / 2]), dur=0.5, settle=0.3)
    d.type(str(HOLE[1]))
    d.key("Escape")


def hole_centre(d):
    sk = d.execute("sketch.inspect")
    c = next(c for c in sk["curves"] if c["type"] == "circle")
    return list(c["center_at"])


Shot.hole_centre = hole_centre


@shot("sketching/create", "clip")
def sketching_create(d):
    """New Sketch, hover the planes (each lights up), click Top: the view turns to face it."""
    d.keep_pauses = True
    d.caption("New Sketch")
    d.tool("sketch.create")
    o = d.at(world=[0, 0, 0])
    d.caption("Hover a plane: it lights up")
    d.glide((o[0] + 20, o[1] - 45), 0.8)
    d.wait(1.0)
    d.glide((o[0] + 62, o[1] + 1), 0.6)
    d.wait(1.0)
    d.caption("Click it: the sketch opens facing you")
    d.click((o[0] + 62, o[1] + 1), dur=0.2, settle=1.5)
    plane = d.execute("sketch.inspect")["plane"]
    if plane["x_dir"] != [1.0, 0.0, 0.0] or plane["y_dir"] != [0.0, 1.0, 0.0]:
        raise RuntimeError(f"the sketch went on the wrong plane: {plane}")
    d.wait(1.0)


@shot("sketching/draw", "clip", setup=top_sketch)
def sketching_draw(d):
    """A rectangle from the origin with its sides typed (60 Tab 40, click), then a circle with
    its diameter typed: each typed value becomes a dimension."""
    d.caption("Rectangle: click the origin, type 60, Tab, 40, click")
    draw_rectangle(d)
    d.wait(0.8)
    d.caption("Circle: click its centre, type 12, click")
    draw_circle(d)
    d.wait(1.2)


@shot("sketching/dimension", "clip", setup=drawn)
def sketching_dimension(d):
    """Dimension the hole's centre from two sides: the sketch turns fully constrained."""
    d.caption("Dimension: click the centre, then a side, place it, type 20")
    dimension_hole(d)
    if d.execute("sketch.inspect").get("dof") != 0:
        raise RuntimeError("the sketch is not fully constrained")
    d.caption("Fully constrained: nothing left to move")
    d.wait(2.0)


NOTCH = (45, RECT[1], 14)  # a circle centred on the top side, for Trim


def offset_outline(d, label=lambda text: None):
    label("Offset: click the outline, type 4, Enter")
    d.tool("sketch.offset")
    d.click(d.at(sketch=[RECT[0] / 2, 0]), dur=0.6, settle=0.8)
    d.wait(0.4)
    d.ui("ui.key", {"key": "A", "cmd": True})
    d.type("4")
    d.key("Escape")
    if len(d.execute("sketch.inspect")["curves"]) != 9:
        raise RuntimeError("the offset did not add four curves")


def notch_circle(d):
    d.tool("sketch.circle.center")
    d.click(d.at(sketch=[NOTCH[0], NOTCH[1]]), dur=0.2, settle=0.3)
    d.glide(d.at(sketch=[NOTCH[0] + 7, NOTCH[1] - 4]), 0.3)
    d.type(str(NOTCH[2]), enter=False)
    d.click(d.pos, dur=0.1, settle=0.4)
    d.key("Escape")


def offset_and_notch(d):
    drawn(d)
    dimension_hole(d)
    offset_outline(d)
    notch_circle(d)
    time.sleep(6)  # the "Dimension applied" notice fades


def dimensioned(d):
    drawn(d)
    dimension_hole(d)
    time.sleep(6)


@shot("sketching/offset", "clip", setup=dimensioned)
def sketching_offset(d):
    """Offset the rectangle's outline 4 mm inward: one click picks the whole chain."""
    offset_outline(d, d.caption)
    d.wait(1.5)


@shot("sketching/trim", "clip", setup=offset_and_notch)
def sketching_trim(d):
    """Trim a notch: click the piece of the top side inside the circle, then the arc above it."""
    d.caption("Trim: click each piece to remove")
    d.tool("sketch.trim")
    r = NOTCH[2] / 2
    before = len(d.execute("sketch.inspect")["curves"])
    d.click(d.at(sketch=[NOTCH[0] - r / 2, NOTCH[1]]), dur=0.6, settle=0.8)
    d.click(d.at(sketch=[NOTCH[0], NOTCH[1] + r]), dur=0.6, settle=0.8)
    d.key("Escape")
    if len(d.execute("sketch.inspect")["curves"]) == before:
        raise RuntimeError("Trim removed nothing")
    d.caption("")
    d.glide((SIZE[0] * 0.55, SIZE[1] * 0.85), 0.6)
    d.wait(1.5)


@shot("sketching/parts", "still", setup=drawn)
def sketching_parts(d):
    """The finished sketch, numbered: 1 the Sketch tab's tools, 2 a constraint, 3 a dimension,
    4 the sketch's state, 5 Finish Sketch."""
    dimension_hole(d)
    d.ui("ui.move", {"x": 5, "y": 600})
    time.sleep(6)  # the "Dimension applied" notice fades
    sel = d.ui("ui.selection")
    dims = sel.get("dimensions") or []
    if not dims:
        raise RuntimeError("no dimensions drawn")
    names = {h["name"]: (h["x"], h["y"]) for h in d.ui("ui.handles")}
    left = d.at(sketch=[0, RECT[1] / 2])
    d.callouts += [
        (names["toolbar:sketch.line"][0] - 2, names["toolbar:sketch.line"][1] + 56),
        (left[0] - 34, left[1] + 26),
        (dims[0]["x"], dims[0]["y"] - 30),
        (names["sketch_palette:status_header"][0] - 128, names["sketch_palette:status_header"][1] + 8),
        (names["sketch_palette:finish"][0] - 130, names["sketch_palette:finish"][1]),
    ]


# features.md: the sketch above made solid, hollowed, rounded and edited from the timeline. Shell
# comes before Fillet: a shell through rounded edges needs the kernel's boolean retry (a warning
# on the timeline), and a 1.5 mm round on the 2 mm wall's outer edge needs nothing.
HEIGHT = 12
TOP_EDGES = [[RECT[0] / 2, 0, HEIGHT], [RECT[0], RECT[1] / 2, HEIGHT], [RECT[0] / 2, RECT[1], HEIGHT], [0, RECT[1] / 2, HEIGHT]]
TOP = [RECT[0] * 0.75, RECT[1] * 0.75, HEIGHT]


def model_view(d):
    """The same three-quarter view for every feature clip, the part left of the dialog."""
    d.ui("ui.camera", {"target": [RECT[0] / 2 + 6, RECT[1] / 2, HEIGHT / 2], "yaw": -0.7854, "pitch": 0.6155, "distance": 125})
    d.settle_camera()


def features(d):
    return [h["name"].split(":", 1)[1] for h in d.ui("ui.handles") if h["name"].startswith("timeline:")]


def wait_feature(d, name):
    """Wait for `name` on the timeline (a shell can take a few seconds to build), and fail on a
    feature with a warning or an error: a figure must not show one."""
    for _ in range(300):
        if name in features(d):
            d.settle_camera()
            r = d.execute("timeline.inspect", {"feature": name})
            if r.get("warning") or r.get("error"):
                raise RuntimeError(f"{name}: {r.get('warning') or r.get('error')}")
            return
        time.sleep(0.1)
    raise RuntimeError(f"no {name} on the timeline: {features(d)}")


def typed(d, value):
    """Replace the focused value and press Enter (Enter is OK in a feature dialog)."""
    d.wait(0.4)
    d.ui("ui.key", {"key": "A", "cmd": True})
    d.type(value)


def extrude(d, label=lambda text: None):
    label("Extrude: click the profile")
    d.tool("solid.extrude")
    d.click(d.at(world=[RECT[0] * 0.8, RECT[1] * 0.8, 0]), dur=0.6, settle=0.8)
    label("Drag the arrow, or type the distance: 12, Enter")
    a = d.at(handle="arrow")
    d.drag(a, (a[0], a[1] - 50), steps=40)
    typed(d, str(HEIGHT))
    wait_feature(d, "Extrude1")


def shell(d, label=lambda text: None):
    label("Shell: click the face to remove")
    d.tool("solid.shell")
    d.click(d.at(world=TOP), dur=0.6, settle=0.8)
    label("Wall thickness: 2, Enter")
    typed(d, "2")
    wait_feature(d, "Shell1")


def fillet(d, label=lambda text: None):
    label("Fillet: click the edges to round")
    d.tool("solid.fillet")
    for edge in TOP_EDGES:
        d.click(d.at(world=edge), dur=0.4, settle=0.3)
    label("Radius: 1.5, Enter")
    typed(d, "1.5")
    wait_feature(d, "Fillet1")


HOLE_AT = [32, 20, 2]  # on the box's floor, beside the tube
TUBE_RIM = [HOLE[0] - HOLE[2] / 2, HOLE[1], HEIGHT]  # on the tube's top circle: the pattern's axis


def hole(d, label=lambda text: None):
    label("Hole: click where it goes, set the diameter, OK")
    d.tool("solid.hole")
    d.click(d.at(world=HOLE_AT), dur=0.6, settle=1.0)
    d.click(d.at(handle="dialog:Diameter"), dur=0.6, settle=0.3)
    d.ui("ui.key", {"key": "A", "cmd": True})
    d.type("4", enter=False)
    d.click(d.at(handle="dialog:ok"), dur=0.6, settle=1.0)
    wait_feature(d, "Hole1")


def pattern(d, label=lambda text: None):
    label("Circular Pattern: Features, pick Hole1 on the timeline")
    d.tool("solid.pattern.circular", panel="panel:MODEL:ARRANGE")
    d.click(d.at(handle="dialog:Object Type"), dur=0.6, settle=0.5)
    d.click(d.at(handle="dialog:Object Type:Features"), dur=0.4, settle=0.5)
    d.click(d.at(handle="timeline:Hole1"), dur=0.6, settle=0.6)
    label("Axis: click the tube's rim. Quantity: 4, Enter")
    d.click(d.at(handle="dialog:Axis"), dur=0.6, settle=0.4)
    d.click(d.at(world=TUBE_RIM), dur=0.6, settle=1.0)
    typed(d, "4")
    wait_feature(d, "CircularPattern1")


def sketched(d):
    drawn(d)
    dimension_hole(d)
    d.tool("sketch.finish")
    model_view(d)
    time.sleep(6)  # the "Dimension applied" notice fades


def extruded(d):
    sketched(d)
    extrude(d)


def shelled(d):
    extruded(d)
    shell(d)


def filleted(d):
    shelled(d)
    fillet(d)


def holed(d):
    filleted(d)
    hole(d)


def patterned(d):
    holed(d)
    pattern(d)


@shot("features/extrude", "clip", setup=sketched)
def features_extrude(d):
    """Extrude the sketch's profile: drag the arrow, type 12, Enter."""
    extrude(d, d.caption)
    d.wait(1.5)


@shot("features/shell", "clip", setup=extruded)
def features_shell(d):
    """Shell: pick the top face to open, keep the 2 mm wall, Enter; then a turn to look in."""
    shell(d, d.caption)
    d.caption("")
    d.orbit(0, 60, steps=40)
    d.wait(1.5)


@shot("features/fillet", "clip", setup=shelled)
def features_fillet(d):
    """Fillet the four outer top edges: pick them, type 1.5, Enter."""
    fillet(d, d.caption)
    d.wait(1.5)


@shot("features/hole", "clip", setup=filleted)
def features_hole(d):
    """A 4 mm hole in the floor beside the tube."""
    hole(d, d.caption)
    d.wait(1.5)


@shot("features/pattern", "clip", setup=holed)
def features_pattern(d):
    """The hole repeated four times around the tube."""
    pattern(d, d.caption)
    d.wait(1.5)


@shot("parameters/dialog", "still", setup=patterned)
def parameters_dialog(d):
    """Change Parameters, numbered: 1 apply as you go, 2 user parameters, 3 model parameters
    grouped by feature, 4 an expression, 5 its value."""
    d.tool("parameters.change")
    d.wait(1.0)
    d.ui("ui.move", {"x": 5, "y": 600})
    n = {h["name"]: (h["x"], h["y"]) for h in d.ui("ui.handles")}
    x0, y0 = n["dialog:title"][0] - 400, n["dialog:title"][1] - 22
    d.crop = (int(x0), int(y0), 800, 500)
    # Wide rows' handles are their middles: each number goes in the gap beside its item, measured
    # from the rows it belongs to.
    d.callouts += [
        (x0 + 290, n["params:apply_live"][1]),
        (x0 + 165, n["dialog:table:user:add"][1]),
        (x0 + 165, n["dialog:section:Sketch1"][1]),
        (n["dialog:Expression"][0] - 95, n["dialog:cell:d1"][1]),
        (n["dialog:report:value:d1"][0] + 30, n["dialog:report:value:d1"][1] - 32),
    ]


@shot("features/edit", "clip", setup=patterned)
def features_edit(d):
    """Double-click Extrude1 on the timeline, make it 20 mm: everything after it follows."""
    d.caption("Double-click a feature on the timeline")
    t = d.at(handle="timeline:Extrude1")
    d.glide(t, 0.8)
    d.ui("ui.click", {"x": t[0], "y": t[1], "double": True})
    d.mark(cursor=list(t), click=True)
    d.wait(1.2)
    d.settle_camera()
    d.caption("Change it: 20, Enter. The features after it follow")
    typed(d, "20")
    for name in ("Extrude1", "Shell1", "Fillet1", "Hole1", "CircularPattern1"):
        wait_feature(d, name)
    if features(d)[-1:] != ["CircularPattern1"]:
        raise RuntimeError(f"the timeline did not rebuild: {features(d)}")
    # Off the timeline (a feature under the pointer lights up its body), nothing selected.
    d.glide((SIZE[0] * 0.55, SIZE[1] * 0.75), 0.8)
    for _ in range(10):
        d.key("Escape")
        if not d.ui("ui.selection")["selection"]:
            break
        time.sleep(0.5)
    d.wait(2.0)


# assemblies.md: two loose hinge leaves made into components, joined by a revolute joint on
# their barrels, then swung by a drag. Leaf B starts lifted and set back, so the joint visibly
# brings it into place.
B_LIFT = (25, 20)  # mm along Y and Z


def hinge_view(d):
    d.ui("ui.camera", {"target": [26, 40, 8], "yaw": -0.7854, "pitch": 0.6155, "distance": 150})
    d.settle_camera()


def loose_leaves(d):
    """Leaf A (its barrel on the far side) and Leaf B (its barrel on the near side), as bodies."""
    dy, dz = B_LIFT
    d.execute("solid.box", {"length": 40, "width": 30, "height": 3, "corner": [0, 0, 0], "body_name": "Leaf A"})
    d.execute("solid.cylinder", {"base": [0, 33, 4], "axis": [1, 0, 0], "radius": 4, "height": 40, "operation": "join", "targets": ["Leaf A"]})
    d.execute("solid.box", {"length": 40, "width": 30, "height": 3, "corner": [0, 36 + dy, dz], "body_name": "Leaf B"})
    d.execute("solid.cylinder", {"base": [0, 33 + dy, 4 + dz], "axis": [1, 0, 0], "radius": 4, "height": 40, "operation": "join", "targets": ["Leaf B"]})
    hinge_view(d)


def components(d, label=lambda text: None):
    label("Right-click Bodies: Create Components from Bodies")
    b = d.at(handle="row:Bodies")
    d.glide(b, 0.8)
    d.ui("ui.click", {"x": b[0], "y": b[1], "button": "right"})
    d.mark(cursor=list(b), click=True)
    d.wait(0.8)
    d.click(d.at(handle="menu:component.from_bodies"), dur=0.6, settle=1.0)
    rows = {h["name"] for h in d.ui("ui.handles") if h["name"].startswith("row:")}
    if not {"row:Leaf A:1", "row:Leaf B:1"} <= rows:
        raise RuntimeError(f"no components: {sorted(rows)}")


def leaves(d):
    loose_leaves(d)
    components(d)


def joint(d, label=lambda text: None):
    label("Joint: click Leaf B's barrel end, then Leaf A's")
    d.click(d.at(handle="tab:ASSEMBLE"), dur=0.6, settle=0.5)
    d.tool("joint.create")
    dy, dz = B_LIFT
    d.click(d.at(world=[40, 33 + dy, 8 + dz]), dur=0.6, settle=0.8)
    d.click(d.at(world=[40, 33, 8]), dur=0.6, settle=1.0)
    label("Type: Revolute. Flip turns Leaf B the right way round")
    d.click(d.at(handle="dialog:Type"), dur=0.5, settle=0.6)
    d.click(d.at(handle="dialog:Type:Revolute"), dur=0.4, settle=0.8)
    d.click(d.at(handle="dialog:Flip"), dur=0.6, settle=1.2)
    d.click(d.at(handle="dialog:ok"), dur=0.6, settle=1.5)
    if "row:Joint1" not in {h["name"] for h in d.ui("ui.handles")}:
        raise RuntimeError("no Joint1 in the browser")


def joined(d):
    leaves(d)
    joint(d)


@shot("assemblies/components", "clip", setup=loose_leaves)
def assemblies_components(d):
    """Two bodies become two components; the first is grounded (the pin)."""
    components(d, d.caption)
    d.caption("Each body is a component now; the first is grounded (the pin)")
    d.glide(d.at(handle="row:Leaf A:1"), 0.8)
    d.wait(2.0)


@shot("assemblies/joint", "clip", setup=leaves)
def assemblies_joint(d):
    """A revolute joint on the two barrels: Leaf B drops into place beside Leaf A."""
    joint(d, d.caption)
    d.caption("")
    d.glide((SIZE[0] * 0.55, SIZE[1] * 0.8), 0.6)
    d.wait(1.5)


@shot("assemblies/drag", "clip", setup=joined)
def assemblies_drag(d):
    """Drag Leaf B: it turns about the joint, and grounded Leaf A stays put."""
    d.caption("Drag Leaf B: it can only turn about the hinge")
    p = d.at(world=[30, 55, 3])
    d.drag(p, (p[0] + 10, p[1] - 170), steps=70)
    d.wait(0.8)
    d.drag(d.pos, (d.pos[0] - 5, d.pos[1] + 90), steps=50)
    d.wait(0.8)
    d.caption("Leaf A is grounded: it stays where it is")
    p = d.at(world=[20, 10, 3])
    d.drag(p, (p[0] - 80, p[1] + 40), steps=40)
    d.wait(1.5)


@shot("assemblies/drive", "clip", setup=joined)
def assemblies_drive(d):
    """Drive Joints: the slider turns the hinge through its angle, OK keeps it there."""
    d.caption("Drive Joints: drag the slider, or type the angle")
    d.tool("joint.drive")
    s = d.at(handle="drive_slider:0")
    d.drag(s, (s[0] - 80, s[1]), steps=60)
    d.wait(0.6)
    d.drag(d.pos, (d.pos[0] + 45, d.pos[1]), steps=40)
    d.wait(0.6)
    d.click(d.at(handle="dialog:ok"), dur=0.6, settle=1.0)
    d.caption("")
    d.glide((SIZE[0] * 0.55, SIZE[1] * 0.85), 0.6)
    d.wait(1.5)


# getting-started.md: the start page, and a sample opened from it.


def start_page(d):
    d.ui("ui.home", {"open": True})
    d.wait(2.0)
    d.ui("ui.move", {"x": 700, "y": 790})
    d.settle_camera()


@shot("getting-started/start", "still", setup=start_page)
def getting_started_start(d):
    """The start page, numbered: 1 new design, 2 open, 3 import, 4 recent designs, 5 samples,
    6 the File menu."""
    n = {h["name"]: (h["x"], h["y"]) for h in d.ui("ui.handles")}
    left = n["home:new_mm"][0] - 154  # the tiles' left edge
    d.callouts += [
        (n["home:new_mm"][0] + 128, n["home:new_mm"][1] - 18),
        (n["home:open_tile"][0] + 112, n["home:open_tile"][1] - 18),
        (n["home:import"][0] + 128, n["home:import"][1] - 18),
        (left + 105, n["home:import"][1] + 81),
        (left + 105, n["home:sample:0"][1] - 112),
        (n["home:recover"][0] + 100, n["home:recover"][1] + 32),
    ]


@shot("getting-started/sample", "clip", setup=start_page)
def getting_started_sample(d):
    """Open a sample from the start page and turn it to look round."""
    d.caption("Click a sample to open it")
    d.click(d.at(handle="home:sample:0"), dur=0.8, settle=2.0)
    for _ in range(50):
        if not d.ui("ui.home").get("open"):
            break
        time.sleep(0.2)
    else:
        raise RuntimeError("the sample did not open")
    d.settle_camera()
    d.caption("")
    d.wait(0.8)
    d.orbit(220, 0, steps=60)
    d.wait(1.5)


# sheet-metal.md: a tray: a base flange from a sketched rectangle, two edge flanges, flat pattern.
PLATE = (80, 50)


def sheet_view(d):
    d.ui("ui.camera", {"target": [PLATE[0] / 2, PLATE[1] / 2, 8], "yaw": -0.7854, "pitch": 0.6155, "distance": 170})
    d.settle_camera()


def plate_sketch(d):
    d.execute("sketch.create", {"plane": "XY"})
    d.execute("sketch.rectangle.two_point", {"p0": [0, 0], "p1": list(PLATE)})
    d.execute("sketch.finish")
    sheet_view(d)
    d.click(d.at(handle="tab:SHEET METAL"), dur=0.2, settle=0.6)


def base_flange(d, label=lambda text: None):
    label("Flange: click the sketch's profile, OK")
    d.tool("sheet.flange")
    d.click(d.at(world=[PLATE[0] * 0.75, PLATE[1] * 0.8, 0]), dur=0.6, settle=1.0)
    d.click(d.at(handle="dialog:ok"), dur=0.6, settle=1.0)
    wait_feature(d, "BaseFlange1")


def edge_flanges(d, label=lambda text: None):
    label("Flange again: click two top edges, height 20, Enter")
    d.tool("sheet.flange")
    top = 2.5  # the default rule's thickness
    for y in (0, PLATE[1]):
        d.click(d.at(world=[PLATE[0] / 2, y, top]), dur=0.6, settle=0.8)
    typed(d, "20")
    wait_feature(d, "EdgeFlange1")


def base(d):
    plate_sketch(d)
    base_flange(d)


def tray(d):
    base(d)
    edge_flanges(d)


@shot("sheet-metal/base", "clip", setup=plate_sketch)
def sheet_metal_base(d):
    """A base flange from a sketched rectangle, at the rule's thickness."""
    base_flange(d, d.caption)
    d.wait(1.5)


@shot("sheet-metal/flange", "clip", setup=base)
def sheet_metal_flange(d):
    """Edge flanges up from two top edges, bent at the rule's radius."""
    edge_flanges(d, d.caption)
    d.wait(1.5)


@shot("sheet-metal/flat", "clip", setup=tray)
def sheet_metal_flat(d):
    """Flat Pattern: the part unfolded with its bend lines and bend table."""
    d.caption("Flat Pattern: the part unfolded, with its bends")
    d.tool("sheet.flat_pattern")
    d.wait(2.5)
    d.settle_camera()
    d.click(d.at(handle="dialog:cancel"), dur=0.8, settle=1.0)
    d.caption("")
    d.wait(1.0)


# files.md: the File menu, where opening, inserting, saving and exporting start.


@shot("files/menu", "still", start="sample")
def files_menu(d):
    """The File menu, numbered: 1 open, 2 insert into this design, 3 save, 4 export, 5 design
    properties."""
    d.click(d.at(handle="appbar:file"), dur=0.2, settle=0.8)
    d.ui("ui.move", {"x": 700, "y": 790})
    d.settle_camera()
    n = {h["name"]: (h["x"], h["y"]) for h in d.ui("ui.handles")}
    right = n["menu:Open…"][0] + 135  # past the shortcuts, inside the menu's right edge
    d.crop = (0, 0, 640, 800)
    d.callouts += [
        (right + 40, n["menu:Open…"][1]),
        (right + 40, (n["menu:Insert STEP…"][1] + n["menu:Insert Design…"][1]) / 2),
        (right + 40, (n["menu:Save"][1] + n["menu:Save As JSON…"][1]) / 2),
        (right + 40, (n["menu:Export STL…"][1] + n["menu:Export Design as JSON…"][1]) / 2),
        (right + 40, n["menu:Design Properties…"][1]),
    ]


# scan-clean.md: the CLEAN panel of the Scan workspace, one clip per tool, on openly licensed
# scans (credited in ATTRIBUTION.md). The scans are not in the repository: SCAN_CORPUS points at a
# folder laid out as below (default `scan-corpus` in the current folder); each path says where it comes from.
CORPUS = Path(os.environ.get("SCAN_CORPUS") or "scan-corpus")
# YCB 025 mug, Berkeley TSDF mesh (CC BY 4.0, ycbbenchmarks.com): unwelded, with holes and specks.
MUG = CORPUS / "ycb/025_mug/tsdf/nontextured.stl"
# Stanford bunny, bun_zipper.ply (Stanford Computer Graphics Laboratory): five holes in its base.
BUNNY = CORPUS / "stanford/bunny/bunny/reconstruction/bun_zipper.ply"
# The same at half resolution: duplicate triangles and edges of three faces for Repair Mesh.
BUNNY_RES2 = CORPUS / "stanford/bunny/bunny/reconstruction/bun_zipper_res2.ply"
# T-LESS object 5, Kinect fusion reconstruction (CC BY 4.0, Hodan et al.): a mirror-symmetric part.
TLESS5 = CORPUS / "tless/models_reconst/obj_000005.ply"
# Smithsonian Open Access scans (CC0), in metres.
SMITHSONIAN = CORPUS / "smithsonian"
HELMET = Path(os.environ.get("HELMET_OBJ") or SMITHSONIAN / "french_adrian_helmet/2017-111-9_FrenchAdrianHelmet-RENDER.obj")
CANTEEN = SMITHSONIAN / "french_army_canteen/2017_111_17_FrenchArmyCanteen_RENDER.obj"
OPENER = SMITHSONIAN / "bully_beef_can_opener/2017-111-7-2_BullyBeefCanOpener_RENDER.obj"
# YCB (CC BY 4.0), in metres.
HAMMER = CORPUS / "ycb/048_hammer/tsdf/nontextured.ply"
# The Stanford bunny's first raw scanner pass, bun000.ply: one side of it, as points.
BUNNY_PASS = CORPUS / "stanford/bunny/bunny/data/bun000.ply"


def scan_open(path, units="mm", camera=None, upright=False, then=(), style=None, origin=True):
    """A setup: insert the scan centred on the origin (`upright`: a Y-up file turned Z-up), run
    the `then` steps (commands whose "{body}" is the scan), open the Scan workspace and frame it."""

    def setup(d):
        if not path.is_file():
            raise RuntimeError(f"{path}: not found (set SCAN_CORPUS to the scan corpus folder)")
        made = d.execute("file.insert_mesh", {"path": str(path), "units": units, "origin": origin})
        # A file's own object name, where it has one (an OBJ's), else the file's.
        body = ((made or {}).get("bodies") or [path.stem])[0]
        d.scan = body
        if upright:
            d.execute("solid.move", {"bodies": [body], "axis": [1, 0, 0], "angle": "90 deg"})
            d.execute("scan.rest_on_grid", {"body": body})
        for command, params in then:
            d.execute(command, json.loads(json.dumps(params).replace("{body}", body)))
        if style:
            d.execute("view.visual_style", {"style": style})
        d.ui("ui.workspace", {"workspace": "scan"})
        d.ui("ui.view", {"view": "iso"})
        d.ui("ui.view", {"view": "fit"})
        if camera:
            d.ui("ui.camera", camera)
        # The "Inserted" note fades before the clip starts.
        d.wait(4.0)
        d.settle_camera()

    return setup


def dialog_ready(d, timeout=180):
    """Wait while the open dialog is still working out what OK will do."""
    for _ in range(timeout * 4):
        dlg = d.ui("ui.inspect").get("dialog")
        if isinstance(dlg, dict) and not dlg.get("waiting") and dlg.get("commands") is not None:
            d.settle_camera()
            return dlg
        time.sleep(0.25)
    raise RuntimeError(f"the dialog never became ready: {dlg}")


def scan_triangles_on(d):
    """Scan Triangles on, from the Check panel, so the triangles show."""
    d.caption("Scan Triangles (Check panel) shows the scan's triangles")
    d.tool("view.scan_triangles", panel="panel:SCAN:CHECK")
    d.settle_camera()
    d.wait(1.5)


def ok_and_wait(d, hold=2.5):
    """Click OK and wait until the dialog has closed and its work is in the design."""
    d.click(d.at(handle="dialog:ok"), dur=0.6, settle=0.5)
    for _ in range(1200):
        if not d.ui("ui.inspect").get("dialog"):
            break
        time.sleep(0.25)
    d.settle_camera()
    d.wait(hold)


@shot("scan-clean/survey", "clip", setup=scan_open(MUG, "m"))
def scan_survey(d):
    """Survey Scan on the YCB mug: size, noise, holes and loose pieces, then the next steps."""
    d.caption("Survey Scan reads the scan without changing it")
    d.tool("scan.survey")
    d.wait(4.0)
    d.settle_camera()
    d.caption("Its size, noise, holes, loose pieces and edges to repair")
    d.wait(2.5)
    card = d.at(handle="dialog:card")
    d.glide(card, 0.6)
    d.caption("Below them, the next steps: each button opens its tool")
    d.ui("ui.scroll", {"x": card[0], "y": card[1], "delta": -400})
    d.settle_camera()
    d.wait(3.0)


@shot("scan-clean/clean-up", "clip", setup=scan_open(MUG, "m"))
def scan_clean_up(d):
    """Clean Up on the YCB mug: repair, floating pieces and holes in one go, each its own step."""
    d.caption("Clean Up: Repair, Remove Floating Pieces and Fill Holes in one go")
    d.tool("scan.clean_up")
    dialog_ready(d)
    d.wait(1.5)
    d.caption("OK: holes closed, specks gone, each tool a step on the timeline")
    ok_and_wait(d, hold=3.0)


@shot("scan-clean/repair", "clip", setup=scan_open(MUG, "m", camera={"distance": 230}))
def scan_repair(d):
    """Repair Mesh on the YCB mug: its file wrote points twice a hair apart, leaving black cracks."""
    d.caption("Repair Mesh is in the Clean drop-down")
    d.tool("scan.repair", panel="panel:SCAN:CLEAN")
    dialog_ready(d)
    d.wait(1.5)
    d.caption("OK: points a hair apart are joined and the cracks close")
    ok_and_wait(d, hold=3.0)


@shot("scan-clean/floating", "clip", setup=scan_open(MUG, "m"))
def scan_floating(d):
    """Remove Floating Pieces on the YCB mug: the specks apart from it are found and marked."""
    d.caption("Floating: the pieces not joined to the part are marked")
    d.tool("scan.remove_floating")
    dialog_ready(d)
    d.wait(2.5)
    d.caption("Click a piece to keep it, or to take it off. OK removes the marked ones")
    ok_and_wait(d, hold=2.5)


@shot("scan-clean/fill-holes", "clip", setup=scan_open(BUNNY, "m", upright=True, camera={"pitch": -0.45}))
def scan_fill_holes(d):
    """Fill Holes on the Stanford bunny, seen from below: its base's holes ticked and filled."""
    d.caption("Fill Holes: each hole is ringed and ticked; click one to leave it open")
    d.tool("scan.fill_holes")
    dialog_ready(d)
    d.wait(2.5)
    d.caption("OK: smooth patches close them")
    ok_and_wait(d, hold=3.0)


@shot("scan-clean/reduce", "clip", setup=scan_open(TLESS5))
def scan_reduce(d):
    """Reduce Mesh on T-LESS object 5, its triangles drawn: the dialog counts them now and kept."""
    scan_triangles_on(d)
    d.caption("Reduce Mesh: fewer triangles, the surface moved as little as possible")
    d.tool("scan.reduce", panel="panel:SCAN:CLEAN")
    dialog_ready(d)
    d.glide(d.at(handle="dialog:card"), 0.6)
    d.caption("The dialog says how many triangles there are now and how many stay")
    d.wait(3.0)
    d.caption("OK: the whole scan, or only the area chosen in the dialog")
    ok_and_wait(d, hold=3.0)


@shot("scan-clean/denoise", "clip", setup=scan_open(TLESS5, camera={"distance": 120}))
def scan_denoise(d):
    """Denoise on T-LESS object 5: the Kinect's ripples smoothed, its edges kept sharp."""
    d.caption("Denoise reads the scan's noise and tries each strength of the filter")
    d.tool("scan.denoise", panel="panel:SCAN:CLEAN")
    dialog_ready(d)
    d.wait(2.0)
    d.caption("OK: the strength with the least error; sharp edges stay sharp")
    ok_and_wait(d, hold=3.0)


@shot("scan-clean/remesh", "clip", setup=scan_open(TLESS5, camera={"distance": 160}))
def scan_remesh(d):
    """Remesh on T-LESS object 5, its triangles drawn: one even surface at the cell size asked."""
    scan_triangles_on(d)
    d.caption("Remesh makes the scan again as even triangles")
    d.tool("scan.remesh", panel="panel:SCAN:CLEAN")
    dialog_ready(d)
    d.wait(2.0)
    d.caption("OK: every triangle about the same size, the ripples averaged out")
    ok_and_wait(d, hold=3.0)


@shot("scan-clean/select", "clip", setup=scan_open(BUNNY, "m", upright=True))
def scan_select(d):
    """Select on Scan on the bunny: a brush stroke over its back, then Grow."""
    d.caption("Select on Scan: brush, polygon, lasso, region, shape or a curve")
    d.tool("scan.select")
    d.settle_camera()
    measured = d.ui("engine.execute", {"command": "inspect.measure", "params": {"bodies": [BUNNY.stem]}})["bodies"]
    box = next(m["bbox"] for m in measured if m["name"] == BUNNY.stem)
    lo, hi = box["min"], box["max"]

    def on(fx, fz):
        return [lo[0] + (hi[0] - lo[0]) * fx, (lo[1] + hi[1]) / 2, lo[2] + (hi[2] - lo[2]) * fz]

    # A brush 10 mm across, along the bunny's side from its tail to its neck.
    d.click(d.at(handle="dialog:length:Radius"), dur=0.4, settle=0.2)
    d.ui("ui.key", {"key": "A", "cmd": True})
    d.ui("ui.text", {"text": "10"})
    d.wait(0.3)
    d.key("Tab")
    a, b = d.ui("ui.worldToScreen", {"points": [on(0.8, 0.35), on(0.3, 0.6)]})
    d.caption("Drag the brush over the scan: what it passes over is painted")
    d.drag(tuple(a), tuple(b), steps=60)
    d.wait(1.0)
    d.caption("Grow, Shrink or Invert it; every scan tool can work on it")
    d.click(d.at(handle="dialog:button::Grow"), dur=0.6, settle=0.6)
    d.click(d.at(handle="dialog:button::Grow"), dur=0.2, settle=0.6)
    d.wait(2.0)


@shot("scan-clean/split", "clip", setup=scan_open(BUNNY, "m", upright=True))
def scan_split(d):
    """Split Mesh on the bunny along the origin's YZ plane, both sides kept."""
    d.caption("Split Mesh: pick what cuts the scan")
    d.tool("scan.split", panel="panel:SCAN:CLEAN")
    d.settle_camera()
    d.click(d.at(handle="fold:Origin"), dur=0.6, settle=0.4)
    d.click(d.at(handle="row:YZ"), dur=0.5, settle=0.4)
    dialog_ready(d)
    d.caption("Each side is painted before OK; keep both, one, or only the cut")
    d.wait(2.5)
    ok_and_wait(d, hold=2.5)


# T-LESS 5 mirrors in YZ and in XZ. Its side past X = 15 mm is cut away, as if never scanned: the
# guide says which plane is meant.
ONE_SIDED = [("scan.split", {"body": "{body}", "plane": {"origin": [15, 0, 0], "normal": [1, 0, 0]}, "keep": "back"})]


@shot("scan-clean/mirror", "clip", setup=scan_open(TLESS5, then=ONE_SIDED))
def scan_mirror(d):
    """Complete by Mirroring on T-LESS object 5 with one side cut away: the origin's YZ plane, its
    mirror plane, picked in the browser and that side added back."""
    d.caption("Complete by Mirroring: for a part with one end never scanned")
    d.tool("scan.complete_by_mirroring", panel="panel:SCAN:CLEAN")
    dialog_ready(d)
    d.caption("The plane across the missing end is found; amber shows what OK adds")
    d.wait(3.0)
    d.caption("OK: the mirror image fills where the scan has nothing")
    ok_and_wait(d, hold=3.0)


def cloud_open(d):
    if not BUNNY_PASS.is_file():
        raise RuntimeError(f"{BUNNY_PASS}: not found (set SCAN_CORPUS to the scan corpus folder)")
    d.execute("pointcloud.insert", {"path": str(BUNNY_PASS), "units": "m"})
    d.execute("pointcloud.edit", {"cloud": BUNNY_PASS.stem, "color_by": "single", "color": [0.8, 0.82, 0.86]})
    d.ui("ui.workspace", {"workspace": "scan"})
    # The pass is Y-up: seen from above, the bunny stands upright on the screen.
    d.ui("ui.view", {"view": "top"})
    d.ui("ui.view", {"view": "fit"})
    d.wait(4.0)
    d.settle_camera()


@shot("scan-clean/points", "clip", setup=cloud_open)
def scan_points(d):
    """Mesh from Points on one raw scanner pass of the Stanford bunny."""
    d.caption("Mesh from Points: a scan that came as points only")
    d.tool("scan.mesh_from_points", panel="panel:SCAN:CLEAN")
    dialog_ready(d)
    d.wait(1.5)
    d.caption("OK: a mesh body, stopping where the points are sparse")
    ok_and_wait(d, hold=1.0)
    d.execute("pointcloud.edit", {"cloud": BUNNY_PASS.stem, "visible": False})
    d.caption("The cloud hidden: the mesh made from it")
    d.wait(3.0)


# scan-align.md: the ALIGN panel. T-LESS object 5 arrives turned and lifted, as a scan does in
# its scanner's frame; two raw passes of the Stanford bunny (bun000, bun045) are laid together.
TILTED = [("solid.move", {"bodies": ["{body}"], "axis": [0.3, 1.0, 0.2], "angle": "35 deg", "translate": [20, 10, 25]})]
# Upright, but lifted off the grid and away from the origin: what Rest and Center put right.
LIFTED = [("solid.move", {"bodies": ["{body}"], "translate": [45, -30, 25]})]
# Upright, turned about Z: its mirror plane stands upright but runs at an angle.
TURNED = [("solid.move", {"bodies": ["{body}"], "axis": [0, 0, 1], "angle": "28 deg"})]
PASSES = [CORPUS / "stanford/bunny/bunny/data/bun000.ply", CORPUS / "stanford/bunny/bunny/data/bun045.ply"]


def passes_open(d):
    for path in PASSES:
        if not path.is_file():
            raise RuntimeError(f"{path}: not found (set SCAN_CORPUS to the scan corpus folder)")
        d.execute("file.insert_mesh", {"path": str(path), "units": "m"})
    # The second pass in another colour, to see the two come together.
    d.execute("appearance.assign", {"bodies": [PASSES[1].stem], "color": "#e8542c"})
    d.ui("ui.workspace", {"workspace": "scan"})
    d.ui("ui.view", {"view": "iso"})
    d.ui("ui.view", {"view": "fit"})
    d.wait(4.0)
    d.settle_camera()


@shot("scan-align/stand-on-base", "clip", setup=scan_open(TLESS5, then=TILTED))
def scan_stand_on_base(d):
    """Stand on Base on T-LESS 5 lying at an angle: its steadiest base down, then the next one."""
    d.caption("Stand on Base: the scan stands on its steadiest base, on the grid")
    d.tool("scan.stand_on_base")
    d.settle_camera()
    dialog_ready(d)
    d.wait(1.5)
    ok_and_wait(d, hold=2.0)
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()
    d.caption("Press again for the next base")
    d.tool("scan.stand_on_base")
    dialog_ready(d)
    ok_and_wait(d, hold=2.5)


@shot("scan-align/quick-align", "clip", setup=scan_open(TLESS5, then=TILTED))
def scan_quick_align(d):
    """Align, Quick Align on T-LESS 5 at an angle: longest along X, thinnest along Z, about the origin."""
    d.caption("Align: choose how the scan is laid")
    d.tool("scan.align")
    d.settle_camera()
    d.wait(1.5)
    d.caption("Quick Align: longest along X, thinnest along Z, its middle at the origin")
    d.click(d.at(handle="dialog:button::Quick Align"), dur=0.6, settle=0.6)
    d.settle_camera()
    if d.ui("ui.inspect").get("dialog"):
        dialog_ready(d)
        ok_and_wait(d, hold=1.0)
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()
    d.wait(2.5)


@shot("scan-align/rest-and-center", "clip", setup=scan_open(TLESS5, then=LIFTED, camera={"target": [20, -15, 25], "distance": 380}))
def scan_rest_and_center(d):
    """Rest on Grid then Center on Origin, from the Align panel's drop-down: the part, upright but
    lifted and off to one side, put down on the grid and over the origin."""
    d.caption("Rest on Grid: down until its lowest point is on the grid")
    d.tool("scan.rest_on_grid", panel="panel:SCAN:ALIGN")
    d.settle_camera()
    d.wait(2.0)
    d.caption("Center on Origin: its middle over the origin, its height kept")
    d.tool("scan.center", panel="panel:SCAN:ALIGN")
    d.settle_camera()
    d.wait(2.5)


@shot("scan-align/align-to-scan", "clip", setup=passes_open)
def scan_align_to_scan(d):
    """Align to Scan: the bunny's second pass, scanned from 45° round, laid onto the first."""
    d.caption("Two passes of one part, each where its scanner saw it")
    d.wait(1.5)
    d.tool("scan.align")
    d.settle_camera()
    d.caption("Align to Scan lays one pass onto the other")
    d.click(d.at(handle="dialog:button::Align to Scan"), dur=0.6, settle=0.6)
    d.settle_camera()
    dialog_ready(d)
    d.wait(2.0)
    d.caption("OK: the passes lie together as one part")
    ok_and_wait(d, hold=1.0)
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()
    d.wait(2.5)


@shot("scan-align/symmetry", "clip", setup=scan_open(TLESS5, then=TURNED))
def scan_symmetry(d):
    """Find Symmetry on T-LESS 5 at an angle, then OK squares the scan to the plane."""
    d.caption("Find Symmetry: the plane the scan mirrors in, drawn over it")
    d.tool("scan.symmetry", panel="panel:SCAN:ALIGN")
    dialog_ready(d)
    d.wait(2.0)
    d.caption("On OK: Make it X = 0 turns the scan square to it")
    d.click(d.at(handle="dialog:On OK"), dur=0.6, settle=0.4)
    d.click(d.at(handle="dialog:On OK:Make it X = 0"), dur=0.4, settle=0.4)
    dialog_ready(d)
    d.wait(1.5)
    ok_and_wait(d, hold=1.0)
    d.ui("ui.view", {"view": "top"})
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()
    d.wait(2.5)


# scan-reconstruct.md: the RECONSTRUCT panel, on scan-like copies of parts modelled in SolidCraft
# itself (families/scanlike: remeshed and given 0.015 mm of noise), so the numbers can be checked
# against the part. The holes plate: 100 x 60 x 12, top at Z = 12, corners R5, holes at
# X = -35, 0, 35 and Y = +-15. The pipe: a 16 mm bar up Z from the origin, bent over to +X.
PLATE = CORPUS / "families/scanlike/holes.ply"
PIPE = CORPUS / "families/scanlike/pipe.ply"


def click_scan(d, at, dur=0.6):
    """Click the scan at the world point `at` (mm)."""
    d.click(d.at(world=list(at)), dur=dur, settle=0.6)


@shot("scan-reconstruct/find-all", "clip", setup=scan_open(PLATE))
def scan_find_all(d):
    """Find All on the holes plate: planes, cylinders, cones and freeform, each its own colour."""
    d.caption("Find All divides the scan into planes, cylinders, cones, spheres and tori")
    d.tool("scan.find_all")
    d.settle_camera()
    for _ in range(240):
        if not d.ui("ui.inspect").get("dialog"):
            break
        time.sleep(0.25)
    d.settle_camera()
    d.wait(3.0)


@shot("scan-reconstruct/fit-plane", "clip", setup=scan_open(PLATE))
def scan_fit_plane(d):
    """Fit Plane on the holes plate's top face: the plane fitted, kept as a trimmed face."""
    d.caption("Fit Plane: click the flat face")
    d.tool("scan.fit_plane")
    d.settle_camera()
    click_scan(d, (15, 0, 12))
    dialog_ready(d)
    d.caption("The fit and how far the scan is off it; OK keeps a face trimmed to it")
    d.wait(2.5)
    ok_and_wait(d, hold=2.5)


@shot("scan-reconstruct/fit-cylinder", "clip", setup=scan_open(PIPE, origin=False))
def scan_fit_cylinder(d):
    """Fit Cylinder on the pipe's straight run: its axis and diameter."""
    d.caption("Fit Cylinder (Reconstruct drop-down): click the round wall")
    d.tool("scan.fit_cylinder", panel="panel:SCAN:RECONSTRUCT")
    d.settle_camera()
    # On the side the view looks at (+X, -Y).
    click_scan(d, (5.66, -5.66, 25))
    dialog_ready(d)
    d.caption("Its diameter and axis; OK keeps a face, an axis, or both")
    d.wait(2.5)
    ok_and_wait(d, hold=2.5)


@shot("scan-reconstruct/rebuild", "clip", setup=scan_open(PLATE))
def scan_rebuild(d):
    """Rebuild (Reconstruct) on the holes plate: the steps that build it as features, then OK."""
    d.caption("Rebuild reads what the scan's shapes make and lists the steps to build it")
    d.tool("scan.reconstruct")
    dialog_ready(d)
    d.wait(3.0)
    d.caption("OK builds them as editable features in the timeline")
    ok_and_wait(d, hold=3.0)


@shot("scan-reconstruct/radius", "clip", setup=scan_open(PLATE))
def scan_radius(d):
    """Radius from Scan: a tap on the plate's corner round reads its radius."""
    d.caption("Radius from Scan (Reconstruct drop-down): tap a round")
    d.tool("scan.radius", panel="panel:SCAN:RECONSTRUCT")
    d.settle_camera()
    click_scan(d, (48.54, 28.54, 6))
    d.settle_camera()
    d.caption("Its radius, side and angle, read off the scan")
    d.wait(3.0)


# scan-freeform.md: the FREEFORM panel, on the family parts as they were modelled (not moved): the
# housing (80 x 60 x 30, its top at Z = 30), the pipe, and the blade (a twisted airfoil lofted up Z
# to 100 mm, its chord along X).
HOUSING = CORPUS / "families/scanlike/housing.ply"
BLADE = CORPUS / "families/scanlike/blade.ply"


def ok_through(d, hold=2.5):
    """OK, and OK again in a dialog the first opens in its place (New Free Form and Wrap Surface
    open Edit Surface on what they made)."""
    first = (d.ui("ui.inspect").get("dialog") or {}).get("command")
    # Aimed again once the pointer is there: a dialog whose lines change as it works moves OK.
    d.glide(d.at(handle="dialog:ok"), 0.6)
    d.click(d.at(handle="dialog:ok"), dur=0.1, settle=0.5)
    for _ in range(1200):
        dlg = d.ui("ui.inspect").get("dialog")
        if not dlg:
            break
        # A new dialog names its own command (one still at work names none).
        if isinstance(dlg, dict) and dlg.get("command") and dlg.get("command") != first and not dlg.get("waiting"):
            d.settle_camera()
            d.wait(1.5)
            d.click(d.at(handle="dialog:ok"), dur=0.6, settle=0.5)
            first = dlg.get("command")
        time.sleep(0.25)
    d.settle_camera()
    d.wait(hold)


@shot("scan-freeform/free-form", "clip", setup=scan_open(HOUSING, origin=False))
def scan_free_form(d):
    """New Free Form on the housing's top: four corners clicked, a face fitted over them."""
    d.caption("New Free Form: click the face's corners in order round it")
    d.tool("scan.free_form")
    d.settle_camera()
    for at in ((-30, -20, 30), (30, -20, 30), (30, 20, 30), (-30, 20, 30)):
        click_scan(d, at, dur=0.5)
    dialog_ready(d)
    d.caption("A surface fitted to the scan between them, coloured by how far off it is")
    d.wait(2.5)
    d.caption("OK keeps it and opens Edit Surface to reshape it")
    ok_through(d)


@shot("scan-freeform/wrap", "clip", setup=scan_open(PIPE, origin=False))
def scan_wrap(d):
    """Wrap Surface round the pipe's straight run."""
    d.caption("Wrap Surface: click the middle of a tube-like part")
    d.tool("scan.wrap_surface")
    d.settle_camera()
    click_scan(d, (5.66, -5.66, 25))
    dialog_ready(d)
    d.caption("A surface wrapped round it, fitted to the scan")
    d.wait(2.5)
    ok_through(d)


@shot("scan-freeform/fit-patches", "clip", setup=scan_open(BLADE, origin=False))
def scan_fit_patches(d):
    """Fit Patches on the twisted blade: a click, the patches fitted over the part."""
    d.caption("Fit Patches: click the part of the scan to cover")
    d.tool("scan.fit_patches")
    d.settle_camera()
    click_scan(d, (15, 4, 50))
    dialog_ready(d)
    d.caption("Smooth patches fitted over it, coloured by how far the scan is off them")
    d.wait(3.0)
    ok_through(d)


@shot("scan-freeform/fit-surface", "clip", setup=scan_open(BLADE, origin=False))
def scan_fit_surface(d):
    """Fit Surface on the twisted blade: one smooth surface round the click, as many control
    points as bring it within the tolerance, trimmed to the area."""
    d.caption("Fit Surface (Freeform drop-down): one smooth surface over a curved part, round the click")
    d.tool("scan.fit_surface", panel="panel:SCAN:FREEFORM")
    d.settle_camera()
    click_scan(d, (15, 4, 50))
    dialog_ready(d)
    d.wait(2.5)
    d.caption("OK keeps it trimmed to the area fitted")
    ok_through(d)


# scan-sketch.md: the SKETCH panel. The block (60 x 40 x 40: a pocket, a slot, holes, a bevel),
# the blade, and the side wall of a van (the user's own scan, used with their permission; outside
# the corpus: VAN_WALL).
BLOCK = CORPUS / "families/scanlike/block.ply"
VAN_WALL = Path(os.environ.get("VAN_WALL") or Path.home() / "Documents/3D-CAD/Scan2.stl")


@shot("scan-sketch/section", "clip", setup=scan_open(BLOCK, origin=False))
def scan_section(d):
    """Cut to Sketch on the block: a plane through it, the cut sketched as lines and arcs."""
    d.caption("Cut to Sketch: where a plane cuts the scan, sketched as lines, arcs and circles")
    d.tool("scan.section_sketch")
    dialog_ready(d)
    d.wait(2.5)
    d.caption("OK: a sketch to dimension and extrude")
    ok_and_wait(d, hold=3.0)


@shot("scan-sketch/curve", "clip", setup=scan_open(BLADE, origin=False))
def scan_curve(d):
    """Curve on Scan up the blade: points clicked, a smooth 3D curve laid on the scan through them."""
    d.caption("Curve on Scan: click points along the line to trace")
    d.tool("scan.curve_on_scan")
    d.settle_camera()
    for z in (12, 37, 62, 88):
        click_scan(d, (15, 4, z), dur=0.5)
    dialog_ready(d)
    d.wait(1.5)
    d.caption("OK: a 3D sketch curve on the scan, for a sweep path or an edge")
    ok_and_wait(d, hold=3.0)


@shot("scan-sketch/flatten", "clip", setup=scan_open(HOUSING, origin=False))
def scan_flatten(d):
    """Flatten the housing's rounded corner (a cylinder between two flat sides): unrolled flat, its
    outline sketched, the stretch said."""
    d.caption("Flatten (Sketch drop-down): click the part of the scan to unroll")
    d.tool("scan.flatten", panel="panel:SCAN:SKETCH")
    d.settle_camera()
    click_scan(d, (37.72, -27.72, 15))
    dialog_ready(d)
    d.click(d.at(handle="dialog:Area"), dur=0.5, settle=0.4)
    d.click(d.at(handle="dialog:Area:Round the click"), dur=0.4, settle=0.4)
    # 10 mm round the click: the corner and the sides beside it, short of the top's round, which
    # curves both ways and cannot lie flat.
    d.click(d.at(handle="dialog:length:Radius"), dur=0.4, settle=0.2)
    d.ui("ui.key", {"key": "A", "cmd": True})
    d.ui("ui.text", {"text": "10"})
    d.wait(0.3)
    d.key("Tab")
    dialog_ready(d)
    d.caption("Laid flat; it says how much the pattern stretches (here next to nothing)")
    d.wait(2.5)
    ok_and_wait(d, hold=1.0)
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()
    d.caption("The outline sketched flat, to draw on and cut")
    d.wait(3.0)


def van_open(d):
    if not VAN_WALL.is_file():
        raise RuntimeError(f"{VAN_WALL}: not found (set VAN_WALL to the wall scan)")
    d.execute("file.insert_mesh", {"path": str(VAN_WALL)})
    d.ui("ui.workspace", {"workspace": "scan"})
    d.ui("ui.view", {"view": "front"})
    d.ui("ui.view", {"view": "fit"})
    d.wait(6.0)
    d.settle_camera()


@shot("scan-sketch/flat-template", "clip", setup=van_open)
def scan_flat_template(d):
    """Flat Template on a van's side wall: the whole wall's outline, with cut-outs for what stands off it."""
    d.caption("Flat Template: a flat template of a wall, for plywood or a liner")
    d.tool("scan.flat_template")
    d.settle_camera()
    d.caption("Outline: Whole Scan follows the wall's own edge")
    d.click(d.at(handle="dialog:Outline"), dur=0.6, settle=0.4)
    d.click(d.at(handle="dialog:Outline:Whole Scan"), dur=0.4, settle=0.4)
    dialog_ready(d, timeout=600)
    d.caption("The wall coloured by how far it stands off the template; cut-outs where it stands off")
    d.wait(3.0)
    d.caption("OK: the template as a sketch, ready to export as DXF")
    ok_and_wait(d, hold=1.0)
    # The scan hidden, to see the sketch.
    d.click(d.at(handle=f"eye:{VAN_WALL.stem}:1"), dur=0.6, settle=1.0)
    d.settle_camera()
    d.wait(3.0)


# scan-check.md: the CHECK panel. The holes plate's scan against a model built the way the plate
# was, but with its corners left sharp: the scan's rounded corners show as where the two differ.
PLATE_MODEL = [
    ("solid.box", {"length": 100, "width": 60, "height": 12, "center": [0, 0, 6], "name": "Plate", "body_name": "Plate"}),
    ("solid.hole", {"position": [-35, 15, 12], "diameter": 6.6, "type": "counterbore", "cb_diameter": 11, "cb_depth": 6.5}),
    ("solid.hole", {"position": [-35, -15, 12], "diameter": 6.6, "type": "counterbore", "cb_diameter": 11, "cb_depth": 6.5}),
    ("solid.hole", {"position": [0, 15, 12], "diameter": 5.5, "type": "countersink", "cs_diameter": 10.4, "cs_angle": 90}),
    ("solid.hole", {"position": [0, -15, 12], "diameter": 5.5, "type": "countersink", "cs_diameter": 10.4, "cs_angle": 90}),
    ("solid.hole", {"position": [35, 15, 12], "thread": "M6"}),
    ("solid.hole", {"position": [35, -15, 12], "diameter": 8, "depth": 8, "type": "drilled"}),
]


def plate_and_model(moved=False):
    def setup(d):
        for command, params in PLATE_MODEL:
            d.execute(command, params)
        then = [("solid.move", {"bodies": ["holes"], "axis": [0, 0, 1], "angle": "12 deg", "translate": [8, -5, 6]})] if moved else []
        scan_open(PLATE, origin=False, then=then)(d)

    return setup


@shot("scan-check/deviation", "clip", setup=plate_and_model())
def scan_check_deviation(d):
    """Scan Deviation: the scan coloured by its distance from the model, a pass or fail said."""
    d.caption("Scan Deviation colours the scan by how far it is from the model")
    d.tool("scan.deviation")
    d.settle_camera()
    for _ in range(240):
        if not d.ui("ui.inspect").get("dialog"):
            break
        time.sleep(0.25)
    d.settle_camera()
    d.wait(1.5)
    d.caption("Green within the tolerance; the extremes labelled: at a corner, 1.45 mm below the sharp model")
    d.wait(3.0)


@shot("scan-check/pin", "clip", setup=plate_and_model())
def scan_check_pin(d):
    """Pin Deviation: places on the deviation map labelled with their distances."""
    d.tool("scan.deviation")
    d.settle_camera()
    d.wait(1.0)
    d.caption("Pin Deviation (Check drop-down): click where you want a reading")
    d.tool("scan.deviation_pin", panel="panel:SCAN:CHECK")
    d.settle_camera()
    for at in ((49.6, 29.6, 6), (10, 0, 12), (-49.6, -29.6, 6)):
        click_scan(d, at, dur=0.5)
    d.settle_camera()
    d.wait(3.0)


@shot("scan-check/align-to-model", "clip", setup=plate_and_model(moved=True))
def scan_check_align_to_model(d):
    """Align to Model: the scan, turned and shifted off the model, laid onto it."""
    d.caption("The scan lies off the model it is checked against")
    d.wait(1.5)
    d.caption("Align to Model lays it onto the model, fitted to its surface")
    d.tool("scan.align_to_model")
    d.settle_camera()
    dialog_ready(d)
    d.wait(1.5)
    ok_and_wait(d, hold=1.0)
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()
    d.caption("The two lie together; the model stays put")
    d.wait(3.0)


# Every choice of Fit Shape and Find Symmetry, one clip each, on the pages that cover them.


def choose(d, handle, option):
    """Open a dialog's drop-down `handle` and pick `option` in it."""
    d.click(d.at(handle=handle), dur=0.5, settle=0.4)
    d.click(d.at(handle=f"{handle}:{option}"), dur=0.4, settle=0.4)


def set_field(d, handle, value):
    """Type `value` into the dialog's value field `handle` (as `dialog:length:Radius` names it)
    and leave it, so the dialog takes it."""
    d.click(d.at(handle=handle), dur=0.4, settle=0.2)
    d.ui("ui.key", {"key": "A", "cmd": True})
    d.ui("ui.text", {"text": str(value)})
    d.wait(0.3)
    d.key("Tab")
    d.settle_camera()


def tick(d, label):
    """Tick or untick the dialog's check box `label`. A box alone in a row's control column is drawn
    at the column's left while its handle is the column's middle, where a click does nothing: click
    the box (CHECKBOX_LEFT points left of the handle) and, if the dialog didn't change, the handle."""
    x, y = d.at(handle=f"dialog:{label}")
    before = d.ui("ui.inspect").get("dialog")
    d.click((x - CHECKBOX_LEFT, y), dur=0.4, settle=0.5)
    if d.ui("ui.inspect").get("dialog") == before:
        d.click((x, y), dur=0.2, settle=0.5)
    d.settle_camera()


# How far a lone check box sits left of its row's handle, in the SIZE window's dialog.
CHECKBOX_LEFT = 54


def field_handle(d, label):
    """The handle of the value field in the row `label` (`dialog:length:…`, `dialog:angle:…`, …)."""
    for h in d.ui("ui.handles"):
        name = h["name"]
        # dialog:<kind>:<label>: not the row, and not dialog:<label> (a check box of the same name).
        if name.startswith("dialog:") and name.endswith(f":{label}") and name.count(":") >= 2 and not name.startswith(("dialog:row:", "dialog:cell:", "dialog:clear:")):
            return name
    raise RuntimeError(f"no field {label}")


def fit_shape(d, shape, at, keep=None, area=None, radius=None):
    """Open Fit Shape, choose `shape`, click the scan at `at`, and set Keep and Area."""
    d.tool(f"scan.fit_{shape.lower()}", panel=None if shape == "Plane" else "panel:SCAN:RECONSTRUCT")
    d.settle_camera()
    click_scan(d, at)
    if keep:
        choose(d, "dialog:Keep", keep)
    if area:
        choose(d, "dialog:Area", area)
    if radius:
        d.click(d.at(handle="dialog:length:Radius"), dur=0.4, settle=0.2)
        d.ui("ui.key", {"key": "A", "cmd": True})
        d.ui("ui.text", {"text": str(radius)})
        d.wait(0.3)
        d.key("Tab")
    dialog_ready(d)


def round_parts(d):
    """A sphere (R20) and a torus (R30 round a 6 mm tube) modelled here and saved as a mesh, then
    inserted as a scan would be: exact shapes, no noise."""
    d.execute("solid.sphere", {"radius": 20, "center": [0, 0, 20], "body_name": "Ball"})
    d.execute("solid.torus", {"major": 30, "minor": 6, "center": [80, 0, 6], "operation": "new_body", "body_name": "Ring"})
    mesh = Path(tempfile.gettempdir()) / f"round-parts-{os.getpid()}.stl"
    d.execute("file.save_mesh", {"path": str(mesh)})
    d.execute("file.new", {})
    scan_open(mesh, origin=False)(d)


@shot("scan-reconstruct/fit-plane-construction", "clip", setup=scan_open(PLATE))
def scan_fit_plane_construction(d):
    """Fit Plane kept as a construction plane, to sketch on."""
    d.caption("Fit Plane, Keep: Construction plane")
    fit_shape(d, "Plane", (15, 0, 12), keep="Construction")
    d.wait(2.0)
    d.caption("OK: a construction plane on the face, in the browser, to sketch on")
    ok_and_wait(d, hold=3.0)


@shot("scan-reconstruct/fit-plane-round", "clip", setup=scan_open(PLATE))
def scan_fit_plane_round(d):
    """Fit Plane over a round area of a given size round the click."""
    d.caption("Fit Plane, Area: Round the click, 15 mm")
    fit_shape(d, "Plane", (15, 0, 12), area="Round the click", radius=15)
    d.caption("Only the scan within 15 mm of the click is fitted")
    d.wait(3.0)
    ok_and_wait(d, hold=2.5)


@shot("scan-reconstruct/fit-cylinder-both", "clip", setup=scan_open(PIPE, origin=False))
def scan_fit_cylinder_both(d):
    """Fit Cylinder on the pipe, keeping both a face and its axis."""
    d.caption("Fit Cylinder, Keep: Both: a face and the axis")
    fit_shape(d, "Cylinder", (5.66, -5.66, 25), keep="Both")
    d.caption("Its axis and end rings drawn before OK; 15.99 mm across")
    d.wait(3.0)
    ok_and_wait(d, hold=3.0)


@shot("scan-reconstruct/fit-cone", "clip", setup=scan_open(PLATE, camera={"distance": 120}))
def scan_fit_cone(d):
    """Fit Cone in a countersink of the plate."""
    d.caption("Fit Cone: click inside a countersink")
    # On the cone of the countersink at Y = 15, 4 mm from its axis, on the far wall the view sees.
    fit_shape(d, "Cone", (-2.83, 17.83, 10.8))
    d.caption("The cone fitted: its axis, and its angle in the dialog")
    d.wait(3.0)
    ok_and_wait(d, hold=2.5)


@shot("scan-reconstruct/fit-sphere", "clip", setup=round_parts)
def scan_fit_sphere(d):
    """Fit Sphere on a ball."""
    d.caption("Fit Sphere: click the ball")
    fit_shape(d, "Sphere", (0, -14, 34))
    d.caption("The sphere fitted, its centre and size in the dialog")
    d.wait(3.0)
    ok_and_wait(d, hold=2.5)


@shot("scan-reconstruct/fit-torus", "clip", setup=round_parts)
def scan_fit_torus(d):
    """Fit Torus on a ring."""
    d.caption("Fit Torus: click the ring")
    fit_shape(d, "Torus", (80, -36, 6))
    d.caption("The torus fitted: its middle circle and tube")
    d.wait(3.0)
    ok_and_wait(d, hold=2.5)


@shot("scan-align/symmetry-keep", "clip", setup=scan_open(TLESS5, then=TURNED))
def scan_symmetry_keep(d):
    """Find Symmetry kept as a construction plane, coloured by mismatch first."""
    d.caption("Find Symmetry: the plane found, and how much of the scan matches it")
    d.tool("scan.symmetry", panel="panel:SCAN:ALIGN")
    dialog_ready(d)
    d.wait(1.5)
    d.caption("Color by mismatch: green where the two sides agree, red where they differ")
    d.click(d.at(handle="dialog:Color by mismatch"), dur=0.6, settle=0.6)
    d.settle_camera()
    d.wait(2.5)
    d.caption("OK with Keep the plane: a construction plane to mirror features across")
    ok_and_wait(d, hold=3.0)


@shot("scan-align/symmetry-guide", "clip", setup=scan_open(TLESS5))
def scan_symmetry_guide(d):
    """Find Symmetry on a part with two mirror planes: a guide drawn across picks the other one."""
    d.caption("A part with two mirror planes: Find Symmetry finds one of them")
    d.tool("scan.symmetry", panel="panel:SCAN:ALIGN")
    dialog_ready(d)
    d.wait(1.5)
    d.caption("Seen from above, Draw a guide along the other, and that plane is found")
    d.view("top")
    d.click(d.at(handle="dialog:Draw a guide"), dur=0.6, settle=0.4)
    # The plane found first is YZ: the guide runs along X, across it, for XZ.
    a, b = d.ui("ui.worldToScreen", {"points": [[-40, 0, 30], [40, 0, 30]]})
    d.drag(tuple(a), tuple(b), steps=30)
    found = dialog_ready(d)
    print(f"  plane after the guide: {[c.get('params', {}).get('plane') for c in found.get('commands', [])]}", file=sys.stderr)
    d.wait(2.0)
    d.view("iso")
    d.caption("The other plane, XZ, found along the guide")
    d.wait(2.5)


# The tools without a clip yet, and more choices of the others.


def markers_plate(d):
    """A plate with four reference markers (6 mm balls) stuck on it, modelled here and saved as a
    mesh, inserted as a scan with markers on it would be."""
    d.execute("solid.box", {"length": 100, "width": 60, "height": 10, "center": [0, 0, 5], "body_name": "Plate"})
    for i, (x, y) in enumerate([(-35, -20), (35, -20), (35, 20), (-35, 20)]):
        d.execute("solid.sphere", {"radius": 3, "center": [x, y, 11], "operation": "join", "targets": ["Plate"], "name": f"Marker{i}"})
    mesh = Path(tempfile.gettempdir()) / f"markers-{os.getpid()}.stl"
    d.execute("file.save_mesh", {"path": str(mesh)})
    d.execute("file.new", {})
    scan_open(mesh, origin=False)(d)


@shot("scan-clean/remove-markers", "clip", setup=markers_plate)
def scan_remove_markers(d):
    """Remove Markers: one marker clicked, every ball of its size found, taken off, the holes closed."""
    d.caption("Remove Markers (Clean drop-down): click one marker")
    d.tool("scan.remove_markers", panel="panel:SCAN:CLEAN")
    d.settle_camera()
    click_scan(d, (35, -20, 14))
    d.settle_camera()
    d.caption("Every marker of its size is found; OK takes them off and closes the holes")
    d.wait(2.5)
    if d.handle("dialog:ok"):
        ok_and_wait(d, hold=3.0)
    else:
        d.key("Enter")
        d.settle_camera()
        d.wait(3.0)


def three_passes(d):
    for path in PASSES + [CORPUS / "stanford/bunny/bunny/data/bun090.ply"]:
        d.execute("file.insert_mesh", {"path": str(path), "units": "m"})
    d.execute("appearance.assign", {"bodies": ["bun045"], "color": "#e8542c"})
    d.execute("appearance.assign", {"bodies": ["bun090"], "color": "#3a8fd8"})
    d.ui("ui.workspace", {"workspace": "scan"})
    d.ui("ui.view", {"view": "iso"})
    d.ui("ui.view", {"view": "fit"})
    d.wait(4.0)
    d.settle_camera()


@shot("scan-align/align-all", "clip", setup=three_passes)
def scan_align_all(d):
    """Align All Scans: three passes of the bunny laid together at once."""
    d.caption("Three passes of one part, each where its scanner saw it")
    d.wait(1.0)
    d.tool("scan.align")
    d.settle_camera()
    d.caption("Align All Scans fits every overlapping pair at once")
    d.click(d.at(handle="dialog:button::Align All Scans"), dur=0.6, settle=0.6)
    dialog_ready(d)
    d.wait(1.5)
    ok_and_wait(d, hold=1.0)
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()
    d.caption("The first pass stays put; the others lie on it")
    d.wait(2.5)


@shot("scan-align/combine", "clip", setup=three_passes)
def scan_combine(d):
    """Combine Scans: the passes aligned and fused into one even scan."""
    d.caption("Combine Scans (Align drop-down): align the passes, then fuse them into one")
    d.tool("scan.combine", panel="panel:SCAN:ALIGN")
    dialog_ready(d, timeout=600)
    d.wait(1.5)
    ok_and_wait(d, hold=1.0)
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()
    d.caption("One scan, its triangles even, the passes' noise averaged out")
    d.wait(3.0)


@shot("scan-reconstruct/selection", "clip", setup=scan_open(BLOCK, origin=False))
def scan_reconstruct_selection(d):
    """Reconstruct Selection: the front hole of the block selected, read and built on its own."""
    d.caption("Select on Scan, Region: click the hole's wall")
    d.tool("scan.select")
    d.settle_camera()
    choose(d, "dialog:Tool", "Region")
    # Just inside the hole's mouth: the click lands on its wall.
    click_scan(d, (11, -18, 22))
    d.settle_camera()
    d.wait(1.0)
    ok_and_wait(d, hold=0.5)
    d.caption("Reconstruct Selection (Reconstruct drop-down) reads only what is selected")
    d.tool("scan.reconstruct_selection", panel="panel:SCAN:RECONSTRUCT")
    dialog_ready(d)
    d.wait(2.5)
    ok_and_wait(d, hold=3.0)


HOUSING_FACE = [("scan.free_form", {"body": "{body}", "corners": [[-30, -20, 30], [30, -20, 30], [30, 20, 30], [-30, 20, 30]]})]


@shot("scan-freeform/make-flat", "clip", setup=scan_open(HOUSING, origin=False, then=HOUSING_FACE))
def scan_make_flat(d):
    """Make Flat on a Free Form fitted to the housing's top: every point moved onto one plane."""
    d.caption("Make Flat (Freeform drop-down): every point of a Free Form onto one plane")
    d.tool("scan.make_flat", panel="panel:SCAN:FREEFORM")
    dialog_ready(d)
    d.wait(2.0)
    ok_and_wait(d, hold=3.0)


@shot("scan-freeform/slice", "clip", setup=scan_open(HOUSING, origin=False, then=HOUSING_FACE))
def scan_slice(d):
    """Slice Surface: a Free Form cut square at a plane, one side kept."""
    d.caption("Slice Surface (Freeform drop-down): cut a surface with a plane, keep one side")
    d.tool("scan.slice_surface", panel="panel:SCAN:FREEFORM")
    d.settle_camera()
    d.caption("Click the surface, then the plane (here the origin's YZ)")
    click_scan(d, (10, 0, 30))
    d.click(d.at(handle="fold:Origin"), dur=0.6, settle=0.4)
    d.click(d.at(handle="row:YZ"), dur=0.5, settle=0.4)
    dialog_ready(d)
    d.wait(2.0)
    ok_and_wait(d, hold=3.0)


def blade_with_curve(d):
    """The blade, with a curve drawn up it by Curve on Scan (clicked, as in its own clip)."""
    scan_open(BLADE, origin=False)(d)
    d.tool("scan.curve_on_scan")
    d.settle_camera()
    for z in (12, 37, 62, 88):
        click_scan(d, (15, 4, z), dur=0.2)
    dialog_ready(d)
    ok_and_wait(d, hold=0.5)


@shot("scan-sketch/sections-along-curve", "clip", setup=blade_with_curve)
def scan_sections_along_curve(d):
    """Sections Along Curve: the blade cut square to a curve up it, each section sketched, ready to loft."""
    d.caption("Sections Along Curve (Sketch drop-down): sections square to a curve, at even steps")
    d.tool("scan.sections_along_curve", panel="panel:SCAN:SKETCH")
    dialog_ready(d)
    d.wait(2.5)
    d.caption("OK: each section sketched as lines and arcs, ready to loft")
    ok_and_wait(d, hold=3.0)


@shot("scan-check/triangles", "clip", setup=scan_open(PLATE, camera={"distance": 28}))
def scan_check_triangles(d):
    """Scan Triangles: the scan's own triangles drawn over it."""
    d.caption("Scan Triangles (Check drop-down): the scan's triangles, drawn over it")
    d.tool("view.scan_triangles", panel="panel:SCAN:CHECK")
    d.settle_camera()
    d.wait(3.0)
    d.caption("Press it again to stop")
    d.tool("view.scan_triangles", panel="panel:SCAN:CHECK")
    d.settle_camera()
    d.wait(1.5)


@shot("scan-check/hole-table", "clip", setup=plate_and_model())
def scan_check_hole_table(d):
    """Hole Table: every round hole of the model numbered, with its place and size."""
    d.caption("Hole Table (Check drop-down): every round hole of the shown solids, numbered")
    d.tool("inspect.hole_table", panel="panel:SCAN:CHECK")
    d.settle_camera()
    d.wait(3.5)


@shot("scan-freeform/edit-surface", "clip", setup=scan_open(HOUSING, origin=False, then=HOUSING_FACE))
def scan_edit_surface(d):
    """Edit Surface on a Free Form: a point dragged up lifts off the scan, the points round it
    following."""
    d.caption("Edit Surface (Freeform drop-down): drag a point of the surface")
    d.tool("scan.edit_surface", panel="panel:SCAN:FREEFORM")
    d.settle_camera()
    d.wait(1.0)
    a, b = d.ui("ui.worldToScreen", {"points": [[10, 7, 30], [10, 7, 45]]})
    d.drag(tuple(a), tuple(b), steps=30)
    d.settle_camera()
    d.caption("It lifts off the scan, the points round it following; colours say how far off")
    d.wait(2.5)
    ok_and_wait(d, hold=2.5)


@shot("scan-reconstruct/rest", "clip", setup=scan_open(BLOCK, origin=False))
def scan_reconstruct_rest(d):
    """Rebuild with "Then reconstruct what is still missing" ticked."""
    d.caption("Rebuild, with Then reconstruct what is still missing ticked")
    d.tool("scan.reconstruct")
    dialog_ready(d)
    for h in d.ui("ui.handles"):
        if h["name"].startswith("dialog:") and "missing" in h["name"].lower() and not h["name"].startswith("dialog:row"):
            d.click((h["x"], h["y"]), dur=0.6, settle=0.4)
            break
    dialog_ready(d)
    d.wait(2.0)
    d.caption("OK builds the steps, then each piece of the scan the body still leaves out")
    ok_and_wait(d, hold=3.0)


@shot("scan-reconstruct/fillet-from-scan", "clip", setup=plate_and_model())
def scan_fillet_from_scan(d):
    """Fillet from Scan: the model's sharp corner edge filleted at the radius the scan shows there."""
    d.caption("Fillet from Scan (Reconstruct drop-down): pick the model's edges the scan rounds")
    d.tool("scan.fillet", panel="panel:SCAN:RECONSTRUCT")
    d.settle_camera()
    d.click(d.at(world=[50, -30, 6]), dur=0.6, settle=0.6)
    dialog_ready(d)
    d.caption("The radius read off the scan along the edge")
    d.wait(2.5)
    ok_and_wait(d, hold=3.0)


@shot("scan-clean/clean-up-reduce", "clip", setup=scan_open(MUG, "m"))
def scan_clean_up_reduce(d):
    """Clean Up with Reduce Mesh ticked: the clean steps, then fewer triangles."""
    d.caption("Clean Up, with Reduce Mesh ticked: the scan cleaned, then reduced")
    d.tool("scan.clean_up")
    dialog_ready(d)
    d.click(d.at(handle="dialog:Reduce Mesh"), dur=0.6, settle=0.4)
    dialog_ready(d)
    d.wait(2.0)
    ok_and_wait(d, hold=3.0)


@shot("scan-clean/split-keep", "clip", setup=scan_open(BUNNY, "m", upright=True))
def scan_split_keep(d):
    """Split Mesh keeping only the side in front of the plane."""
    d.caption("Split Mesh, Keep: In Front: the other side taken off")
    d.tool("scan.split", panel="panel:SCAN:CLEAN")
    d.settle_camera()
    d.click(d.at(handle="fold:Origin"), dur=0.6, settle=0.4)
    d.click(d.at(handle="row:YZ"), dur=0.5, settle=0.4)
    choose(d, "dialog:Keep", "In Front")
    dialog_ready(d)
    d.wait(2.0)
    ok_and_wait(d, hold=3.0)


@shot("scan-sketch/curve-from-selection", "clip", setup=scan_open(BLOCK, origin=False))
def scan_curve_from_selection(d):
    """Curve from Selection Edge: the block's top face selected, its outline traced as a 3D curve."""
    d.caption("Select the top face (Select on Scan, Region): up to its edges")
    d.tool("scan.select")
    d.settle_camera()
    choose(d, "dialog:Tool", "Region")
    click_scan(d, (20, 0, 40))
    d.settle_camera()
    ok_and_wait(d, hold=0.5)
    d.caption("Curve from Selection Edge (Sketch drop-down): its outline as a 3D curve")
    d.tool("scan.curve_from_selection", panel="panel:SCAN:SKETCH")
    dialog_ready(d)
    d.wait(2.0)
    ok_and_wait(d, hold=3.0)


# -- main ---------------------------------------------------------------------------------------



# ---- Tours: one short clip per choice of a tool, on the scan that shows it best. ----


def scan_box(d):
    """The scan's box, (lo, hi), in mm."""
    measured = d.ui("engine.execute", {"command": "inspect.measure", "params": {"bodies": [d.scan]}})["bodies"]
    box = next(m["bbox"] for m in measured if m["name"] == d.scan)
    return box["min"], box["max"]


def box_point(d, fx, fy, fz):
    """The point at fractions (fx, fy, fz) across the scan's box."""
    lo, hi = scan_box(d)
    return [lo[i] + (hi[i] - lo[i]) * f for i, f in enumerate((fx, fy, fz))]


def screen_of(d, world):
    return tuple(d.ui("ui.worldToScreen", {"points": [world]})[0])


@shot("scan-clean/select-polygon", "clip", setup=scan_open(BUNNY, "m", upright=True))
def scan_select_polygon(d):
    """Select on Scan, Polygon: corners clicked round the bunny's head, the first clicked again."""
    d.caption("Select on Scan, Tool Polygon: click corners round what you see")
    d.tool("scan.select")
    d.settle_camera()
    choose(d, "dialog:Tool", "Polygon")
    for f in [(0.05, 0.5, 0.55), (0.45, 0.5, 0.55), (0.5, 0.5, 1.0), (0.05, 0.5, 1.0)]:
        d.click(screen_of(d, box_point(d, *f)), dur=0.5, settle=0.3)
    d.caption("Click the first corner again: what is inside, on the side you see, is selected")
    d.click(screen_of(d, box_point(d, 0.05, 0.5, 0.55)), dur=0.5, settle=0.8)
    d.wait(2.5)


@shot("scan-clean/select-add-remove", "clip", setup=scan_open(BUNNY, "m", upright=True))
def scan_select_add_remove(d):
    """Select on Scan: a brush stroke, a second added with Selection Add, a bite taken out with Remove."""
    d.caption("Brush a stroke: Selection Replace starts afresh")
    d.tool("scan.select")
    d.settle_camera()
    set_field(d, "dialog:length:Radius", 8)
    d.drag(screen_of(d, box_point(d, 0.8, 0.5, 0.3)), screen_of(d, box_point(d, 0.4, 0.5, 0.35)), steps=40)
    d.wait(0.8)
    d.caption("Selection Add: a second stroke joins the first")
    choose(d, "dialog:Selection", "Add")
    d.drag(screen_of(d, box_point(d, 0.75, 0.5, 0.55)), screen_of(d, box_point(d, 0.4, 0.5, 0.6)), steps=40)
    d.wait(0.8)
    d.caption("Selection Remove: a stroke across both takes that much away")
    choose(d, "dialog:Selection", "Remove")
    d.drag(screen_of(d, box_point(d, 0.6, 0.5, 0.2)), screen_of(d, box_point(d, 0.6, 0.5, 0.7)), steps=40)
    d.wait(2.5)


@shot("scan-clean/select-region", "clip", setup=scan_open(BLOCK, origin=False))
def scan_select_region(d):
    """Select on Scan, Region: a click on the block's side takes it up to its creases."""
    d.caption("Select on Scan, Tool Region: click a smooth area")
    d.tool("scan.select")
    d.settle_camera()
    choose(d, "dialog:Tool", "Region")
    click_scan(d, (15, -20, 30))
    d.caption("It grows up to the creases sharper than the Crease angle")
    d.wait(2.0)
    d.caption("Grow and Shrink widen or narrow it by a ring of triangles")
    d.click(d.at(handle="dialog:button::Grow"), dur=0.5, settle=0.6)
    d.click(d.at(handle="dialog:button::Grow"), dur=0.2, settle=0.6)
    d.wait(2.0)


@shot("scan-clean/split-envelope", "clip", setup=scan_open(BUNNY, "m", upright=True))
def scan_split_envelope(d):
    """Split Mesh with an Envelope: a slab 10 mm thick round the YZ plane taken out as its own scan."""
    d.caption("Split Mesh, Envelope 10 mm: a slab either side of the cutter")
    d.tool("scan.split", panel="panel:SCAN:CLEAN")
    d.settle_camera()
    d.click(d.at(handle="fold:Origin"), dur=0.6, settle=0.4)
    d.click(d.at(handle="row:YZ"), dur=0.5, settle=0.4)
    set_field(d, field_handle(d, "Envelope"), 10)
    dialog_ready(d)
    d.caption("The slab becomes a scan of its own: a cross-section band to study")
    d.wait(2.0)
    ok_and_wait(d, hold=1.0)
    d.execute("browser.visibility", {"items": [BUNNY.stem], "visible": False})
    d.wait(2.5)


@shot("scan-align/quick-align-canteen", "clip", setup=scan_open(CANTEEN, "m", then=TILTED))
def scan_quick_align_canteen(d):
    """Quick Align on a CC0 scan of a flat canteen, arrived at an angle."""
    d.caption("A canteen, scanned lying at an angle: Align, Quick Align")
    d.tool("scan.align")
    d.settle_camera()
    d.click(d.at(handle="dialog:button::Quick Align"), dur=0.6, settle=0.6)
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()
    d.caption("Square to its own faces, longest along X, flat side down on the grid")
    d.wait(2.5)
    d.ui("ui.view", {"view": "front"})
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()
    d.wait(2.0)


@shot("scan-align/stand-on-face", "clip", setup=scan_open(BLOCK, origin=False))
def scan_stand_on_face(d):
    """Stand on Base on the face clicked: the block tipped onto its front face."""
    d.caption("Stand on Base: click the face to stand it on")
    d.tool("scan.stand_on_base", panel="panel:SCAN:ALIGN")
    d.settle_camera()
    # The front face, beside its hole.
    click_scan(d, (-15, -20, 12))
    dialog_ready(d)
    d.caption("The face grown from the click is marked; OK stands the part on it")
    d.wait(2.0)
    ok_and_wait(d, hold=0.5)
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()
    d.wait(2.0)


@shot("scan-freeform/fit-surface-helmet", "clip", setup=scan_open(HELMET, "m", upright=True))
def scan_fit_surface_helmet(d):
    """Fit Surface on the dome of a CC0 helmet scan: a surface that curves both ways."""
    d.caption("Fit Surface on a helmet's dome: it curves both ways, no plane or cylinder fits it")
    d.tool("scan.fit_surface", panel="panel:SCAN:FREEFORM")
    d.settle_camera()
    d.click(screen_of(d, box_point(d, 0.5, 0.5, 0.9)), dur=0.6, settle=0.6)
    dialog_ready(d)
    d.caption("Radius sets how much of the dome; the scan is coloured by how far off it is")
    set_field(d, "dialog:length:Radius", 60)
    dialog_ready(d)
    d.wait(2.5)
    ok_and_wait(d, hold=1.0)
    d.execute("browser.visibility", {"items": [d.scan], "visible": False})
    d.caption("The surface on its own, trimmed to the area: a face to build on")
    d.wait(2.5)


@shot("scan-freeform/fit-surface-untrimmed", "clip", setup=scan_open(BLADE, origin=False))
def scan_fit_surface_untrimmed(d):
    """Fit Surface with Trim to the area unticked: the surface spans the area's whole rectangle."""
    d.caption("Fit Surface, Trim to the area off: the whole rectangle round the area")
    d.tool("scan.fit_surface", panel="panel:SCAN:FREEFORM")
    d.settle_camera()
    click_scan(d, (15, 4, 50))
    dialog_ready(d)
    tick(d, "Trim to the area")
    dialog_ready(d)
    d.wait(1.5)
    ok_and_wait(d, hold=1.0)
    d.execute("browser.visibility", {"items": [BLADE.stem], "visible": False})
    d.caption("Untrimmed: a four-sided surface, to trim or extend yourself")
    d.wait(2.5)


@shot("scan-freeform/wrap-hammer", "clip", setup=scan_open(HAMMER, "m"))
def scan_wrap_hammer(d):
    """Wrap Surface round a real hammer's handle (YCB)."""
    d.caption("Wrap Surface on a hammer's handle: click the middle of it")
    d.tool("scan.wrap_surface")
    d.settle_camera()
    lo, hi = scan_box(d)
    # The handle runs along the box's longest side; click a third of the way along it.
    long = max(range(3), key=lambda i: hi[i] - lo[i])
    f = [0.5, 0.5, 0.5]
    f[long] = 0.75
    d.click(screen_of(d, box_point(d, *f)), dur=0.6, settle=0.6)
    dialog_ready(d)
    d.caption("A net round the handle, fitted to the scan, open at a seam")
    d.wait(2.5)
    d.caption("Details Fine: more points, closer to the scan")
    choose(d, "dialog:Details", "Fine")
    dialog_ready(d)
    d.wait(2.5)


@shot("scan-freeform/wrap-arc", "clip", setup=scan_open(PIPE, origin=False))
def scan_wrap_arc(d):
    """Wrap Surface with Arc 180°: half way round the bar only."""
    d.caption("Wrap Surface, Arc 180°: the net goes only half way round")
    d.tool("scan.wrap_surface")
    d.settle_camera()
    click_scan(d, (5.66, -5.66, 25))
    dialog_ready(d)
    set_field(d, field_handle(d, "Arc"), 180)
    dialog_ready(d)
    d.wait(2.0)
    d.caption("Stiffness up: the net keeps its shape over bumps instead of following them")
    set_field(d, field_handle(d, "Stiffness"), 40)
    dialog_ready(d)
    d.wait(2.5)


@shot("scan-reconstruct/fit-cylinder-mug", "clip", setup=scan_open(MUG, "m"))
def scan_fit_cylinder_mug(d):
    """Fit Cylinder on a real, noisy mug scan (YCB): the body's diameter read through the noise."""
    d.caption("Fit Cylinder on a real mug scan: click its side")
    d.tool("scan.fit_cylinder", panel="panel:SCAN:RECONSTRUCT")
    d.settle_camera()
    d.click(screen_of(d, box_point(d, 1.0, 0.5, 0.5)), dur=0.6, settle=0.6)
    dialog_ready(d)
    d.caption("Its diameter, axis and how far the scan strays from it, before OK")
    d.wait(3.0)


@shot("scan-sketch/curve-closed", "clip", setup=scan_open(HELMET, "m", upright=True))
def scan_curve_closed(d):
    """Curve on Scan, closed: points round the helmet's dome, the first clicked again."""
    d.caption("Curve on Scan: points round the helmet's dome")
    d.tool("scan.curve_on_scan")
    d.settle_camera()
    import math
    cx, cy = screen_of(d, box_point(d, 0.5, 0.5, 1.0))
    pts = [(cx + 45 * math.cos(t), cy + 45 + 22 * math.sin(t)) for t in [i * math.pi / 4 for i in range(8)]]
    for p in pts:
        d.click(p, dur=0.45, settle=0.3)
    d.caption("Click the first point again: the curve closes into a loop on the scan")
    d.click(pts[0], dur=0.45, settle=0.6)
    dialog_ready(d)
    d.wait(2.0)
    ok_and_wait(d, hold=0.5)
    d.execute("browser.visibility", {"items": [d.scan], "visible": False})
    d.caption("A closed 3D curve: an outline to split, select inside, or sweep along")
    d.wait(2.5)


@shot("scan-clean/select-like", "clip", setup=scan_open(PLATE))
def scan_select_like(d):
    """Select on Scan, Shape with All like it: after Find All, a click on one hole's wall selects
    every hole wall of its size."""
    d.caption("Find All first: the scan divided into shapes")
    d.tool("scan.find_all")
    for _ in range(240):
        if not d.ui("ui.inspect").get("dialog"):
            break
        time.sleep(0.25)
    d.settle_camera()
    d.wait(1.0)
    d.caption("Select on Scan, Tool Shape, All like it ticked")
    d.tool("scan.select")
    d.settle_camera()
    choose(d, "dialog:Tool", "Shape")
    tick(d, "All like it")
    d.caption("Click one hole's wall: every hole of its size is selected")
    # The far wall of the front counterbore (11 mm across, 6.5 deep): its twin at Y = +15 is taken too.
    click_scan(d, (-35 - 3.9, -15 + 3.9, 10))
    d.wait(3.0)


@shot("scan-sketch/curve-drawn", "clip", setup=scan_open(BUNNY, "m", upright=True))
def scan_curve_drawn(d):
    """Curve on Scan drawn as a stroke: dragged along the bunny's back."""
    d.caption("Curve on Scan: drag to draw along the scan")
    d.tool("scan.curve_on_scan")
    d.settle_camera()
    d.drag(screen_of(d, box_point(d, 0.7, 0.5, 0.35)), screen_of(d, box_point(d, 0.45, 0.5, 0.55)), steps=60)
    dialog_ready(d)
    d.caption("The stroke becomes a smooth curve on the scan, its length and fit shown")
    d.wait(3.0)


# -- tours: every option of one tool ------------------------------------------------------------
# A tour ("scan-tour/<tool>") opens one tool on a scan chosen for it and goes through its options
# one by one, each captioned. After every step it notes what the dialog reads and what OK would run;
# with --check that goes to NAME.tour.txt beside the sheet, with these flagged:
#   REFUSED   the dialog shows an error;
#   STUCK     it was still working after the step's time;
#   SAME AS   two choices of one drop-down give the same readout and the same commands (one of them
#             does nothing);
#   NO EFFECT a value or tick changed and neither the readout nor the commands did.
# A flag is also put on the clip as a caption, so the sheet shows it at the frame it happened.


def tour_state(d):
    """What the open dialog shows now: error, waiting note, hints, readout rows (label: value) and
    the commands OK would run. {"closed": True} when no dialog is open."""
    dlg = d.ui("ui.inspect").get("dialog")
    if not isinstance(dlg, dict):
        return {"closed": True}
    rows, hints, cell = {}, [], None
    for h in d.ui("ui.handles"):
        name = h["name"]
        if name.startswith("dialog:cell:"):
            cell = name[len("dialog:cell:") :]
        elif name.startswith("dialog:row:"):
            if cell is not None:
                rows[name[len("dialog:row:") :]] = cell
            cell = None
        elif name.startswith("dialog:hint:"):
            hints.append(name[len("dialog:hint:") :])
    return {"error": dlg.get("error"), "waiting": dlg.get("waiting"), "hints": hints, "rows": rows, "commands": dlg.get("commands")}


def tour_settle(d, timeout=120):
    """Wait until the dialog has worked out the step, refused it, or asks for input; its state, with
    "stuck" when it was still working after `timeout` seconds. A "waiting" note that stays the same
    for 3 s and doesn't say it is working ("...", "…") is a request for input, not work."""
    last, since = None, time.time()
    end = time.time() + timeout
    while time.time() < end:
        dlg = d.ui("ui.inspect").get("dialog")
        # A preview still being worked out beside the window says so in a hint.
        busy = any(h["name"].startswith("dialog:hint:") and ("refining" in h["name"] or "placing" in h["name"]) for h in d.ui("ui.handles"))
        if not busy and (not isinstance(dlg, dict) or dlg.get("error") or not dlg.get("waiting")):
            d.settle_camera()
            return tour_state(d)
        w = (dlg.get("waiting") if isinstance(dlg, dict) else "") or ("…" if busy else "")
        if w != last:
            last, since = w, time.time()
        elif time.time() - since > 3.0 and not ("…" in w or "..." in w):
            return tour_state(d)
        time.sleep(0.25)
    st = tour_state(d)
    st["stuck"] = True
    return st


def short_commands(cmds):
    """What OK runs, short: each command with the params that say how (not the body or the click)."""
    if not cmds:
        return "-"
    out = []
    for c in cmds:
        p = {k: v for k, v in (c.get("params") or {}).items() if k not in ("body", "bodies", "point", "points", "polygon", "stroke", "faces", "triangles")}
        text = json.dumps(p, separators=(",", ":"))
        out.append(f"{c.get('command')}{text if len(text) < 220 else text[:217] + '...'}")
    return " ; ".join(out)


def tour_step(d, caption, act=None, hold=2.0, change=True, timeout=120):
    """Caption the step, do it, wait for the dialog and note what it shows. `change`: the step must
    change the readout or what OK runs (flagged NO EFFECT when neither changed)."""
    d.caption(caption)
    before = d.tour[-1]["state"] if d.tour else None
    if act:
        act()
    st = tour_settle(d, timeout)
    flags = []
    if st.get("error"):
        flags.append(f"REFUSED: {st['error']}")
    elif (st.get("waiting") or "").startswith("invalid parameters"):
        flags.append(f"REFUSED: {st['waiting'].split(': ', 1)[-1]}")
    if st.get("stuck"):
        flags.append(f"STUCK: still working after {timeout} s")
    if change and before and not st.get("closed") and not before.get("closed") and (st.get("rows"), st.get("commands")) == (before.get("rows"), before.get("commands")):
        flags.append("NO EFFECT: readout and what OK runs unchanged")
    d.tour.append({"step": caption, "state": st, "flags": flags})
    if flags:
        d.caption(f"⚠ {flags[0][:110]}")
    d.wait(hold)
    return st


def options_of(d, row):
    """The choices of the dialog's drop-down `row`: opened, read, closed again (Escape would close
    the dialog)."""
    before = d.ui("ui.inspect").get("dialog")
    d.click(d.at(handle=f"dialog:{row}"), dur=0.4, settle=0.5)
    prefix = f"dialog:{row}:"
    opts = list(dict.fromkeys(h["name"][len(prefix) :] for h in d.ui("ui.handles") if h["name"].startswith(prefix)))
    if opts:
        d.click(d.at(handle=f"dialog:{row}"), dur=0.2, settle=0.3)
    elif d.ui("ui.inspect").get("dialog") != before:
        # A check box, and the click ticked it: put it back (apart, or it reads as a double click).
        d.wait(0.6)
        d.click(d.at(handle=f"dialog:{row}"), dur=0.2, settle=0.5)
    return opts


def every(d, row, skip=(), only=None, hold=2.0, note=None, restore=True):
    """Each choice of the drop-down `row` in turn (`only`: these, in this order; `skip`: not these).
    `note(option)`, when given, is added to the caption. Two choices that give the same readout and
    the same commands are flagged SAME AS. `restore`: the choice it had is put back at the end, read
    as the one whose readout and commands match those before."""
    opts = only or [o for o in options_of(d, row) if o not in skip]
    first = tour_state(d)
    was = json.dumps([first.get("rows"), first.get("commands")], sort_keys=True)
    seen = {}
    for o in opts:
        extra = note(o) if note else ""
        st = tour_step(d, f"{row}: {o}{' - ' + extra if extra else ''}", lambda o=o: choose(d, f"dialog:{row}", o), hold=hold, change=False)
        key = json.dumps([st.get("rows"), st.get("commands")], sort_keys=True)
        if key in seen and not st.get("error") and not st.get("waiting"):
            d.tour[-1]["flags"].append(f"SAME AS {row}: {seen[key]}")
        seen.setdefault(key, o)
    back = seen.get(was)
    if restore and back and back != opts[-1]:
        tour_step(d, f"{row}: back to {back}", lambda: choose(d, f"dialog:{row}", back), hold=0.8, change=False)


def dialog_rows(d):
    """The dialog's drop-downs and check boxes by their row labels (not its pick fields)."""
    dlg = d.ui("ui.inspect").get("dialog") or {}
    picks = {i.get("label") for i in (dlg.get("inputs") or [])} if isinstance(dlg, dict) else set()
    names = [h["name"] for h in d.ui("ui.handles")]
    have = set(names)
    rows = []
    for n in names:
        if n.startswith("dialog:row:"):
            r = n[len("dialog:row:") :]
            if r not in picks and f"dialog:{r}" in have and f"dialog:clear:{r}" not in have and r not in rows:
                rows.append(r)
    return rows


def explore(d, skip=(), skip_options=(), hold=1.8):
    """Every drop-down's every choice and every check box ticked and back, as the dialog shows them
    now (and once more for rows an earlier choice brought in). `skip`: rows to leave alone;
    `skip_options`: choices never picked (ones that need more input, tried by hand)."""
    done = set()
    for _ in range(2):
        for r in dialog_rows(d):
            if r in skip or r in done:
                continue
            if r not in dialog_rows(d):
                continue  # an earlier choice took it away
            done.add(r)
            try:
                found = options_of(d, r)
            except RuntimeError as e:
                d.tour.append({"step": f"{r}: not reachable", "state": tour_state(d), "flags": [f"GONE: {e}"]})
                continue
            opts = [o for o in found if o not in skip_options]
            if opts:
                every(d, r, only=opts, hold=hold)
            elif not found:
                toggle(d, r, hold=hold)


def sweep(d, label, values, hold=2.0, unit=""):
    """The value field `label` set to each of `values` in turn (noted, not failed, when the dialog
    has no such field: the fields it has are listed)."""
    try:
        field_handle(d, label)
    except RuntimeError:
        fields = sorted({h["name"] for h in d.ui("ui.handles") if h["name"].count(":") == 2 and h["name"].split(":")[1] in ("length", "angle", "number", "count", "int", "value")})
        d.tour.append({"step": f"{label}: no such field", "state": tour_state(d), "flags": [f"NO FIELD {label}; fields: {', '.join(fields) or 'none'}"]})
        return
    for v in values:
        tour_step(d, f"{label}: {v}{unit}", lambda v=v: set_field(d, field_handle(d, label), v), hold=hold)


def toggle(d, label, hold=2.0):
    """The check box `label` ticked and unticked."""
    d.wait(0.6)  # apart from any click before it, so it isn't taken as a double click
    tour_step(d, f"{label}: changed", lambda: tick(d, label), hold=hold)
    tour_step(d, f"{label}: back", lambda: tick(d, label), hold=hold)


def tour_ok(d, caption, hold=2.5, then=None):
    """OK (and OK in a dialog it opens in its place), then note what the design holds."""
    d.caption(caption)
    # A preview still being worked out changes the dialog under the pointer: wait for it first.
    d.wait(0.5)
    tour_settle(d)
    ok_through(d, hold=0.5)
    st = tour_state(d)
    bodies = d.ui("engine.execute", {"command": "inspect.measure", "params": {}}).get("bodies") or []
    note = ", ".join(f"{b['name']}" for b in bodies)
    flags = [] if st.get("closed") else [f"NOT CLOSED: {st.get('error') or st.get('waiting')}"]
    d.tour.append({"step": caption, "state": {"bodies": note}, "flags": flags})
    if flags:
        d.caption(f"⚠ {flags[0][:110]}")
    if then:
        then()
    d.wait(hold)


def tour_text(name, tour):
    lines = [f"# {name}"]
    for i, s in enumerate(tour, 1):
        st = s["state"]
        rows = ", ".join(f"{k} {v}" for k, v in (st.get("rows") or {}).items())
        lines.append(f"{i:2}. {s['step']}")
        if st.get("bodies") is not None:
            lines.append(f"      design: {st['bodies']}")
        if rows:
            lines.append(f"      reads: {rows}")
        if st.get("waiting"):
            lines.append(f"      waiting: {st['waiting']}")
        if st.get("commands") is not None or "rows" in st:
            lines.append(f"      OK runs: {short_commands(st.get('commands'))}")
        for f in s["flags"]:
            lines.append(f"      !! {f}")
    flagged = sum(1 for s in tour if s["flags"])
    lines.append(f"-- {len(tour)} steps, {flagged} flagged")
    return "\n".join(lines) + "\n"


def tour(command, panel=None):
    """Open the tool for a tour: the toolbar button as a user presses it, then `d.tour` starts."""

    def start(d):
        d.tour = []
        d.tool(command, panel=panel)
        d.settle_camera()

    return start


# The tours, one per tool. Each on the scan that shows the tool best: a family part where the
# answer is known (families/scanlike: modelled here, remeshed, 0.015 mm noise), then, where it says
# more, a real scan.


def tour_click(d, caption, at):
    """A tour step that clicks the scan at the world point `at` (mm)."""
    return tour_step(d, caption, lambda: click_scan(d, at), change=False)


def tour_box_click(d, caption, f):
    """A tour step that clicks the scan at fractions `f` across its box."""
    return tour_step(d, caption, lambda: d.click(screen_of(d, box_point(d, *f)), dur=0.6, settle=0.6), change=False)


def tour_fit(shape, at, what, skip=()):
    def run(d):
        tour(f"scan.fit_{shape}", panel=None if shape == "plane" else "panel:SCAN:RECONSTRUCT")(d)
        tour_click(d, f"Fit {shape.title()}: click {what}", at)
        explore(d, skip=("Shape",) + tuple(skip), skip_options=("Brush", "What is selected"))
        sweep(d, "Scan tolerance", [0.03, 0.5], unit=" mm")
        tour_ok(d, "OK: what Keep said, in the browser")

    return run


# Fit Shape, each shape on a part that has it: the plate's top and countersinks, the pipe's bar, a
# modelled sphere and torus (exact), and a real mug (noisy).
shot("scan-tour/fit-plane", "clip", setup=scan_open(PLATE))(tour_fit("plane", (15, 0, 12), "the plate's top (Z = 12, 100 x 60)"))
shot("scan-tour/fit-cylinder", "clip", setup=scan_open(PIPE, origin=False))(tour_fit("cylinder", (5.66, -5.66, 25), "the pipe's straight bar (16 mm across)"))
shot("scan-tour/fit-cone", "clip", setup=scan_open(PLATE, camera={"distance": 120}))(tour_fit("cone", (-2.83, 17.83, 10.8), "a countersink (90 degrees)"))
shot("scan-tour/fit-sphere", "clip", setup=round_parts)(tour_fit("sphere", (0, -14, 34), "the ball (R20)"))
shot("scan-tour/fit-torus", "clip", setup=round_parts)(tour_fit("torus", (80, -36, 6), "the ring (R30, tube R6)"))


@shot("scan-tour/fit-cylinder-mug", "clip", setup=scan_open(MUG, "m"))
def tour_fit_cylinder_mug(d):
    """Fit Cylinder on a real mug: Area and tolerance through the Kinect noise."""
    tour("scan.fit_cylinder", panel="panel:SCAN:RECONSTRUCT")(d)
    tour_box_click(d, "Fit Cylinder on a real mug (YCB): click its side", (1.0, 0.5, 0.5))
    every(d, "Area", skip=("Brush", "What is selected"))
    sweep(d, "Scan tolerance", [0.2, 0.5, 1.5], unit=" mm")


@shot("scan-tour/survey", "clip", setup=scan_open(MUG, "m"))
def tour_survey(d):
    """Survey Scan on the mug, then each next-step button it offers, one at a time."""
    tour("scan.survey")(d)
    tour_step(d, "Survey Scan: size, noise, holes, loose pieces", change=False, hold=3.0)
    explore(d)


def tour_plain(command, panel=None, first="", ok="OK", click=None, skip=(), skip_options=(), sweeps=(), then=None):
    """A tool's tour: open it, (click), every drop-down and check box, the sweeps, OK."""

    def run(d):
        tour(command, panel)(d)
        if click:
            tour_click(d, first, click)
        else:
            tour_step(d, first, change=False, hold=2.5)
        explore(d, skip=skip, skip_options=skip_options)
        for label, values, unit in sweeps:
            sweep(d, label, values, unit=unit)
        if then:
            then(d)
        if ok:
            tour_ok(d, ok)

    return run


AREA_BY_HAND = ("Brush", "What is selected")

shot("scan-tour/clean-up", "clip", setup=scan_open(MUG, "m"))(tour_plain("scan.clean_up", first="Clean Up on a real mug: cracks, specks and holes", ok="OK: each tool its own step"))
shot("scan-tour/repair", "clip", setup=scan_open(MUG, "m", camera={"distance": 230}))(tour_plain("scan.repair", "panel:SCAN:CLEAN", first="Repair Mesh on the mug: points written twice a hair apart"))
shot("scan-tour/floating", "clip", setup=scan_open(MUG, "m"))(tour_plain("scan.remove_floating", first="Remove Floating Pieces on the mug: the specks marked"))
shot("scan-tour/fill-holes", "clip", setup=scan_open(BUNNY, "m", upright=True, camera={"pitch": -0.45}))(tour_plain("scan.fill_holes", first="Fill Holes on the bunny from below: five holes in its base"))
shot("scan-tour/reduce", "clip", setup=scan_open(TLESS5, style="shaded_with_edges"))(
    tour_plain("scan.reduce", "panel:SCAN:CLEAN", first="Reduce Mesh on T-LESS 5 (Kinect scan)", skip_options=AREA_BY_HAND))
shot("scan-tour/denoise", "clip", setup=scan_open(TLESS5, camera={"distance": 120}))(
    tour_plain("scan.denoise", "panel:SCAN:CLEAN", first="Denoise on T-LESS 5: Kinect ripples, sharp edges", skip_options=AREA_BY_HAND))
shot("scan-tour/remesh", "clip", setup=scan_open(TLESS5, camera={"distance": 160}))(
    tour_plain("scan.remesh", "panel:SCAN:CLEAN", first="Remesh on T-LESS 5", skip_options=AREA_BY_HAND))
shot("scan-tour/mirror", "clip", setup=scan_open(TLESS5, then=ONE_SIDED))(
    tour_plain("scan.complete_by_mirroring", "panel:SCAN:CLEAN", first="Complete by Mirroring: T-LESS 5 with its end past X = 15 cut away"))
shot("scan-tour/points", "clip", setup=cloud_open)(tour_plain("scan.mesh_from_points", "panel:SCAN:CLEAN", first="Mesh from Points: one raw pass of the bunny"))
shot("scan-tour/remove-markers", "clip", setup=markers_plate)(
    tour_plain("scan.remove_markers", "panel:SCAN:CLEAN", first="Remove Markers: click one sticker dot", click=(35, -20, 14)))


def tour_select_with(tools, find_all=False):
    """Select on Scan: each of `tools` ({Tool: (caption, action(d))}) used once, its options gone
    through, then Grow, Shrink, Invert and Clear."""

    def run(d):
        if find_all:
            d.tool("scan.find_all")
            for _ in range(240):
                if not d.ui("ui.inspect").get("dialog"):
                    break
                time.sleep(0.25)
        tour("scan.select")(d)
        tour_step(d, "Select on Scan", change=False)
        for t, (caption, act) in tools.items():
            tour_step(d, f"Tool: {t}", lambda t=t: choose(d, "dialog:Tool", t), change=False, hold=0.8)
            tour_step(d, f"{t}: {caption}", lambda act=act: act(d))
            explore(d, skip=("Tool", "Selection", "Name"))
        for b in ("Grow", "Shrink", "Invert", "Clear"):
            if d.handle(f"dialog:button::{b}"):
                tour_step(d, b, lambda b=b: d.click(d.at(handle=f"dialog:button::{b}"), dur=0.4, settle=0.6))

    return run


def clicks_at(*fs):
    def act(d):
        for f in fs:
            d.click(screen_of(d, box_point(d, *f)), dur=0.4, settle=0.25)

    return act


def stroke(a, b):
    return lambda d: d.drag(screen_of(d, box_point(d, *a)), screen_of(d, box_point(d, *b)), steps=50)


HEAD = [(0.15, 0.5, 0.55), (0.55, 0.5, 0.55), (0.55, 0.5, 0.95), (0.15, 0.5, 0.95), (0.15, 0.5, 0.55)]

# Strokes and outlines on the bunny (organic: what a brush or an outline is for).
shot("scan-tour/select", "clip", setup=scan_open(BUNNY, "m", upright=True))(tour_select_with({
    "Brush": ("drag along the side", stroke((0.8, 0.5, 0.35), (0.3, 0.5, 0.6))),
    "Polygon": ("corners round the head, the first again", clicks_at(*HEAD)),
    "Lasso": ("drag across the head", stroke((0.1, 0.5, 0.6), (0.55, 0.5, 0.95))),
}))
# Region and Shape on the holes plate (creases and fitted shapes: what they stop at).
shot("scan-tour/select-region", "clip", setup=scan_open(PLATE))(tour_select_with({
    "Region": ("click the top: grows to its creases", lambda d: click_scan(d, (15, 0, 12))),
    "Shape": ("click a counterbore wall: that shape", lambda d: click_scan(d, (-35 - 3.9, -15 + 3.9, 10))),
}, find_all=True))


@shot("scan-tour/split", "clip", setup=scan_open(BUNNY, "m", upright=True))
def tour_split(d):
    """Split Mesh by the origin's YZ plane: every Keep, Envelope sizes, OK."""
    tour("scan.split", "panel:SCAN:CLEAN")(d)

    def pick():
        d.click(d.at(handle="fold:Origin"), dur=0.6, settle=0.4)
        d.click(d.at(handle="row:YZ"), dur=0.5, settle=0.4)

    tour_step(d, "Split Mesh: the origin's YZ plane cuts the bunny", pick, change=False)
    explore(d)
    sweep(d, "Envelope", [5, 20, 0], unit=" mm")
    tour_ok(d, "OK: the pieces as scans of their own")


@shot("scan-tour/stand-on-base", "clip", setup=scan_open(TLESS5, then=TILTED))
def tour_stand_on_base(d):
    tour_plain("scan.stand_on_base", first="Stand on Base: T-LESS 5 lying tilted", ok="OK: on its steadiest base")(d)
    d.ui("ui.view", {"view": "fit"})
    d.tool("scan.stand_on_base")
    tour_step(d, "Pressed again: the next base", change=False)
    tour_ok(d, "OK: on the next base")


@shot("scan-tour/align", "clip", setup=scan_open(TLESS5, then=TILTED))
def tour_align(d):
    """Align: each of its buttons, Quick Align first."""
    tour("scan.align")(d)
    tour_step(d, "Align: the ways to lay a scan", change=False)
    explore(d)
    tour_step(d, "Quick Align", lambda: d.click(d.at(handle="dialog:button::Quick Align"), dur=0.6, settle=0.6), change=False)
    if d.ui("ui.inspect").get("dialog"):
        tour_ok(d, "OK")
    d.ui("ui.view", {"view": "fit"})
    tour_step(d, "Squared to its faces, longest along X, on the grid", change=False, hold=2.5)


shot("scan-tour/symmetry", "clip", setup=scan_open(TLESS5, then=TURNED))(
    tour_plain("scan.symmetry", "panel:SCAN:ALIGN", first="Find Symmetry on T-LESS 5 turned 28 degrees", ok="OK: as On OK said"))
shot("scan-tour/align-all", "clip", setup=three_passes)(
    tour_plain("scan.align", first="Three raw passes of the bunny", ok=None,
               then=lambda d: (tour_step(d, "Align All Scans", lambda: d.click(d.at(handle="dialog:button::Align All Scans"), dur=0.6, settle=0.6), change=False), explore(d), tour_ok(d, "OK: the passes together"))))
shot("scan-tour/combine", "clip", setup=three_passes)(tour_plain("scan.combine", "panel:SCAN:ALIGN", first="Combine: three passes into one scan"))
shot("scan-tour/find-all", "clip", setup=scan_open(PLATE))(tour_plain("scan.find_all", first="Find All on the holes plate", ok=None))
shot("scan-tour/rebuild", "clip", setup=scan_open(PLATE))(tour_plain("scan.reconstruct", first="Rebuild the holes plate: the steps that build it", ok="OK: features in the timeline"))
shot("scan-tour/radius", "clip", setup=scan_open(PLATE))(tour_plain("scan.radius", "panel:SCAN:RECONSTRUCT", first="Radius from Scan: the corner round (R5)", click=(48.54, 28.54, 6), ok=None))
shot("scan-tour/fillet", "clip", setup=plate_and_model())(tour_plain("scan.fillet", "panel:SCAN:RECONSTRUCT", first="Fillet from Scan: the model's sharp corner, the scan rounded R5", click=(50, -30, 6)))


@shot("scan-tour/free-form", "clip", setup=scan_open(HOUSING, origin=False))
def tour_free_form(d):
    """New Free Form over the housing's top: four corners, then every option."""
    tour("scan.free_form")(d)
    # Its options first: a new Points choice starts the corners again.
    explore(d)
    for i, at in enumerate(((-30, -20, 30), (30, -20, 30), (30, 20, 30), (-30, 20, 30)), 1):
        tour_click(d, f"New Free Form: corner {i} of the housing's top", at)
    tour_ok(d, "OK: kept, Edit Surface opens on it")


shot("scan-tour/wrap", "clip", setup=scan_open(PIPE, origin=False))(
    tour_plain("scan.wrap_surface", first="Wrap Surface: the pipe's bar", click=(5.66, -5.66, 25), sweeps=[("Arc", [180, 360], " deg"), ("Stiffness", [5, 40], "")]))


@shot("scan-tour/wrap-hammer", "clip", setup=scan_open(HAMMER, "m"))
def tour_wrap_hammer(d):
    """Wrap Surface round a real hammer's handle (YCB): every option."""
    tour("scan.wrap_surface")(d)
    lo, hi = scan_box(d)
    f = [0.5, 0.5, 0.5]
    f[max(range(3), key=lambda i: hi[i] - lo[i])] = 0.75
    tour_box_click(d, "Wrap Surface on a real hammer's handle (YCB)", tuple(f))
    explore(d)


shot("scan-tour/fit-patches", "clip", setup=scan_open(BLADE, origin=False))(
    tour_plain("scan.fit_patches", first="Fit Patches: the twisted blade", click=(15, 4, 50), skip_options=AREA_BY_HAND))
shot("scan-tour/fit-surface", "clip", setup=scan_open(BLADE, origin=False))(
    tour_plain("scan.fit_surface", "panel:SCAN:FREEFORM", first="Fit Surface: the blade round the click", click=(15, 4, 50), skip_options=AREA_BY_HAND, sweeps=[("Radius", [10, 30], " mm")]))
shot("scan-tour/make-flat", "clip", setup=scan_open(HOUSING, origin=False, then=HOUSING_FACE))(tour_plain("scan.make_flat", "panel:SCAN:FREEFORM", first="Make Flat: the housing's fitted top"))


@shot("scan-tour/slice", "clip", setup=scan_open(HOUSING, origin=False, then=HOUSING_FACE))
def tour_slice(d):
    """Slice the housing's fitted top with the YZ plane: every option."""
    tour("scan.slice_surface", "panel:SCAN:FREEFORM")(d)
    tour_click(d, "Slice: click the fitted top", (10, 0, 30))

    def pick():
        d.click(d.at(handle="fold:Origin"), dur=0.6, settle=0.4)
        d.click(d.at(handle="row:YZ"), dur=0.5, settle=0.4)

    tour_step(d, "the origin's YZ plane cuts it", pick, change=False)
    explore(d)
    tour_ok(d, "OK: the two pieces")


shot("scan-tour/section", "clip", setup=scan_open(BLOCK, origin=False))(
    tour_plain("scan.section_sketch", first="Cut to Sketch: the block (pocket, slot, holes, bevel)", sweeps=[("Offset", [5, 20, 35], " mm")]))
shot("scan-tour/flatten", "clip", setup=scan_open(HOUSING, origin=False))(
    tour_plain("scan.flatten", "panel:SCAN:SKETCH", first="Flatten: the housing's rounded corner", click=(37.72, -27.72, 15), skip_options=AREA_BY_HAND))
shot("scan-tour/sections-along-curve", "clip", setup=blade_with_curve)(tour_plain("scan.sections_along_curve", "panel:SCAN:SKETCH", first="Sections Along Curve: up the blade", sweeps=[("Count", [3, 8], "")]))
shot("scan-tour/deviation", "clip", setup=plate_and_model())(tour_plain("scan.deviation", first="Scan Deviation: the plate against the sharp-cornered model", ok=None))
shot("scan-tour/align-to-model", "clip", setup=plate_and_model(moved=True))(tour_plain("scan.align_to_model", first="Align to Model: the scan 12 degrees and 11 mm off"))
shot("scan-tour/flat-template", "clip", setup=van_open)(tour_plain("scan.flat_template", first="Flat Template: a van's side wall (10.7M triangles)", ok=None))


# scan-e2e: scan to CAD end to end, as a user drives it: the scan read into shapes, Rebuild's
# steps built as features, the solid looked at alone, then checked against the scan.
RIM = CORPUS / "cars/drivaerml/scan/rim_left_scan.stl"


def eye(d, name):
    """Show or hide the body `name` with its eye in the browser."""
    handle = next((h["name"] for h in d.ui("ui.handles") if h["name"].startswith(f"eye:{name}:")), None)
    if handle is None:
        raise RuntimeError(f"no eye for {name}")
    hidden = lambda: name in json.dumps((d.ui("ui.inspect").get("visibility") or {}))
    was = hidden()
    d.click(d.at(handle=handle), dur=0.6, settle=1.0)
    if hidden() == was:
        # The click didn't take: the same change as the browser's eye makes.
        d.execute("browser.visibility", {"items": [name], "visible": was})
    d.settle_camera()


def no_dialog(d, timeout=240):
    """Wait until a tool that closes itself (Find All, Scan Deviation) has done."""
    for _ in range(timeout * 4):
        if not d.ui("ui.inspect").get("dialog"):
            break
        time.sleep(0.25)
    d.settle_camera()


def e2e(scan, what, first, edit=None, untick=()):
    """Find All, Rebuild and OK, the solid alone, then Scan Deviation; `edit` (caption, parameter's
    feature, its input, new value) changes one reconstructed size and checks again."""

    def run(d):
        d.caption(f"A scan of {what}: only triangles, no features yet")
        d.wait(2.5)
        d.caption("Find All reads the scan into planes, cylinders, cones and freeform")
        d.tool("scan.find_all")
        no_dialog(d)
        d.wait(2.5)
        d.caption(first)
        d.tool("scan.reconstruct")
        dialog_ready(d, timeout=600)
        d.wait(3.5)
        for row, why in untick:
            d.caption(why)
            # Reconstruct's step boxes have no name of their own: the row'th unnamed one.
            boxes = [(h["x"], h["y"]) for h in d.ui("ui.handles") if h["name"] == "dialog:"]
            d.click(boxes[row], dur=0.5, settle=0.6)
            dialog_ready(d)
            d.wait(2.0)
        d.caption("OK builds each step as a feature: sketches dimensioned, sizes as parameters")
        ok_and_wait(d, hold=1.5)
        d.caption("The scan hidden: the solid, with its steps in the timeline")
        eye(d, scan)
        d.ui("ui.view", {"view": "fit"})
        d.settle_camera()
        d.wait(3.5)
        eye(d, scan)
        d.caption("Scan Deviation: the scan coloured by how far it is from the solid")
        d.tool("scan.deviation")
        no_dialog(d)
        d.wait(4.0)
        if edit:
            caption, feature, field, value = edit
            listed = d.execute("parameters.list", {})
            params = (listed or {}).get("parameters") or ((listed or {}).get("result") or {}).get("parameters") or []
            name = next((p["name"] for p in params if p.get("feature") == feature and p.get("input") == field), None)
            if name is None:
                raise RuntimeError(f"no parameter for {feature} {field}")
            d.caption(caption)
            d.execute("parameters.change", {"name": name, "expression": value})
            d.wait(1.5)
            d.execute("scan.deviation", {"body": f"{scan} Reconstructed", "scan": scan})
            d.wait(4.5)

    return run


shot("scan-e2e/holes-plate", "clip", setup=scan_open(PLATE, origin=False))(
    e2e("holes", "a plate with six holes", "Rebuild lists the steps that build it: one extrude, six holes of their kinds",
        edit=("Hole It 6's diameter is a parameter: 8 mm made 12, the scan shows where it no longer fits", "Hole It 6", "Diameter", "12 mm")))
shot("scan-e2e/housing", "clip", setup=scan_open(HOUSING, origin=False))(
    e2e("housing", "a drafted, shelled housing", "Rebuild lists the steps: an extrude with its draft, the shell, the fillets"))
shot("scan-e2e/wheel", "clip", setup=scan_open(RIM, origin=False))(
    e2e("rim_left_scan", "a car's wheel rim (DrivAerML, 1.2M triangles)", "Rebuild reads a wheel: turned about its axis, 5 spokes; Revolve It cuts the openings",
        untick=[(1, "Fillet It unticked: none of its rounds fits better than a sharp edge, and it would stop OK")]))


@shot("scan-tour/curve", "clip", setup=scan_open(BLADE, origin=False))
def tour_curve(d):
    """Curve on Scan up the blade: points clicked, then every option."""
    tour("scan.curve_on_scan")(d)
    for z in (12, 37, 62):
        tour_click(d, f"Curve on Scan: a point at Z = {z}", (15, 4, z))
    tour_click(d, "and the last at Z = 88", (15, 4, 88))
    explore(d)
    tour_ok(d, "OK: a 3D curve on the scan")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("shots", nargs="*", help="shot names, or a page (ui-tour); default every shot")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--app", default=str(Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target") / "release/solidcraft"))
    ap.add_argument("--display", default=":97", help="the Xvfb display to start (or use, with --no-xvfb)")
    ap.add_argument("--no-xvfb", action="store_true", help="use a display that is already running")
    ap.add_argument("--keep", action="store_true", help="keep each shot's work folder")
    ap.add_argument("--light", action="store_true", help="the stills again in the light theme, as NAME.light.png")
    ap.add_argument("--check", metavar="DIR", help="check, don't publish: clips go to DIR with a contact sheet NAME.png each (and NAME.failed.png when a shot fails)")
    ap.add_argument("-j", "--jobs", type=int, default=1, help="shots at once, each on its own Xvfb screen (clips only)")
    a = ap.parse_args()
    if a.list:
        for name, (kind, start, f, _) in SHOTS.items():
            print(f"{name:32} {kind:5} {start:6} {(f.__doc__ or '').split(chr(10))[0]}")
        return
    names = [n for n in SHOTS if not a.shots or n in a.shots or n.split("/")[0] in a.shots]
    if a.light:
        names = [n for n in names if SHOTS[n][0] == "still"]
    if a.shots and not names:
        sys.exit(f"no such shot: {' '.join(a.shots)} (--list)")
    global CHECK
    if a.check:
        CHECK = Path(a.check).resolve()
        CHECK.mkdir(parents=True, exist_ok=True)
        stale(Path(a.app))
    if a.jobs > 1 and len(names) > 1 and not a.light:
        sys.exit(check_parallel(names, a))
    xvfb = None
    if not a.no_xvfb:
        xvfb = subprocess.Popen(["Xvfb", a.display, "-screen", "0", f"{SIZE[0]}x{SIZE[1]}x24", "-nolisten", "tcp"], stderr=subprocess.DEVNULL)
        time.sleep(1.0)
    try:
        for n in names:
            print(f"{n} ...", file=sys.stderr)
            run_shot(n, a.app, a.display, a.keep, a.light)
    finally:
        if xvfb:
            xvfb.terminate()


def stale(app):
    """Say so when the app checked is older than the newest Rust source: a check of an old build
    looks at code that is not there any more."""
    try:
        built = app.stat().st_mtime
    except OSError:
        sys.exit(f"{app}: no such app (build it first)")
    newest = max((f.stat().st_mtime, f) for f in (ROOT / "crates").rglob("*.rs"))
    if newest[0] > built:
        print(f"warning: {app} is older than {newest[1].relative_to(ROOT)}: build it again, or the check shows old code", file=sys.stderr)


# Ranges ("scan-range/<tool>"): one tool on scan after scan, from the clean family parts (where the
# answer is known) to real scans (noisy, open, organic), each captioned with what it should show:
# what the tool can do and where it stops. The tools with few options (Survey, Find All, Rebuild,
# Radius, Deviation, Quick Align) say little in a tour; across scans they say more. Each scan goes
# in beside the last, the last hidden, so the tool takes the one shown. With --check the readout on
# each scan goes to NAME.tour.txt.

GEAR = CORPUS / "families/scanlike/gear.ply"
BRACKET = CORPUS / "families/scanlike/bracket.ply"
TLESS = CORPUS / "tless"
T2 = 0.03492076949174773  # tan 2 degrees: the housing's draft


def range_case(path, caption, units="mm", origin=True, upright=False, then=(), model=(), cad=None, click=None, camera=None):
    """One scan of a range: inserted (with `model` commands and a `cad` mesh beside it, in the
    scan's own frame when `origin` is False), the tool used on it, `caption` saying what to see."""
    return {"path": path, "caption": caption, "units": units, "origin": origin, "upright": upright, "then": then, "model": model, "cad": cad, "click": click, "camera": camera}


def range_bodies(d):
    return {b["name"] for b in d.ui("engine.execute", {"command": "inspect.measure", "params": {}}).get("bodies") or []}


def readout_in(d, timeout=90):
    """Wait until the open dialog shows its readout rows (Survey reads the scan beside the window,
    saying so meanwhile), or it has closed."""
    for _ in range(timeout * 4):
        st = tour_state(d)
        if st.get("closed") or st.get("error") or st.get("rows"):
            return
        time.sleep(0.25)


def close_dialog(d):
    if d.ui("ui.inspect").get("dialog"):
        d.click(d.at(handle="dialog:cancel"), dur=0.5, settle=0.8)


def scan_range(command, panel, cases, use=None, hold=3.5):
    """The tool `command` on each of `cases` in turn. `use(d, case)` uses it (default: its button,
    the case's click, wait for the dialog to settle); the readout is noted and the dialog closed."""

    def run(d):
        d.tour = []
        shown = set()
        for case in cases:
            # Captioned first: a step's still is its last frame, before the next scan goes in.
            d.caption(case["caption"])
            if shown:
                close_dialog(d)
                d.execute("browser.visibility", {"items": sorted(shown), "visible": False})
            before = range_bodies(d)
            for command_, params in case["model"]:
                d.execute(command_, params)
            if case["cad"]:
                d.execute("file.insert_mesh", {"path": str(case["cad"]), "units": case["units"]})
            path = case["path"]
            if not path.is_file():
                raise RuntimeError(f"{path}: not found (set SCAN_CORPUS to the scan corpus folder)")
            made = d.execute("file.insert_mesh", {"path": str(path), "units": case["units"], "origin": case["origin"]})
            body = ((made or {}).get("bodies") or [path.stem])[0]
            d.scan = body
            if case["upright"]:
                d.execute("solid.move", {"bodies": [body], "axis": [1, 0, 0], "angle": "90 deg"})
                d.execute("scan.rest_on_grid", {"body": body})
            for command_, params in case["then"]:
                d.execute(command_, json.loads(json.dumps(params).replace("{body}", body)))
            shown = range_bodies(d) - before
            d.ui("ui.workspace", {"workspace": "scan"})
            d.ui("ui.view", {"view": "iso"})
            d.ui("ui.view", {"view": "fit"})
            if case["camera"]:
                d.ui("ui.camera", case["camera"])
            d.wait(1.0)
            d.settle_camera()
            if use:
                use(d, case)
            else:
                d.tool(command, panel=panel)
                d.settle_camera()
                if case["click"]:
                    click_scan(d, case["click"])
            readout_in(d)
            tour_step(d, case["caption"], change=False, hold=hold)
        close_dialog(d)

    return run


shot("scan-range/survey", "clip")(scan_range("scan.survey", None, [
    range_case(PLATE, "Survey, the holes plate: little noise, nine pinholes (1.8 mm at most); next: Find All"),
    range_case(MUG, "Survey, a real mug (YCB): cracks, holes and specks; next: Repair, Fill, Floating", units="m"),
    range_case(BUNNY, "Survey, the bunny: five holes in its base, organic; next: Fill Holes", units="m", upright=True),
    range_case(TLESS5, "Survey, T-LESS 5 (Kinect): its noise read, holes counted"),
    range_case(HAMMER, "Survey, a hammer (YCB): long and thin, noisy, open", units="m"),
    range_case(HELMET, "Survey, a helmet (Smithsonian): a thin open shell, fine texture scan", units="m", upright=True),
]))

def find_all_use(d, case):
    """Find All's button, then time for it to read the scan beside the window (no dialog says when
    it is done; these scans, under 600k triangles, take a few seconds)."""
    # The button shows the colouring or takes it away: off first, so each press shows it.
    d.execute("scan.find_all", {"show": False})
    d.tool("scan.find_all")
    d.wait(12.0)
    d.settle_camera()


shot("scan-range/find-all", "clip")(scan_range("scan.find_all", None, [
    range_case(PLATE, "Find All, the holes plate: planes, hole walls (cylinders), countersinks (cones)"),
    range_case(BLOCK, "Find All, the block: planes, the pocket, slot, holes and the bevel"),
    range_case(HOUSING, "Find All, the housing: drafted walls, R6 corners, R3 rim, 2 mm shell"),
    range_case(GEAR, "Find All, the gear: involute teeth are freeform, the bore a cylinder"),
    range_case(BRACKET, "Find All, the Z bracket: flats and R2 bends (cylinders)"),
    range_case(TLESS5, "Find All, T-LESS 5 (Kinect): real noise, the same shapes harder to tell"),
    range_case(MUG, "Find All, the mug: its body a cylinder, the handle freeform", units="m"),
    range_case(BUNNY, "Find All, the bunny: organic, cut into many small patches rather than read as freeform (its limit)", units="m", upright=True),
], use=find_all_use))

shot("scan-range/rebuild", "clip")(scan_range("scan.reconstruct", None, [
    range_case(PLATE, "Rebuild, the holes plate: a plate, round corners, six holes of four kinds"),
    range_case(BLOCK, "Rebuild, the block: stock, pocket, slot, two holes, bevel"),
    range_case(HOUSING, "Rebuild, the housing: a drafted, shelled box with rounds"),
    range_case(GEAR, "Rebuild, the gear: an extruded profile with a bore"),
    range_case(BRACKET, "Rebuild, the Z bracket: a bent sheet"),
    range_case(TLESS5, "Rebuild, T-LESS 5: a real scan of a machined part (where it stops)"),
], hold=5.0))

def radius_use(d, case):
    """Radius from Scan stays on, its button a switch: pressed for the first scan only, then each
    scan clicked."""
    if not getattr(d, "radius_on", False):
        d.tool("scan.radius", panel="panel:SCAN:RECONSTRUCT")
        d.settle_camera()
        d.radius_on = True
    click_scan(d, case["click"])


XM, YM = 40 - 15 * T2, 30 - 15 * T2
shot("scan-range/radius", "clip")(scan_range("scan.radius", "panel:SCAN:RECONSTRUCT", [
    range_case(PLATE, "Radius, the plate's corner: R5", origin=False, click=(45 + 5 * 0.7071, 25 + 5 * 0.7071, 6)),
    range_case(HOUSING, "Radius, the housing's corner: R6", origin=False, click=(XM - 6 + 6 * 0.7071, YM - 6 + 6 * 0.7071, 15)),
    range_case(HOUSING, "Radius, the housing's top rim: R3", origin=False, click=(0, 30 - 30 * T2 - 3 + 3 * 0.7071, 27 + 3 * 0.7071)),
    range_case(PIPE, "Radius, the pipe's bar (16 mm across): R8, a bend of R30", origin=False, click=(30 - 30 * 0.7071 - 8 * 0.7071, 0, 50 + 30 * 0.7071 + 8 * 0.7071)),
], use=radius_use))


def deviation_use(d, case):
    # The button shows the colouring or takes it away: off first, so each press shows it.
    d.execute("scan.deviation", {"show": False})
    d.tool("scan.deviation")
    d.settle_camera()
    for _ in range(240):
        if not d.ui("ui.inspect").get("dialog"):
            break
        time.sleep(0.25)
    d.settle_camera()


shot("scan-range/deviation", "clip")(scan_range("scan.deviation", None, [
    range_case(PLATE, "Deviation, the plate against its model with sharp corners: off only at the corners", origin=False, model=PLATE_MODEL),
    range_case(TLESS / "models_reconst/obj_000005.ply", "Deviation, T-LESS 5's scan against its CAD: the scanner's own error", origin=False, cad=TLESS / "models_cad/obj_000005.ply"),
    range_case(TLESS / "models_reconst/obj_000007.ply", "Deviation, T-LESS 7 against its CAD", origin=False, cad=TLESS / "models_cad/obj_000007.ply"),
    range_case(PLATE, "Deviation, the plate 12 degrees and 11 mm off its model: not aligned, it all reads off (align first)", origin=False, model=PLATE_MODEL,
               then=[("solid.move", {"bodies": ["{body}"], "axis": [0, 0, 1], "angle": "12 deg", "translate": [8, -5, 6]})]),
], use=deviation_use, hold=4.0))


def quick_align_use(d, case):
    d.tool("scan.align")
    d.settle_camera()
    d.click(d.at(handle="dialog:button::Quick Align"), dur=0.6, settle=0.6)
    if d.ui("ui.inspect").get("dialog"):
        tour_settle(d)
        ok_through(d, hold=0.5)
    d.ui("ui.view", {"view": "fit"})
    d.settle_camera()


shot("scan-range/quick-align", "clip")(scan_range("scan.quick_align", None, [
    range_case(TLESS5, "Quick Align, T-LESS 5 lying tilted: squared, longest along X, on the grid", then=TILTED),
    range_case(HOUSING, "Quick Align, the housing tilted: its box squared to the grid", then=TILTED),
    range_case(MUG, "Quick Align, the mug tilted: its axis is its longest way", units="m", then=TILTED),
    range_case(HAMMER, "Quick Align, the hammer tilted: handle along X", units="m", then=TILTED),
    range_case(BUNNY, "Quick Align, the bunny tilted: organic, its directions loosely held", units="m", then=TILTED),
], use=quick_align_use))


def check_parallel(names, a):
    """Each shot in a child of its own on its own screen, a.jobs at once; 1 when any failed."""
    import queue
    from concurrent.futures import ThreadPoolExecutor

    base = int(a.display.lstrip(":"))
    screens = queue.Queue()
    for k in range(a.jobs):
        screens.put(f":{base + 1 + k}")

    def one(name):
        screen = screens.get()
        try:
            argv = [sys.executable, __file__, name, "--app", a.app, "--display", screen] + (["--check", a.check] if a.check else [])
            r = subprocess.run(argv, capture_output=True, text=True)
        finally:
            screens.put(screen)
        lines = (r.stdout + r.stderr).strip().splitlines()
        tail = "\n".join([l for l in lines[:-3] if "!!" in l or l.strip().startswith("--")] + lines[-3:])
        print(f"{'ok' if r.returncode == 0 else 'FAILED'} {name}\n{tail}\n", flush=True)
        return r.returncode == 0

    with ThreadPoolExecutor(a.jobs) as pool:
        results = list(pool.map(one, names))
    return 0 if all(results) else 1


if __name__ == "__main__":
    main()
