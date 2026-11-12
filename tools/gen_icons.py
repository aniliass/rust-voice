#!/usr/bin/env python3
"""Generates src/icons.rs: the Rust Voice icons, drawn from scratch for this project (MIT).

Each icon is pixel art on a grid; the script packs it into horizontal one-colour runs.
Usage: python3 tools/gen_icons.py [--preview out.png]
"""
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# palette
W = 0xFFFFFFFF  # white
L = 0xFFE4E4EC  # light (counts as "light" for the talking fill)
M = 0xFFB4B4C2  # mid grey: grille, holder
D = 0xFF8A8A9A  # dark grey: stand
K = 0xFF2A2A36  # near black: details on light shapes
R = 0xFFE5484D  # red slash
r = 0xFF9E2A2E  # dark red slash edge
P = 0xFFA855F7  # Rust Voice purple
p = 0xFF7E3BC4  # dark purple


def blank(w=16, h=16):
    return [[0] * w for _ in range(h)]


def put(g, x, y, c):
    if 0 <= y < len(g) and 0 <= x < len(g[0]):
        g[y][x] = c


def grid(rows, key):
    return [[key[ch] if ch != '.' else 0 for ch in row] for row in rows]


# ---- 16x16 ---------------------------------------------------------------
# microphone: rounded capsule with two grille lines, U-shaped holder, stand, base.
# Capsule rows 1..8 are the "capsule" filled with purple while talking (see nametag.rs).
MIC_ROWS = [
    "................",
    "......WWWW......",
    ".....WWWWWL.....",
    ".....WMMMWL.....",
    ".....WWWWWL.....",
    ".....WMMMWL.....",
    "...M.WWWWWL.M...",
    "...M.WWWWWL.M...",
    "...M..LLLL..M...",
    "....M......M....",
    ".....MMMMMM.....",
    ".......DD.......",
    ".......DD.......",
    ".....DDDDDD.....",
    "................",
    "................",
]
KEY = {'W': W, 'L': L, 'M': M, 'D': D, 'K': K, 'R': R, 'r': r, 'P': P, 'p': p}


def mic():
    return grid(MIC_ROWS, KEY)


def speaker(waves=3):
    g = blank()
    # box
    for y in range(5, 11):
        for x in range(2, 5):
            put(g, x, y, W if y < 8 else L)
    # cone, flaring to the right
    for x in range(5, 9):
        k = min(x - 4, 3)
        for y in range(5 - k, 11 + k):
            put(g, x, y, W if y < 8 else L)
    # waves
    if waves >= 1:
        for y in range(6, 10):
            put(g, 10, y, L)
    if waves >= 2:
        put(g, 11, 4, L); put(g, 11, 11, L)
        for y in range(5, 11):
            put(g, 12, y, L)
    if waves >= 3:
        put(g, 13, 2, M); put(g, 13, 13, M)
        for y in range(3, 13):
            put(g, 14, y, M)
    return g


HEADPHONES_ROWS = [
    "................",
    "................",
    ".....LWWWWL.....",
    "....W......W....",
    "...W........W...",
    "..W..........W..",
    "..W..........W..",
    "..W..........W..",
    ".MWW........WWM.",
    ".MWW........WWM.",
    ".MWW........WWM.",
    ".MLL........LLM.",
    ".MLL........LLM.",
    "..MM........MM..",
    "................",
    "................",
]


def headphones():
    """Headphones: the whole voice chat (you hear others)."""
    return grid(HEADPHONES_ROWS, KEY)


NOTE_ROWS = [
    "................",
    "........WWWWWW..",
    "........WWWWWW..",
    "........W....W..",
    "........W....W..",
    "........W....W..",
    "........W....W..",
    "........W....W..",
    "........W....W..",
    "........W..LLW..",
    ".....LLLW.LLLL..",
    "....LLLLW.LLLL..",
    "....LLLLW..LL...",
    ".....LL.........",
    "................",
    "................",
]


def note():
    """Music note: sounds of the world (discs, mobs)."""
    return grid(NOTE_ROWS, KEY)


def slash(g):
    """Red slash from the top-left to the bottom-right, 2 px thick with a dark edge."""
    for y in range(1, 15):
        x = y + 1
        put(g, x - 1, y, R)
        put(g, x, y, R)
        put(g, x + 1, y, r)
    return g


