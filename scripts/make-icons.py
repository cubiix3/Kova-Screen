#!/usr/bin/env python3
"""Renders the Kova Screen icon set.

The mark is a capture frame whose sides break mid-edge, leaving the four corner
brackets of a region selector, with a solid accent dot for the shutter. It is
generated here rather than committed as opaque binaries so the geometry stays
reviewable and every size comes from one source of truth.

Standard library only: PNGs are written with zlib, and the .ico is a container
of PNG-encoded entries, which Windows Vista and newer accept.
"""

import math
import struct
import zlib
from pathlib import Path

# Kova accent, shared with the rest of the product family.
ACCENT = (0x86, 0xD5, 0xF4)

# Supersampling factor. The mark is drawn at 4x and box-filtered down, which
# gives clean edges without needing a real rasteriser.
SS = 4


def rounded_rect_sdf(x, y, cx, cy, half_w, half_h, radius):
    """Signed distance to a rounded rectangle; negative means inside."""
    dx = abs(x - cx) - (half_w - radius)
    dy = abs(y - cy) - (half_h - radius)
    outside = math.hypot(max(dx, 0.0), max(dy, 0.0))
    inside = min(max(dx, dy), 0.0)
    return outside + inside - radius


def render(size):
    """Renders one RGBA icon at `size` pixels, returning raw PNG scanlines."""
    n = size * SS
    coverage = [[0] * n for _ in range(n)]

    cx = cy = n / 2.0
    half = n * 0.36
    radius = n * 0.11
    stroke = max(n * 0.075, 1.0)
    # How much of each side is cut away, leaving corner brackets.
    gap = half * 0.44
    dot_r = n * 0.105

    for py in range(n):
        y = py + 0.5
        for px in range(n):
            x = px + 0.5

            d = rounded_rect_sdf(x, y, cx, cy, half, half, radius)
            on_ring = abs(d) <= stroke / 2.0

            # Break each side at its midpoint so the frame reads as selection
            # brackets rather than a plain box.
            if on_ring:
                if abs(x - cx) < gap and abs(abs(y - cy) - half) < stroke:
                    on_ring = False
                elif abs(y - cy) < gap and abs(abs(x - cx) - half) < stroke:
                    on_ring = False

            in_dot = math.hypot(x - cx, y - cy) <= dot_r

            if in_dot or on_ring:
                coverage[py][px] = 1

    out = bytearray()
    total = SS * SS
    for y in range(size):
        out.append(0)  # PNG filter type 0 for this scanline
        for x in range(size):
            hits = 0
            for sy in range(SS):
                row = coverage[y * SS + sy]
                base = x * SS
                for sx in range(SS):
                    hits += row[base + sx]
            alpha = (hits * 255) // total
            if alpha == 0:
                out += b"\x00\x00\x00\x00"
            else:
                out += bytes((*ACCENT, alpha))
    return bytes(out)


def write_png(path, size, raw):
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)  # 8-bit RGBA
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    path.write_bytes(png)
    return png


def write_ico(path, entries):
    """Writes an .ico whose entries are PNG-encoded."""
    count = len(entries)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    directory = b""
    payload = b""
    for size, png in entries:
        # A 0 in the size byte means 256.
        directory += struct.pack(
            "<BBBBHHII",
            size if size < 256 else 0,
            size if size < 256 else 0,
            0,
            0,
            1,
            32,
            len(png),
            offset,
        )
        payload += png
        offset += len(png)
    path.write_bytes(header + directory + payload)


def main():
    icons = Path(__file__).resolve().parent.parent / "apps" / "kova-screen" / "icons"
    icons.mkdir(parents=True, exist_ok=True)

    png_sizes = [32, 128, 256, 512]
    ico_sizes = [16, 24, 32, 48, 64, 128, 256]

    rendered = {s: render(s) for s in sorted(set(png_sizes + ico_sizes))}

    for size in png_sizes:
        name = "icon.png" if size == 512 else f"{size}x{size}.png"
        write_png(icons / name, size, rendered[size])
    # Tauri also looks for this specific retina name.
    write_png(icons / "128x128@2x.png", 256, rendered[256])

    entries = [(s, write_png(icons / f"_ico_{s}.png", s, rendered[s])) for s in ico_sizes]
    write_ico(icons / "icon.ico", entries)
    for s in ico_sizes:
        (icons / f"_ico_{s}.png").unlink()

    print(f"wrote {len(png_sizes) + 1} pngs and icon.ico with {len(ico_sizes)} sizes")


if __name__ == "__main__":
    main()
