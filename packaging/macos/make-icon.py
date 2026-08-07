#!/usr/bin/env python3
"""Draw the application icon and pack it into an .icns.

Run this only when the icon should change; the results are committed so that
packaging needs nothing but a shell:

    python3 packaging/macos/make-icon.py

Writes icon.png (the 1024pt master, for reference and for anyone who wants to
rebuild the iconset with Apple's own tools) and icon.icns (what the bundle
actually carries).

The .icns container is a header followed by typed entries, and every modern
entry type takes a PNG as-is, so this needs no Apple tooling and produces the
same file on any machine.
"""

import struct
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).parent
MASTER = 1024

# macOS icons are not full-bleed: the artwork sits inside a rounded square with
# a margin, so that it lines up with every other icon in the Dock.
MARGIN = 100
RADIUS = 185

# A dusk gradient, dark enough for the light Dock and saturated enough for the
# dark one.
TOP = (60, 62, 120)
BOTTOM = (22, 26, 46)

# One colour per stem, in the order the app lists them.
STEMS = [
    (255, 186, 84),   # vocals
    (86, 204, 242),   # melody
    (235, 110, 165),  # drums
]

# A bar pattern per row. Values are fractions of the row height. Chosen by eye
# to read as audio rather than as a chart: a sung line, a held melody, a beat.
PATTERNS = [
    [0.35, 0.62, 0.90, 0.70, 0.45, 0.75, 0.55, 0.30],
    [0.55, 0.40, 0.65, 0.85, 0.60, 0.35, 0.50, 0.70],
    [1.00, 0.25, 0.55, 0.25, 1.00, 0.25, 0.60, 0.30],
]


def rounded_mask(size: int, radius: int) -> Image.Image:
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, size - 1, size - 1], radius, fill=255)
    return mask


def gradient(size: int, top: tuple, bottom: tuple) -> Image.Image:
    image = Image.new("RGB", (1, size))
    for y in range(size):
        t = y / (size - 1)
        image.putpixel(
            (0, y),
            tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)),
        )
    return image.resize((size, size))


def draw_icon() -> Image.Image:
    icon = Image.new("RGBA", (MASTER, MASTER), (0, 0, 0, 0))

    side = MASTER - 2 * MARGIN
    plate = gradient(side, TOP, BOTTOM).convert("RGBA")
    plate.putalpha(rounded_mask(side, RADIUS))
    icon.paste(plate, (MARGIN, MARGIN), plate)

    draw = ImageDraw.Draw(icon)

    # Three rows of bars, one per stem, inset from the plate.
    inset = side * 0.16
    left = MARGIN + inset
    right = MARGIN + side - inset
    top = MARGIN + inset
    bottom = MARGIN + side - inset

    rows = len(PATTERNS)
    gap = (bottom - top) * 0.09
    row_height = ((bottom - top) - gap * (rows - 1)) / rows

    for row, (colour, pattern) in enumerate(zip(STEMS, PATTERNS)):
        row_top = top + row * (row_height + gap)
        centre = row_top + row_height / 2

        count = len(pattern)
        spacing = (right - left) / count
        bar_width = spacing * 0.52
        radius = bar_width / 2

        for index, fraction in enumerate(pattern):
            height = max(row_height * fraction, bar_width)
            x = left + spacing * (index + 0.5) - bar_width / 2
            draw.rounded_rectangle(
                [x, centre - height / 2, x + bar_width, centre + height / 2],
                radius=radius,
                fill=colour + (255,),
            )

    return icon


# Entry type per pixel size. The @2x types carry the same pixels as the plain
# type of twice the point size; listing both is what makes an icon look right on
# a Retina display and on an external monitor that is not one.
ENTRIES = [
    (b"ic04", 16),
    (b"ic05", 32),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic13", 256),
    (b"ic14", 512),
]


def build_icns(icon: Image.Image) -> bytes:
    import io

    chunks = []
    for kind, size in ENTRIES:
        buffer = io.BytesIO()
        icon.resize((size, size), Image.LANCZOS).save(buffer, format="PNG")
        data = buffer.getvalue()
        chunks.append(kind + struct.pack(">I", len(data) + 8) + data)

    body = b"".join(chunks)
    return b"icns" + struct.pack(">I", len(body) + 8) + body


def main() -> None:
    icon = draw_icon()
    icon.save(HERE / "icon.png")
    (HERE / "icon.icns").write_bytes(build_icns(icon))
    print(f"wrote {HERE / 'icon.png'} and {HERE / 'icon.icns'}")


if __name__ == "__main__":
    main()
