"""Draws assets/icon.png (256x256): the mod icon for the Horizon registry, the menu logo
(purple tile with a sound level) redrawn at 32x32 pixels."""
import os
from PIL import Image, ImageDraw

N, S = 32, 8
HERE = os.path.dirname(os.path.abspath(__file__))


def lerp(a, b, t):
    return tuple(int(x + (y - x) * t) for x, y in zip(a, b))


img = Image.new("RGBA", (N * S, N * S), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
CUT = {0: 4, 1: 2, 2: 1, 3: 1}  # pixel-rounded corners: how many pixels are cut on each corner row


def inside(x, y):
    for row, cut in CUT.items():
        if y in (row, N - 1 - row) and (x < cut or x >= N - cut):
            return False
    return True


TOP, MID, LOW = (0xB5, 0x76, 0xFA), (0xA8, 0x55, 0xF7), (0x7E, 0x3B, 0xC4)
for y in range(N):
    for x in range(N):
        if not inside(x, y):
            continue
        c = lerp(TOP, MID, y / 15) if y < 16 else lerp(LOW, (0x6A, 0x2E, 0xA8), (y - 16) / 15)
        if x in (0, N - 1) or y in (0, N - 1) or not inside(x - 1, y) or not inside(x + 1, y):
            c = lerp(c, (0x4A, 0x1D, 0x80), 0.5)  # dark rim
        d.rectangle([x * S, y * S, x * S + S - 1, y * S + S - 1], fill=c + (255,))

# sound level: a tall bar in the middle, short bars at the sides
BARS = ((6, 10, 13, 19), (13, 19, 4, 27), (22, 26, 13, 19))  # x0, x1 (excl), y0, y1 (excl)
for x0, x1, y0, y1 in BARS:
    for y in range(y0, y1):
        for x in range(x0, x1):
            c = (0xFF, 0xFF, 0xFF) if y < 16 else (0xEC, 0xE0, 0xFF)
            if x == x1 - 1:
                c = lerp(c, (0xC9, 0xB0, 0xF0), 0.6)  # shaded right edge
            d.rectangle([x * S, y * S, x * S + S - 1, y * S + S - 1], fill=c + (255,))

img.save(os.path.join(HERE, "..", "assets", "icon.png"), optimize=True)
print(img.size)
