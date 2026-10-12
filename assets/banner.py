#!/usr/bin/env python3
"""Draws assets/banner.png, the picture at the top of the README.

Everything is made of square blocks, like the app itself: the lettering comes
from the small bitmap glyphs below, not from a font file, so the picture looks
the same wherever it is drawn and no font licence comes with it.

    python3 assets/banner.py

Needs Pillow (`pip install pillow`).
"""

from pathlib import Path

from PIL import Image, ImageDraw

WIDTH, HEIGHT = 1600, 480
# The only thing that is not one of the title's colours: what is behind them.
BLACK = (10, 10, 10)
# The colours of the title in the app: 208, 214, 220 and 226 of the 256
# colours of a terminal. Orange, amber, gold and yellow, and no red.
RAMP = [(255, 135, 0), (255, 175, 0), (255, 215, 0), (255, 255, 0)]

# Capitals, 5 by 7.
CAPITALS = {
    "A": [".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
    "D": ["####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####."],
    "E": ["#####", "#....", "#....", "####.", "#....", "#....", "#####"],
    "F": ["#####", "#....", "#....", "####.", "#....", "#....", "#...."],
    "G": [".###.", "#...#", "#....", "#.###", "#...#", "#...#", ".###."],
    "I": ["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "#####"],
    "J": ["..###", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##.."],
    "L": ["#....", "#....", "#....", "#....", "#....", "#....", "#####"],
    "M": ["#...#", "##.##", "#.#.#", "#.#.#", "#...#", "#...#", "#...#"],
    "N": ["#...#", "##..#", "#.#.#", "#.#.#", "#..##", "#...#", "#...#"],
    "O": [".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
    "R": ["####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#"],
    "S": [".####", "#....", "#....", ".###.", "....#", "....#", "####."],
    "W": ["#...#", "#...#", "#...#", "#.#.#", "#.#.#", "##.##", "#...#"],
    " ": ["....."] * 7,
}

# Small letters, 5 by 9: two rows for what sticks out above, two for below.
SMALL = {
    "a": [".....", ".....", ".###.", "....#", ".####", "#...#", ".####", ".....", "....."],
    "c": [".....", ".....", ".###.", "#...#", "#....", "#...#", ".###.", ".....", "....."],
    "d": ["....#", "....#", ".####", "#...#", "#...#", "#...#", ".####", ".....", "....."],
    "g": [".....", ".....", ".####", "#...#", "#...#", "#...#", ".####", "....#", ".###."],
    "i": ["..#..", ".....", ".##..", "..#..", "..#..", "..#..", ".###.", ".....", "....."],
    "k": ["#....", "#....", "#..#.", "#.#..", "##...", "#.#..", "#..#.", ".....", "....."],
    "n": [".....", ".....", "####.", "#...#", "#...#", "#...#", "#...#", ".....", "....."],
    "o": [".....", ".....", ".###.", "#...#", "#...#", "#...#", ".###.", ".....", "....."],
    "p": [".....", ".....", "####.", "#...#", "#...#", "#...#", "####.", "#....", "#...."],
    "s": [".....", ".....", ".####", "#....", ".###.", "....#", "####.", ".....", "....."],
    "t": [".#...", ".#...", "####.", ".#...", ".#...", ".#..#", "..##.", ".....", "....."],
    ".": [".....", ".....", ".....", ".....", ".....", ".##..", ".##..", ".....", "....."],
}


def width_of(text, size):
    """How wide a line of glyphs is: five blocks a letter, one in between."""
    return (len(text) * 6 - 1) * size


def write(draw, glyphs, text, x, y, size, colour_of):
    """Draw text from blocks. `colour_of` gives the colour of letter `i`."""
    for i, letter in enumerate(text):
        for row, line in enumerate(glyphs[letter]):
            for column, block in enumerate(line):
                if block == "#":
                    left = x + (i * 6 + column) * size
                    top = y + row * size
                    draw.rectangle((left, top, left + size - 1, top + size - 1), fill=colour_of(i))


def mark(draw, x, y, quadrant):
    """The mark in front of the title, ▟▙: two cells of four quadrants each."""
    # As (column, row) per cell: ▟ has its top right quadrant and both bottom
    # ones, ▙ its top left and both bottom ones. Together: a little hill.
    shapes = [
        ([(1, 0), (0, 1), (1, 1)], RAMP[0]),
        ([(0, 0), (0, 1), (1, 1)], RAMP[1]),
    ]
    for cell, (quadrants, colour) in enumerate(shapes):
        for column, row in quadrants:
            left = x + (cell * 2 + column) * quadrant
            top = y + row * quadrant
            draw.rectangle((left, top, left + quadrant - 1, top + quadrant - 1), fill=colour)


def main():
    image = Image.new("RGB", (WIDTH, HEIGHT), BLACK)
    draw = ImageDraw.Draw(image)

    # The title, one colour of the ramp per three letters, behind its mark.
    title, size = "DOMAINJAEGER", 12
    quadrant = 7 * size // 2
    gap = 3 * size
    total = 4 * quadrant + gap + width_of(title, size)
    x, y = (WIDTH - total) // 2, 96
    mark(draw, x, y, quadrant)
    write(draw, CAPITALS, title, x + 4 * quadrant + gap, y, size, lambda i: RAMP[i * len(RAMP) // len(title)])

    # What it is about.
    tagline, size = "DOMAIN NAMES MADE OF REAL WORDS", 4
    spread = lambda i: RAMP[i * len(RAMP) // len(tagline)]
    write(draw, CAPITALS, tagline, (WIDTH - width_of(tagline, size)) // 2, 228, size, spread)

    # Three finds, the way the app lists them: a block for a name that is
    # available, the word in yellow and the extension in orange.
    names, size = ["cook.ing", "sn.app", "tod.ai"], 6
    block, space, between = 5 * size, 3 * size, 14 * size
    total = sum(block + space + width_of(name, size) for name in names) + between * (len(names) - 1)
    x, y = (WIDTH - total) // 2, 312
    for name in names:
        top = y + 2 * size
        draw.rectangle((x, top, x + block - 1, top + block - 1), fill=RAMP[0])
        x += block + space
        dot = name.index(".")
        write(draw, SMALL, name, x, y, size, lambda i, dot=dot: RAMP[3] if i < dot else RAMP[0])
        x += width_of(name, size) + between

    # The ramp once more, as a line along the bottom.
    step = WIDTH // len(RAMP)
    for i, colour in enumerate(RAMP):
        draw.rectangle((i * step, HEIGHT - 12, WIDTH if i == len(RAMP) - 1 else (i + 1) * step - 1, HEIGHT - 1), fill=colour)

    out = Path(__file__).with_name("banner.png")
    image.save(out, optimize=True)
    print(f"{out} · {WIDTH}x{HEIGHT} · {out.stat().st_size // 1024} KB")


if __name__ == "__main__":
    main()
