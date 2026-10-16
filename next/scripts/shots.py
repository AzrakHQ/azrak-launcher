#!/usr/bin/env python3
"""Convert target/shots/*.ppm (written by the render test) to PNG for review."""
import glob, struct, zlib, os
root = os.path.join(os.path.dirname(__file__), "..", "target", "shots")
for f in sorted(glob.glob(os.path.join(root, "*.ppm"))):
    d = open(f, "rb").read()
    _, rest = d.split(b"255\n", 1)
    w, h = 1280, 800
    raw = b"".join(b"\x00" + rest[y * w * 3:(y + 1) * w * 3] for y in range(h))
    def ch(t, b):
        c = struct.pack(">I", len(b)) + t + b
        return c + struct.pack(">I", zlib.crc32(t + b) & 0xffffffff)
    open(f[:-4] + ".png", "wb").write(b"\x89PNG\r\n\x1a\n" + ch(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + ch(b"IDAT", zlib.compress(raw, 6)) + ch(b"IEND", b""))
