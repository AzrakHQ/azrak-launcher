#!/usr/bin/env python3
"""Record the README demo: a short looping WebP and a captioned walkthrough video.

The real launcher runs on a hidden virtual display (Xvfb) with a throwaway home folder, so
nothing personal appears on screen, and a script drives it with key presses. Captions are
timed from the script itself, so re-running this after UI changes regenerates everything.

    scripts/record_demo.py --game ~/Games/PS5/PPSA02929-app0

Needs: Xvfb, xdotool, ffmpeg (libx264, libwebp, libass), ImageMagick (`convert`) for the poster.
Outputs (default: dist/demo/):
    demo-loop.webp            ~20 s loop for the top of the README
    walkthrough.mp4           full quality captioned walkthrough (1920x1200)
    walkthrough-small.mp4     1600x1000 copy under 10 MB, for uploading to the README on github.com
    walkthrough-poster.jpg    thumbnail for linking the video
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from ui_fixture import CACHE_SKIP, install_game

ROOT = Path(__file__).resolve().parent.parent
FONTS = ROOT / "assets" / "fonts"
DISPLAY = ":77"
W, H, BAND = 1920, 1080, 120
FPS = 30


def log(*a):
    print("[demo]", *a, flush=True)


def run(cmd, **kw):
    return subprocess.run(cmd, check=True, **kw)


class Session:
    """The launcher on a virtual display, plus helpers to drive and record it."""

    def __init__(self, binary: Path, game: Path | None, work: Path):
        self.work = work
        self.home = work / "home"
        self.log_path = work / "launcher.log"
        self.binary = work / "bin" / "azrak"  # copied: keeps the demo away from real installs
        self.binary.parent.mkdir(parents=True)
        shutil.copy2(binary, self.binary)
        cfg = self.home / ".config" / "azrak"
        cfg.mkdir(parents=True)
        # The first-launch scene shows KytyPS5 downloading; shadPS4 would join it, so it stays off.
        (cfg / "config.json").write_text(json.dumps({"game_dirs": ["~/Games/PS5"], "sounds": False, "shad_auto_update": False}))
        install_game(self.home / "Games" / "PS5", game)
        # Nothing outside the launcher may appear in a take: browsers, video players, etc.
        stubs = work / "stubs"
        stubs.mkdir()
        for tool in ("xdg-open", "mpv", "vlc", "ffplay", "firefox", "google-chrome", "chromium"):
            (stubs / tool).write_text("#!/bin/sh\nexit 0\n")
            (stubs / tool).chmod(0o755)
        self.env = {**os.environ, "HOME": str(self.home), "DISPLAY": DISPLAY,
                    "PATH": f"{stubs}:{os.environ.get('PATH', '')}"}
        for k in ("XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME", "SLINT_SCALE_FACTOR"):
            self.env.pop(k, None)
        self.xvfb = subprocess.Popen(["Xvfb", DISPLAY, "-screen", "0", f"{W}x{H}x24", "+extension", "GLX", "-noreset"],
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1.5)
        self.app = None
        self.rec = None
        self.t0 = 0.0
        self.events: list[tuple[float, str, str]] = []
        self.marks: dict[str, float] = {}

    # -- app
    def launch(self):
        self.logf = open(self.log_path, "a")
        self.app = subprocess.Popen([str(self.binary)], env=self.env, stdout=self.logf, stderr=self.logf)

    def window(self) -> str:
        for _ in range(100):
            out = subprocess.run(["xdotool", "search", "--pid", str(self.app.pid)], env=self.env, capture_output=True, text=True).stdout.split()
            if out:
                return out[-1]
            time.sleep(0.1)
        raise RuntimeError("launcher window did not appear")

    def log_count(self, text: str) -> int:
        return self.log_path.read_text(errors="replace").count(text) if self.log_path.exists() else 0

    def wait_log(self, text: str, timeout: float = 600, after: int = 0):
        """Wait until `text` has appeared in the log more than `after` times."""
        end = time.time() + timeout
        while time.time() < end:
            if self.log_count(text) > after:
                return
            time.sleep(0.25)
        raise RuntimeError(f"timed out waiting for: {text}")

    def restart(self):
        seen = self.log_count("home screen shown")
        self.app.terminate()
        self.app.wait(10)
        self.launch()
        self.wait_log("home screen shown", 60, after=seen)
        time.sleep(1.5)

    def key(self, *keys: str, gap: float = 0.0):
        win = self.window()
        subprocess.run(["xdotool", "windowfocus", win], env=self.env)
        for k in keys:
            subprocess.run(["xdotool", "key", "--window", win, k], env=self.env)
            if gap:
                time.sleep(gap)

    def type(self, text: str, delay_ms: int = 140):
        win = self.window()
        subprocess.run(["xdotool", "type", "--window", win, "--delay", str(delay_ms), text], env=self.env)

    # -- recording
    def record(self, out: Path):
        self.events, self.marks = [], {}
        self.rec = subprocess.Popen(
            ["ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-draw_mouse", "0", "-framerate", str(FPS),
             "-video_size", f"{W}x{H}", "-i", DISPLAY, "-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", str(out)],
            env=self.env, stdin=subprocess.PIPE)
        self.t0 = time.monotonic() + 0.15  # x11grab start-up latency

    def now(self) -> float:
        return time.monotonic() - self.t0

    def cap(self, chapter: str, text: str):
        self.events.append((self.now(), chapter, text))

    def mark(self, name: str):
        self.marks[name] = self.now()

    def stop(self) -> float:
        dur = self.now()
        self.rec.communicate(b"q", timeout=60)
        self.rec = None
        return dur

    def close(self):
        for p in (self.rec, self.app):
            if p and p.poll() is None:
                p.send_signal(signal.SIGTERM)
                try:
                    p.wait(5)
                except subprocess.TimeoutExpired:
                    p.kill()
        self.xvfb.terminate()


# ---------------------------------------------------------------------- scripted scenes

def scene_first_launch(s: Session, out: Path):
    # Start recording only once the window is drawn: the video's first frame doubles as
    # the thumbnail GitHub's player shows, so it must not be black.
    s.launch()
    s.window()
    time.sleep(0.8)
    s.record(out)
    time.sleep(1.2)
    s.cap("First launch", "The first time it opens, a welcome screen gets everything ready.")
    time.sleep(4.5)
    s.mark("fast_from")
    s.cap("First launch", "It downloads the game catalog, the official artwork and the latest KytyPS5 emulator, all at once.")
    s.wait_log("welcome screen ready")
    s.mark("fast_to")
    time.sleep(0.8)
    s.cap("First launch", "When everything is ready, press Enter, or ✕ on a controller.")
    time.sleep(3.5)
    s.key("Return")
    s.wait_log("home screen shown", 30)
    time.sleep(2.2)
    return s.stop()


def scene_tour(s: Session, out: Path, has_game: bool):
    s.record(out)
    time.sleep(0.5)
    first = "Your installed games come first, then the newest releases." if has_game else "Home shows the newest releases."
    s.cap("Home", first)
    time.sleep(4.2)
    s.cap("Home", "Moving along the row shows each game’s official artwork, logo, rating and release date.")
    s.key("Right", gap=1.25)
    s.key("Right", gap=1.25)
    s.key("Right", gap=1.25)
    time.sleep(1.8)

    s.cap("Game Hub", "Enter opens the Game Hub.")
    s.key("Down", gap=1.0)
    s.key("Return")
    time.sleep(2.6)
    s.cap("Game Hub", "It has the details, screenshots and the full description.")
    time.sleep(2.8)
    s.key("Down", gap=1.1)
    s.key("Right", gap=0.8)
    s.key("Right", gap=0.9)
    s.cap("Game Hub", "Screenshots open full screen.")
    s.key("Return", gap=2.2)
    s.key("Right", gap=1.6)
    s.key("Right", gap=1.6)
    s.key("Escape", gap=0.9)
    s.key("Escape", gap=1.3)

    s.cap("Library", "The Library has the whole catalog, more than 700 games.")
    s.key("Up", gap=0.5)
    s.key("Up", gap=0.6)
    s.key("Right", gap=0.6)
    s.key("Return")
    time.sleep(2.6)
    s.key("Down", gap=0.9)
    s.key("Down", gap=0.9)
    s.key("Right", gap=1.2)
    s.cap("Library", "Filter by genre…")
    s.key("Home", gap=0.6)  # first card, whatever the layout
    s.key("Up", gap=0.6)
    # Chips: All, (Installed), Action, … — land on Action either way.
    for _ in range(2 if has_game else 1):
        s.key("Right", gap=0.7)
    s.key("Return", gap=2.0)
    s.cap("Library", "…sort by date added, name, release date, rating or size…")
    s.key("Up", gap=0.5)
    s.key("Right", gap=0.7)
    s.key("Return", gap=0.9)
    s.key("Return", gap=0.9)
    s.key("Return", gap=2.2)
    s.cap("Library", "…or just start typing to search.")
    s.key("Left", gap=0.5)
    s.key("Return", gap=0.4)
    s.type("resident")
    time.sleep(2.2)
    s.key("Return", gap=1.2)
    s.cap("Options", "The Options menu (O, or Options on a controller) has quick actions for any game.")
    s.key("o", gap=3.6)
    s.key("Escape", gap=0.8)
    s.key("Escape", gap=1.4)

    if has_game:
        s.key("Home", gap=1.4)
        s.cap("Playing", "Installed games start with the KytyPS5 emulator.")
        time.sleep(2.2)
        s.key("Return")
        time.sleep(3.4)
        s.cap("Playing", "While a game runs, it’s marked Playing with a timer. Resume goes back to it, and Stop closes it cleanly.")
        time.sleep(3.0)
        s.key("Down", gap=1.2)
        s.key("Right", gap=1.4)
        s.key("Return")
        time.sleep(1.8)
        s.cap("Playing", "Playtime and last played are saved for every game.")
        time.sleep(3.6)

    s.cap("Settings", "Settings keeps KytyPS5 installed and up to date for you, with one-step rollback.")
    s.key("s", gap=1.0)
    s.key("Down", gap=3.8)
    s.cap("Settings", "It also has game folders, emulator options, the display, and an optional RAWG key for extra artwork.")
    for _ in range(9):
        s.key("Down", gap=0.42)
    time.sleep(2.4)
    s.key("Escape", gap=1.2)
    s.cap("Azrak Launcher", "Works with a keyboard, a mouse or a controller. Install it with one command; see the README.")
    time.sleep(4.5)
    return s.stop()


def scene_loop(s: Session, out: Path):
    """A seamless loop from a fresh start: Home → Game Hub (screenshots) → Library → PS4 games
    only → a PS4 game's Hub → Home."""
    s.restart()
    s.record(out)
    time.sleep(1.2)
    s.key("Down", gap=0.7)        # Play
    s.key("Right", gap=0.7)       # Game Hub (never Play)
    s.key("Return", gap=2.0)
    s.key("Down", gap=1.0)        # screenshots
    s.key("Right", gap=0.8)
    s.key("Escape", gap=1.0)
    s.key("Up", gap=0.4)          # tile row
    s.key("Up", gap=0.4)          # tabs
    s.key("Right", gap=0.3)
    s.key("Return", gap=1.4)      # Library
    s.key("Up", gap=0.3)          # genre chips
    s.key("Up", gap=0.5)          # status and console chips
    for _ in range(5):
        s.key("Right", gap=0.25)  # … PS4 games only
    s.key("Return", gap=1.4)
    s.key("Down", gap=0.3)
    s.key("Down", gap=0.6)        # the PS4 games
    s.key("Right", gap=0.5)
    s.key("Return", gap=2.4)      # a PS4 game's Hub: Download, shadPS4 result
    s.key("Escape", gap=0.8)
    s.key("Escape", gap=1.8)      # back Home
    return s.stop()


