#!/usr/bin/env python3
"""Screenshot every screen of the launcher at several screen sizes, for visual review.

Runs the real launcher fullscreen under a real window manager (xfwm4) on hidden X displays,
with a throwaway home folder seeded from your cache (no network wait, nothing personal).

    scripts/ui_matrix.py                      # all sizes
    scripts/ui_matrix.py 1280x720 3440x1440   # just these

Writes dist/ui-matrix/<size>/<screen>.png and a contact sheet per size.
Needs: Xvfb, xfwm4, xdotool, ImageMagick.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from ui_fixture import NO_UPDATES, install_game, seed_cache

ROOT = Path(__file__).resolve().parent.parent
SIZES = ["1280x720", "1366x768", "1920x1080", "1920x1200", "2560x1440", "3440x1440", "3840x2160", "1280x800"]

# (screen name, keys to press before the shot)
STEPS = [
    ("00-notification", []),
    ("01-home", ["sleep:9"]),
    ("02-home-new-game", ["Right", "Right"]),
    ("03-hub", ["Down", "Right", "Return", "sleep:1.5"]),   # the "Game Hub" button (never Play)
    ("04-hub-scrolled", ["Down", "Down", "sleep:0.8"]),
    ("05-library", ["Escape", "Escape", "Up", "Up", "Right", "Return", "sleep:1.5"]),
    ("06-library-down", ["Down", "Down", "Down", "sleep:1"]),
    ("07-search", ["slash", "r", "e", "s", "i", "sleep:1"]),
    ("08-menu", ["Escape", "Escape", "sleep:0.5", "Escape", "sleep:0.8", "o", "sleep:0.6"]),
    ("09-settings", ["Escape", "Escape", "Escape", "sleep:0.5", "s", "sleep:0.8"]),
    ("10-settings-bottom", ["End"] + ["Down"] * 22 + ["sleep:0.8"]),
    ("11-toasts", ["Escape", "sleep:0.5", "Home", "Down", "Right", "Right", "Right", "Return", "sleep:0.4",
                   "Up", "Right", "Right", "Down", "Return", "sleep:0.8"]),
]


def run(cmd, **kw):
    return subprocess.run(cmd, **kw)


def shoot(size: str, n: int, out: Path, binary: Path, game: Path | None):
    w, h = size.split("x")
    disp = f":{90 + n}"
    work = Path(tempfile.mkdtemp(prefix="ps5-ui-"))
    home = work / "home"
    (home / ".config/azrak").mkdir(parents=True)
    seed_cache(home)
    install_game(home / "Games", game)
    # A stand-in emulator that exits at once: screenshots must never start a real game.
    fake = work / "kyty_emulator"
    fake.write_text("#!/bin/sh\nexit 0\n")
    fake.chmod(0o755)
    (home / ".config/azrak/config.json").write_text(json.dumps(
        {"game_dirs": ["~/Games"], "sounds": False, **NO_UPDATES, "emulator": str(fake)}))
    # No browsers or players may pop up; pretend to be an old version so the update notification shows.
    stubs = work / "stubs"
    stubs.mkdir()
    for tool in ("xdg-open", "mpv", "vlc", "ffplay", "firefox"):
        (stubs / tool).write_text("#!/bin/sh\nexit 0\n")
        (stubs / tool).chmod(0o755)
    env = {**os.environ, "DISPLAY": disp, "HOME": str(home), "PATH": f"{stubs}:{os.environ['PATH']}",
           "AZRAK_PRETEND_VERSION": "1.3.0"}
    for k in ("XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME"):
        env.pop(k, None)
    procs = [subprocess.Popen(["Xvfb", disp, "-screen", "0", f"{w}x{h}x24", "+extension", "GLX", "-noreset"],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)]
    time.sleep(1.5)
    procs.append(subprocess.Popen(["xfwm4", "--compositor=off"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
    time.sleep(1.5)
    app = subprocess.Popen([str(binary)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    procs.append(app)
    d = out / size
    d.mkdir(parents=True, exist_ok=True)
    try:
        time.sleep(3.5)
        win = run(["xdotool", "search", "--pid", str(app.pid)], env=env, capture_output=True, text=True).stdout.split()[-1]
        for name, keys in STEPS:
            for k in keys:
                if k.startswith("sleep:"):
                    time.sleep(float(k[6:]))
                    continue
                run(["xdotool", "windowfocus", win], env=env)
                run(["xdotool", "key", "--window", win, k], env=env)
                time.sleep(0.25)
            time.sleep(0.7)
            run(["import", "-window", "root", str(d / f"{name}.png")], env=env)
        pngs = sorted(str(p) for p in d.glob("[0-9]*.png"))
        run(["montage", *pngs, "-tile", "3x", "-geometry", "960x+6+6", "-background", "#333", str(out / f"sheet-{size}.png")])
        print("done", size, flush=True)
    finally:
        for p in reversed(procs):
            p.terminate()
        time.sleep(0.5)
        shutil.rmtree(work, ignore_errors=True)


def main():
    sizes = sys.argv[1:] or SIZES
    binary = ROOT / "target/release/azrak"
    out = ROOT / "dist/ui-matrix"
    out.mkdir(parents=True, exist_ok=True)
    game = Path(os.environ["DEMO_GAME"]) if os.environ.get("DEMO_GAME") else None
    for i, s in enumerate(sizes):
        shoot(s, i, out, binary, game)


if __name__ == "__main__":
    main()
