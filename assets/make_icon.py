"""Render the DiskTools app icon (Lucide "gauge" on a dark rounded tile).

Usage: python assets/make_icon.py <path-to-lucide.ttf>
Writes assets/icon.png (256 px) and assets/icon.ico (16-256 px).
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

GAUGE = ""
BG = (0x19, 0x1A, 0x1D, 255)
BORDER = (0x36, 0x39, 0x3E, 255)
ACCENT = (0x34, 0xBE, 0x82, 255)


def render(size: int, font_path: str) -> Image.Image:
    # Draw at 4x and downsample for clean edges.
    s = size * 4
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    r = int(s * 0.22)
    d.rounded_rectangle([0, 0, s - 1, s - 1], radius=r, fill=BG, outline=BORDER, width=max(4, s // 64))
    font = ImageFont.truetype(font_path, int(s * 0.62))
    box = d.textbbox((0, 0), GAUGE, font=font)
    w, h = box[2] - box[0], box[3] - box[1]
    d.text(((s - w) / 2 - box[0], (s - h) / 2 - box[1]), GAUGE, font=font, fill=ACCENT)
    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    font_path = sys.argv[1]
    out = Path(__file__).parent
    big = render(256, font_path)
    big.save(out / "icon.png")
    big.save(out / "icon.ico", sizes=[(n, n) for n in (16, 20, 24, 32, 40, 48, 64, 128, 256)])
    print(f"wrote {out / 'icon.png'} and {out / 'icon.ico'}")


if __name__ == "__main__":
    main()