# ---------------------------------------------------------------------- editing

def ass_time(t: float) -> str:
    t = max(0.0, t)
    h, rem = divmod(t, 3600)
    m, sec = divmod(rem, 60)
    return f"{int(h)}:{int(m):02d}:{sec:05.2f}"


def ass_escape(s: str) -> str:
    return s.replace("\\", "\\\\").replace("{", "(").replace("}", ")")


def write_ass(path: Path, events, total: float, notes):
    """Captions in a band under the app: chapter label on top, sentence below."""
    head = f"""[Script Info]
ScriptType: v4.00+
PlayResX: {W}
PlayResY: {H + BAND}
WrapStyle: 2
ScaledBorderAndShadow: yes

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Chapter,Inter SemiBold,22,&H00F2B68F,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,3,0,1,0,0,7,0,0,0,1
Style: Caption,Inter Medium,34,&H00FBF7F5,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,7,0,0,0,1
Style: Note,Inter Medium,22,&H00A3887D,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,1,0,1,0,0,9,0,0,0,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
"""
    lines = []
    y_ch, y_cap = H + 22, H + 54
    # Chapter labels span every caption of the same chapter.
    chapters = []
    for i, (t, ch, text) in enumerate(events):
        end = events[i + 1][0] if i + 1 < len(events) else total - 0.3
        lines.append(f"Dialogue: 0,{ass_time(t)},{ass_time(end - 0.05)},Caption,,0,0,0,,{{\\pos(96,{y_cap})\\fad(160,120)}}{ass_escape(text)}")
        if chapters and chapters[-1][2] == ch:
            chapters[-1][1] = end
        else:
            chapters.append([t, end, ch])
    for n, (a, b, ch) in enumerate(chapters, 1):
        label = ch.upper()
        lines.append(f"Dialogue: 0,{ass_time(a)},{ass_time(b - 0.05)},Chapter,,0,0,0,,{{\\pos(96,{y_ch})\\fad(160,120)}}{ass_escape(label)}")
        lines.append(f"Dialogue: 0,{ass_time(a)},{ass_time(b - 0.05)},Note,,0,0,0,,{{\\pos({W - 96},{y_cap + 8})\\fad(160,120)}}{n} / {len(chapters)}")
    for a, b, text in notes:
        lines.append(f"Dialogue: 0,{ass_time(a)},{ass_time(b)},Note,,0,0,0,,{{\\pos({W - 96},{y_ch})\\fad(200,200)}}{ass_escape(text)}")
    path.write_text(head + "\n".join(lines) + "\n")