def no_voice():
    """Player without a voice mod: grey "no" sign (circle with a slash), unlike the red-slashed mic and headphones."""
    g = blank()
    cx = cy = 7.5
    for y in range(16):
        for x in range(16):
            d = ((x - cx) ** 2 + (y - cy) ** 2) ** 0.5
            if 5.0 <= d <= 6.9:
                put(g, x, y, M if y < 8 else D)
            # перекладина из правого верхнего угла в левый нижний
            if d < 5.0 and abs((x - cx) + (y - cy)) <= 1.0:
                put(g, x, y, M if y < 8 else D)
    return g


PEOPLE_ROWS = [
    "................",
    "................",
    ".........MMM....",
    "....WW..MMMMM...",
    "...WWWW.MMMMM...",
    "...WWWW.MMMMM...",
    "....WW...MMM....",
    "................",
    "..WWWWWW.MMMMM..",
    ".WWWWWWWWMMMMMM.",
    ".WWWWWWWWMMMMMM.",
    ".LLLLLLLLDDDDDD.",
    ".LLLLLLLL.......",
    "................",
    "................",
    "................",
]


def people():
    """Two people: the group."""
    return grid(PEOPLE_ROWS, KEY)


def group_speaker():
    """Group voice volume row in the menu: two people, the front one purple."""
    g = people()
    for row in g:
        for i, c in enumerate(row):
            row[i] = {W: P, L: p}.get(c, c)
    return g


def group_mic():
    """Talking to a group: the microphone in purple (the overlay puts the people icon beside it)."""
    g = mic()
    for row in g:
        for i, c in enumerate(row):
            row[i] = {W: 0xFFC79BFF, L: P}.get(c, c)
    return g


def reset():
    """Circular arrow."""
    g = blank()
    cx, cy = 7.5, 8.0
    for y in range(16):
        for x in range(16):
            d = math.hypot(x - cx, y - cy)
            a = math.degrees(math.atan2(-(y - cy), x - cx))  # 0 = right, 90 = up
            if 4.3 <= d <= 5.8 and not (5 < a < 80):
                put(g, x, y, W if y < 8 else L)
    # arrow head on the top end of the arc, pointing right (clockwise)
    for y in range(0, 6):
        put(g, 9, y, W)
    for y in range(1, 5):
        put(g, 10, y, W)
    for y in range(2, 4):
        put(g, 11, y, W)
    return g


LOGO_ROWS = [
    ".PPPPPP.",
    "PPPWWPPP",
    "PPPWWPPP",
    "PWPWWPWP",
    "pWpWWpWp",
    "pppWWppp",
    "pppWWppp",
    ".pppppp.",
]

# ---- 8x8 tab icons ---------------------------------------------------------
TAB_ROWS = {
    # small microphone
    "DEVICES": [
        "...WW...",
        "..WWWW..",
        "..WMMW..",
        "..WWWW..",
        ".M.LL.M.",
        "..MMMM..",
        "...DD...",
        "..DDDD..",
    ],
    # three bars, rising
    "VOLUME": [
        "......WW",
        "......WW",
        "...WW.WW",
        "...WW.WW",
        "WW.WW.WW",
        "WW.LL.LL",
        "LL.LL.LL",
        "LL.LL.LL",
    ],
    # two people: voice groups
    "GROUPS": [
        "........",
        ".WW..LL.",
        ".WW..LL.",
        "........",
        "WWWW.LLL",
        "WWWW.LLL",
        "WWWW.LLL",
        "........",
    ],
    # padlock: password-protected group
    "LOCK": [
        "..MMMM..",
        ".M....M.",
        ".M....M.",
        "WWWWWWWW",
        "WWWKKWWW",
        "WWWKKWWW",
        "WWWWWWWW",
        "LLLLLLLL",
    ],
    # sound wave
    "ACTIVATION": [
        "....W...",
        "..W.W...",
        "..W.W.W.",
        "W.W.W.W.",
        "L.L.L.L.",
        "..L.L.L.",
        "..L.L...",
        "....L...",
    ],
    # speech bubble with three dots
    "OVERLAY": [
        "........",
        ".WWWWW..",
        "WWWWWWW.",
        "WKWKWKW.",
        "WWWWWWW.",
        ".LLLLL..",
        ".LL.....",
        ".L......",
    ],
    # key cap with a letter
    "HOTKEYS": [
        "MMMMMMMM",
        "MWWWWWWM",
        "MWWKKWWM",
        "MWKWWKWM",
        "MWKKKKWM",
        "MWKWWKWM",
        "MLLLLLLM",
        "MMMMMMMM",
    ],
    # two sliders
    "ADVANCED": [
        ".....WW.",
        "LLLLLWWL",
        ".....WW.",
        "........",
        "..WW....",
        "LLWWLLLL",
        "..WW....",
        "........",
    ],
}

