#!/usr/bin/env python3
"""Automated UI audit: finds selected/highlighted elements that are cut off by their container.

How it works:
  * The launcher runs without artwork (AZRAK_NO_ART=1) under a real window manager on
    hidden X displays, so the only bright-white shapes on screen are interface elements:
    selected buttons, selected chips, menu items, the Start button.
  * The script walks the selection through every screen (Home, Library, Game Hub, menu,
    Settings, screenshots) at several screen sizes and captures every step.
  * Every white shape in this design has rounded ends. A white shape whose edge is a perfectly
    straight line along its full height (or width) was cut off by something - that is the bug.

    scripts/ui_audit.py                    # default sizes
    scripts/ui_audit.py 1280x720           # one size

Writes dist/ui-audit/<size>/*.png and dist/ui-audit/report.txt (with crops of each problem).
Exit code 1 if anything was found. Needs: Xvfb, xfwm4, xdotool, ImageMagick, numpy, scipy.
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

import numpy as np
from PIL import Image
from scipy import ndimage

ROOT = Path(__file__).resolve().parent.parent
SIZES = ["1280x720", "1920x1080", "2560x1440", "3440x1440", "1280x800"]

# (name, keys before the first capture, keys to step through with a capture after each)
WALK = [
    ("home-actions", [], ["Down"] + ["Right"] * 4),
    ("home-top", ["Up", "Up"], ["Right"] * 6),
    ("home-row", ["Down"], ["Right"] * 3),
    ("library-chips", ["Up", "Right", "Return", "sleep:1", "Up"], ["Right"] * 22),
    ("library-sort", ["Up"], ["Right", "Return", "Return"]),
    ("library-grid", ["Down", "Down"], ["Right"] * 8 + ["Down"] * 3 + ["Left"] * 8),
    ("hub-actions", ["Home", "Return", "sleep:1"], ["Right"] * 4),
    ("hub-shots", ["Down"], ["Right"] * 8),
    ("menu", ["Escape", "sleep:0.5", "o", "sleep:0.5"], ["Down"] * 8),
    ("settings", ["Escape", "Escape", "Escape", "sleep:0.5", "s", "sleep:0.8"], ["Down"] * 24),
]


def run(cmd, **kw):
    return subprocess.run(cmd, **kw)


def _profile_cut(comp):
    """Compare the two ends of a shape. Buttons, chips and panels are symmetric, so if one end
    comes much closer to its bounding box than the other, that end was cut off. Returns
    'left'/'right'/None."""
    rows = comp.any(axis=1)
    c = comp[rows]
    if len(c) < 8:
        return None
    w = c.shape[1]
    left = c.argmax(axis=1)
    right = c[:, ::-1].argmax(axis=1)
    d = left.astype(float) - right.astype(float)
    if d.mean() < -2.5 and (left == 0).mean() > 0.3:
        return "left"
    if d.mean() > 2.5 and (right == 0).mean() > 0.3:
        return "right"
    return None


def find_cuts(png: Path):
    """Return [(x, y, w, h, side)] of solid white shapes that were cut off by their container.

    Only solid shapes are checked (selected buttons, chips, menu rows); letters are skipped
    because they are mostly hollow.
    """
    a = np.asarray(Image.open(png).convert("RGB")).astype(np.int16)
    white = (a[:, :, 0] > 232) & (a[:, :, 1] > 232) & (a[:, :, 2] > 232)
    closed = ndimage.binary_closing(white, iterations=2)
    filled = ndimage.binary_fill_holes(closed)
    labels, n = ndimage.label(filled)
    cuts = []
    H, W = white.shape
    for i, sl in enumerate(ndimage.find_objects(labels), 1):
        ys, xs = sl
        h, w = ys.stop - ys.start, xs.stop - xs.start
        if h < 22 or w < 36:
            continue
        comp = labels[sl] == i
        solid = (closed[sl] & comp).sum() / max(1, comp.sum())
        if comp.mean() < 0.75 or solid < 0.78:
            continue
        # Letters are strokes: shrinking them by ~1/8 of their height makes them vanish, while a
        # button's white fill survives.
        core = ndimage.binary_erosion(closed[sl] & comp, iterations=max(3, h // 8))
        if core.sum() < 0.12 * comp.sum():
            continue
        for side in (_profile_cut(comp), {"left": "top", "right": "bottom"}.get(_profile_cut(comp.T) or "")):
            if not side:
                continue
            at_edge = (side == "left" and xs.start == 0) or (side == "right" and xs.stop == W) or \
                      (side == "top" and ys.start == 0) or (side == "bottom" and ys.stop == H)
            if not at_edge and not _fades(a, sl, side):
                cuts.append((xs.start, ys.start, w, h, side))
    return cuts


def _fades(a, sl, side):
    """A shape fading out (a scrolled row's edge gradient) stays mid-grey just past its
    white edge; a shape that is sliced off drops straight to the dark background."""
    ys, xs = sl
    H, W = a.shape[:2]
    if side in ("left", "right"):
        x = xs.start - 3 if side == "left" else xs.stop + 2
        if not 0 <= x < W:
            return False
        beyond = a[ys, x]
    else:
        y = ys.start - 3 if side == "top" else ys.stop + 2
        if not 0 <= y < H:
            return False
        beyond = a[y, xs]
    return beyond.mean() > 100


def audit(size: str, n: int, out: Path, binary: Path, game: Path | None):
    w, h = size.split("x")
    disp = f":{70 + n}"
    work = Path(tempfile.mkdtemp(prefix="ps5-audit-"))
    home = work / "home"
    (home / ".config/azrak").mkdir(parents=True)
    seed_cache(home)
    install_game(home / "Games", game)
    fake = work / "kyty_emulator"                     # never start a real game
    fake.write_text("#!/bin/sh\nexit 0\n")
    fake.chmod(0o755)
    (home / ".config/azrak/config.json").write_text(json.dumps(
        {"game_dirs": ["~/Games"], "sounds": False, **NO_UPDATES, "emulator": str(fake)}))
    stubs = work / "stubs"
    stubs.mkdir()
    for tool in ("xdg-open", "mpv", "vlc", "firefox"):
        (stubs / tool).write_text("#!/bin/sh\nexit 0\n")
        (stubs / tool).chmod(0o755)
    env = {**os.environ, "DISPLAY": disp, "HOME": str(home), "AZRAK_NO_ART": "1",
           "PATH": f"{stubs}:{os.environ['PATH']}"}
    for k in ("XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME"):
        env.pop(k, None)
    procs = [subprocess.Popen(["Xvfb", disp, "-screen", "0", f"{w}x{h}x24", "+extension", "GLX", "-noreset"],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)]
    time.sleep(1.5)
    procs.append(subprocess.Popen(["xfwm4", "--compositor=off"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
    time.sleep(1.5)
    logf = open(work / "app.log", "w")
    app = subprocess.Popen([str(binary)], env=env, stdout=logf, stderr=logf)
    procs.append(app)
    d = out / size
    d.mkdir(parents=True, exist_ok=True)
    problems = []
    try:
        for _ in range(60):
            if "home screen shown" in (work / "app.log").read_text():
                break
            time.sleep(0.5)
        time.sleep(1.5)
        win = run(["xdotool", "search", "--pid", str(app.pid)], env=env, capture_output=True, text=True).stdout.split()[-1]

        def key(k):
            if k.startswith("sleep:"):
                time.sleep(float(k[6:]))
                return
            run(["xdotool", "windowfocus", win], env=env)
            run(["xdotool", "key", "--window", win, k], env=env)
            time.sleep(0.45)                                        # let animations settle

        for name, pre, steps in WALK:
            for k in pre:
                key(k)
            for s, k in enumerate(["sleep:0.3"] + steps):
                key(k)
                png = d / f"{name}-{s:02}.png"
                run(["import", "-window", "root", str(png)], env=env)
                for (x, y, cw, ch, side) in find_cuts(png):
                    crop = d / f"PROBLEM-{name}-{s:02}-{side}.png"
                    pad = 60
                    run(["convert", str(png), "-crop", f"{cw + 2 * pad}x{ch + 2 * pad}+{max(0, x - pad)}+{max(0, y - pad)}", str(crop)])
                    problems.append(f"{size} {name} step {s}: white shape at ({x},{y}) {cw}x{ch} is cut on its {side} edge  -> {crop.name}")
        print("audited", size, "-", len(problems), "problem(s)", flush=True)
    finally:
        for p in reversed(procs):
            p.terminate()
        time.sleep(0.5)
        shutil.rmtree(work, ignore_errors=True)
    return problems


def main():
    sizes = sys.argv[1:] or SIZES
    out = ROOT / "dist/ui-audit"
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True)
    game = Path(os.environ["DEMO_GAME"]) if os.environ.get("DEMO_GAME") else None
    problems = []
    for i, s in enumerate(sizes):
        problems += audit(s, i, out, ROOT / "target/release/azrak", game)
    (out / "report.txt").write_text("\n".join(problems) + "\n" if problems else "No problems found.\n")
    print("\n".join(problems) if problems else "No problems found.")
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