def make_poster(frame: Path, poster: Path):
    """1600 px wide still with a clear, quiet play button in the middle."""
    k = 1600 / W
    cx, cy, r = int(W / 2 * k), int(((H + BAND) / 2 - 40) * k), 58
    run(["convert", str(frame), "-resize", "1600x",
         "-fill", "rgba(5,7,15,0.72)", "-stroke", "rgba(255,255,255,0.85)", "-strokewidth", "3",
         "-draw", f"circle {cx},{cy} {cx + r},{cy}",
         "-stroke", "none", "-fill", "white",
         "-draw", f"polygon {cx - 16},{cy - 25} {cx - 16},{cy + 25} {cx + 27},{cy}",
         "-quality", "85", str(poster)])


def build(args, work: Path, clip1: Path, dur1: float, ev1, marks1, clip2: Path, dur2: float, ev2, loop: Path, out: Path):
    out.mkdir(parents=True, exist_ok=True)
    # 1) Speed up the long download wait in the first-launch clip, and say so on screen.
    a, b = marks1["fast_from"] + 1.5, marks1["fast_to"] - 0.3
    speed = max(1.0, (b - a) / 7.0)
    fast = work / "first.mkv"
    if speed > 1.2:
        run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(clip1), "-filter_complex",
             f"[0:v]trim=0:{a},setpts=PTS-STARTPTS[p0];"
             f"[0:v]trim={a}:{b},setpts=(PTS-STARTPTS)/{speed},fps={FPS}[p1];"
             f"[0:v]trim={b},setpts=PTS-STARTPTS[p2];[p0][p1][p2]concat=n=3:v=1[v]",
             "-map", "[v]", "-c:v", "libx264", "-preset", "veryfast", "-crf", "10", str(fast)])
        remap = lambda t: t if t <= a else (a + (t - a) / speed if t <= b else a + (b - a) / speed + (t - b))
        notes = [(a, a + (b - a) / speed, f"shown {round(speed)}× faster")]
    else:
        shutil.copy2(clip1, fast)
        remap = lambda t: t
        notes = []
    d1 = remap(dur1)

    # 2) One timeline: first launch, then the tour.
    events = [(remap(t), c, x) for t, c, x in ev1] + [(d1 + t, c, x) for t, c, x in ev2]
    total = d1 + dur2
    ass = work / "captions.ass"
    write_ass(ass, events, total, notes)
    joined = work / "joined.mkv"
    run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(fast), "-i", str(clip2), "-filter_complex",
         f"[0:v]fps={FPS},setsar=1[a];[1:v]fps={FPS},setsar=1[b];[a][b]concat=n=2:v=1[v]", "-map", "[v]",
         "-c:v", "libx264", "-preset", "veryfast", "-crf", "10", str(joined)])

    # 3) Caption band + gentle fades.
    vf = (f"pad={W}:{H + BAND}:0:0:color=0x05070f,"
          f"drawbox=x=0:y={H}:w={W}:h=1:color=white@0.08:t=fill,"
          f"subtitles={ass}:fontsdir={FONTS},"
          f"fade=t=out:st={total - 0.8:.2f}:d=0.8,format=yuv420p")
    full = out / "walkthrough.mp4"
    run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(joined), "-vf", vf, "-c:v", "libx264", "-preset", "slow",
         "-crf", "20", "-tune", "animation", "-movflags", "+faststart", "-an", str(full)])
    small = out / "walkthrough-small.mp4"
    # GitHub accepts README video uploads up to 10 MB: as sharp as fits.
    for crf in (26, 28, 31):
        run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(full), "-vf", "scale=1600:-2:flags=lanczos",
             "-c:v", "libx264", "-preset", "slow", "-crf", str(crf), "-tune", "animation", "-movflags", "+faststart",
             "-an", str(small)])
        if small.stat().st_size < 9.5 * 1024 * 1024:
            break

    # 4) Poster: a Home frame with a quiet play button.
    frame = work / "poster.png"
    t_home = d1 + 2.0
    run(["ffmpeg", "-loglevel", "error", "-y", "-ss", f"{t_home:.2f}", "-i", str(full), "-frames:v", "1", str(frame)])
    poster = out / "walkthrough-poster.jpg"
    make_poster(frame, poster)

    encode_loop(loop, out)
    for f in (full, small, poster):
        log(f"{f.name}: {f.stat().st_size / 1e6:.1f} MB")