DIM = {W: 0xFFC0C0C0, L: 0xFF9C9CA4, M: 0xFF6C6C74, D: 0xFF55555C, K: 0xFF1E1E24}


def dim(g):
    return [[DIM.get(c, c) for c in row] for row in g]


def runs(g):
    out = []
    for y, row in enumerate(g):
        x = 0
        while x < len(row):
            c = row[x]
            if not c:
                x += 1
                continue
            n = 1
            while x + n < len(row) and row[x + n] == c:
                n += 1
            out.append((x, y, n, c))
            x += n
    return out


def icons():
    yield "MIC", mic(), "Microphone. Capsule rows 1..=8 fill with purple while the player talks."
    yield "MIC_MUTED", slash(mic()), "Microphone off: red slash."
    yield "NO_VOICE", no_voice(), "Above a nick: player without a voice mod (grey no-sign)."
    yield "MIC_MENU", mic(), "Microphone in the menu."
    yield "MIC_MENU_OFF", slash(mic()), "Microphone (activation) off in the menu."
    yield "HEADPHONES_OFF", slash(headphones()), "Voice chat off (you hear nobody, and your microphone is off too)."
    yield "NOTE", note(), "Music and world sounds (discs from pv-addon-discs)."
    yield "SPK", speaker(), "Speaker."
    yield "SPK_MENU", speaker(), "Player volume in the menu."
    yield "SPK_MENU_OFF", slash(speaker()), "Player muted in the menu."
    yield "SPK_GROUP", group_speaker(), "Group voice (volume row in the menu)."
    yield "MIC_GROUP", group_mic(), "You are talking to your group."
    yield "RESET", reset(), "Reset to default."
    yield "LOGO", grid(LOGO_ROWS, KEY), "Rust Voice logo: purple tile with a sound level."
    for name, rows in TAB_ROWS.items():
        g = grid(rows, KEY)
        yield f"TAB_{name}", g, None
        if name != "LOCK":  # замок не гаснет: вкладка-замок всегда активна
            yield f"TAB_{name}_OFF", dim(g), None


def rust():
    out = [
        "// Rust Voice icons, drawn for this project (MIT, see LICENSE).",
        "// Generated by tools/gen_icons.py, do not edit by hand.",
        "",
        "/// Horizontal run of one colour: x, y, length in icon pixels, ARGB colour.",
        "pub type Run = (u8, u8, u8, u32);",
        "",
        "pub struct Icon {",
        "    pub w: u8,",
        "    pub h: u8,",
        "    pub runs: &'static [Run],",
        "}",
        "",
    ]
    for name, g, doc in icons():
        if doc:
            out.append(f"/// {doc}")
        rs = ", ".join(f"({x}, {y}, {n}, 0x{c:08X})" for x, y, n, c in runs(g))
        out.append(f"pub static {name}: Icon = Icon {{ w: {len(g[0])}, h: {len(g)}, runs: &[{rs}] }};")
    return "\n".join(out) + "\n"


def preview(path):
    from PIL import Image
    items = list(icons())
    s = 12
    img = Image.new("RGBA", (len(items) * 17 * s // 1 // 1 if False else 18 * s * 6, ((len(items) + 5) // 6) * 18 * s), (60, 60, 70, 255))
    for i, (_, g, _) in enumerate(items):
        ox, oy = (i % 6) * 18 * s + s, (i // 6) * 18 * s + s
        for y, row in enumerate(g):
            for x, c in enumerate(row):
                if c:
                    col = ((c >> 16) & 255, (c >> 8) & 255, c & 255, 255)
                    for dy in range(s):
                        for dx in range(s):
                            img.putpixel((ox + x * s + dx, oy + y * s + dy), col)
    img.save(path)


if __name__ == "__main__":
    src = ROOT / "src" if (ROOT / "src").is_dir() else ROOT / "mixins" / "rust_voice" / "api" / "src"
    (src / "icons.rs").write_text(rust())
    if "--preview" in sys.argv:
        preview(sys.argv[sys.argv.index("--preview") + 1])
    print("src/icons.rs written")