def encode_loop(loop: Path, out: Path):
    """The loop: smaller and lighter, for the top of the README."""
    out.mkdir(parents=True, exist_ok=True)
    webp = out / "demo-loop.webp"
    for width, fps, q in ((1200, 24, 72), (1040, 20, 68), (960, 18, 62)):
        run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(loop), "-vf",
             f"fps={fps},scale={width}:-1:flags=lanczos", "-c:v", "libwebp_anim", "-lossless", "0",
             "-quality", str(q), "-compression_level", "6", "-loop", "0", "-an", str(webp)])
        if webp.stat().st_size < 7 * 1024 * 1024:
            break
    log(f"{webp.name}: {webp.stat().st_size / 1e6:.1f} MB")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--binary", type=Path, default=ROOT / "target" / "release" / "azrak")
    ap.add_argument("--game", type=Path, help="an installed game folder (with sce_sys/param.json) to show as installed")
    ap.add_argument("--out", type=Path, default=ROOT / "dist" / "demo")
    ap.add_argument("--keep", action="store_true", help="keep the temporary work folder")
    ap.add_argument("--loop-only", action="store_true",
                    help="only record the README loop, reusing your downloaded catalog and artwork (fast)")
    args = ap.parse_args()
    for tool in ("Xvfb", "xdotool", "ffmpeg", "convert"):
        if not shutil.which(tool):
            sys.exit(f"missing tool: {tool}")
    if not args.binary.is_file():
        sys.exit(f"launcher binary not found: {args.binary} (run `cargo build --release`)")
    if args.game and not (args.game / "sce_sys" / "param.json").is_file():
        sys.exit(f"not a game folder: {args.game}")

    work = Path(tempfile.mkdtemp(prefix="azrak-demo-"))
    log("work folder:", work)
    s = Session(args.binary, args.game, work)
    if args.loop_only:
        try:
            real = Path.home() / ".cache/azrak"
            shutil.copytree(real, s.home / ".cache/azrak", ignore=shutil.ignore_patterns(*CACHE_SKIP))
            fake = work / "kyty_emulator"                     # never start a real game
            fake.write_text("#!/bin/sh\nexit 0\n")
            fake.chmod(0o755)
            cfg = s.home / ".config/azrak/config.json"
            c = json.loads(cfg.read_text())
            c.update({"emulator": str(fake), "kyty_auto_update": False, "app_auto_update": False, "shad_auto_update": False})
            cfg.write_text(json.dumps(c))
            s.launch()
            s.wait_log("home screen shown", 60)
            time.sleep(1.5)
            log("recording: loop")
            loop = work / "loop.mkv"
            scene_loop(s, loop)
            s.close()
            encode_loop(loop, args.out)
            log("done:", args.out)
        finally:
            s.close()
            if not args.keep:
                shutil.rmtree(work, ignore_errors=True)
        return
    try:
        log("recording: first launch (downloads catalog, artwork and KytyPS5)")
        c1 = work / "c1.mkv"
        d1 = scene_first_launch(s, c1)
        ev1, m1 = s.events, s.marks
        # The demo can't run real games on a virtual display: swap in a stand-in "emulator"
        # that just waits, so Play / Playing / Stop can be shown.
        emu = s.home / ".local/share/azrak/kyty/current/kyty_emulator"
        if emu.exists():
            emu.unlink()
            emu.write_text('#!/bin/bash\nexec -a "$0" sleep 3600\n')
            emu.chmod(0o755)
        log("recording: tour")
        c2 = work / "c2.mkv"
        d2 = scene_tour(s, c2, has_game=True)  # a stand-in game when --game isn't given
        ev2 = s.events
        log("recording: loop")
        c3 = work / "c3.mkv"
        scene_loop(s, c3)
        s.close()
        log("editing")
        build(args, work, c1, d1, ev1, m1, c2, d2, ev2, c3, args.out)
        log("done:", args.out)
    finally:
        s.close()
        if not args.keep:
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
