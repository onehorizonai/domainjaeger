#!/usr/bin/env python3
"""Makes the screenshots in the README: assets/screenshots/*.png.

Nothing is photographed. domainjaeger runs in a terminal that is not on
screen, what it writes goes through a terminal emulator, and the result is
painted in Menlo with the colours of a plain dark terminal, on the black of
the banner. So the pictures do not depend on anyone's own terminal.

    python3 assets/screenshots.py

The hunt is real. It runs on a fresh cache in a temporary folder, never on
your own, and without a contact, so at the dictionary's slow pace: count on
five minutes. Every run finds the names of that day, so the pictures change.
After a run:

- look at the pictures, and bring the text and the alt texts around them in
  README.md in line with what they show;
- check every name that is shown as available at its registry before the
  picture goes public. "Not registered" is not always "can be registered".

Needs macOS (for Menlo), Pillow and pyte (`pip install pillow pyte`), and a
domainjaeger to run: the one on your PATH, else the one `cargo build
--release` made, or the one `--bin` points at.
"""

import argparse
import fcntl
import os
import pty
import select
import shutil
import signal
import struct
import sys
import tempfile
import termios
import time
from pathlib import Path

import pyte
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent

# The terminal the program runs in. Tall, so that two rounds fit on it.
COLUMNS, ROWS = 84, 60
# The hunt of the first picture: the first extensions of the usual mix,
# without .in. Its registry answers "not registered" for two-letter names it
# holds back (jo.in, sk.in), so the program shows them as available, and a
# picture should not.
EXTENSIONS = "ing,ly,st,sh,me,se,re,it,ai,app"
ROUNDS = 2
# The words that are tried in the third picture.
WORDS = ["clothing", "today"]

# Letters of 30 pixels: Menlo is then 18 wide. 84 columns and the padding
# make 1600, as wide as the banner.
SIZE = 30
CELL_W, CELL_H = 18, 40
PAD_X, PAD_Y = 44, 40
BLACK = (10, 10, 10)
WHITE = (222, 222, 222)
# How much of its colour a faint letter keeps.
FAINT = 0.52

MENLO = "/System/Library/Fonts/Menlo.ttc"
# Where a sign comes from that Menlo does not have.
OTHER_FONTS = [
    "/System/Library/Fonts/Apple Symbols.ttf",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
]

# The sixteen named colours, as a plain dark terminal shows them. The names
# are those of pyte, which calls yellow brown.
NAMED = {
    "black": (0, 0, 0), "red": (224, 82, 82), "green": (63, 201, 122),
    "brown": (229, 192, 62), "blue": (72, 135, 222), "magenta": (188, 108, 201),
    "cyan": (66, 184, 213), "white": (222, 222, 222),
    "brightblack": (118, 118, 118), "brightred": (241, 106, 106),
    "brightgreen": (92, 222, 148), "brightbrown": (245, 220, 96),
    "brightblue": (104, 162, 240), "brightmagenta": (214, 136, 222),
    "brightcyan": (96, 206, 232), "brightwhite": (255, 255, 255),
}

# Blocks are drawn, not typed, so that they meet exactly: which quarters of
# its cell each one fills (a b on top, c d below).
QUARTERS = {
    "▖": "c", "▗": "d", "▘": "a", "▙": "acd",
    "▚": "ad", "▛": "abc", "▜": "abd", "▝": "b",
    "▞": "bc", "▟": "bcd", "█": "abcd", "▀": "ab",
    "▄": "cd", "▌": "ac", "▐": "bd",
}


class Screen(pyte.Screen):
    """pyte does not know faint text. The program never blinks, so faint is
    kept where blinking would be."""

    def select_graphic_rendition(self, *attrs, **kwargs):
        attrs, mapped, i = list(attrs), [], 0
        while i < len(attrs):
            code = attrs[i]
            if code in (38, 48):  # a colour with numbers of its own: leave them
                count = 3 if attrs[i + 1:i + 2] == [5] else 5
                mapped += attrs[i:i + count]
                i += count
                continue
            if code == 2:
                mapped.append(5)
            elif code == 22:
                mapped += [22, 25]
            else:
                mapped.append(code)
            i += 1
        super().select_graphic_rendition(*mapped, **kwargs)


class Terminal:
    """A program in a terminal that is not on screen."""

    def __init__(self, command, env):
        self.screen = Screen(COLUMNS, ROWS)
        self.stream = pyte.ByteStream(self.screen)
        self.written = 0
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            size = struct.pack("HHHH", ROWS, COLUMNS, 0, 0)
            fcntl.ioctl(0, termios.TIOCSWINSZ, size)
            os.execvpe(command[0], command, env)
        # What the program asks the terminal (where is the cursor?) gets an
        # answer.
        self.screen.write_process_input = lambda text: os.write(self.fd, text.encode())

    def listen(self, seconds):
        """Take in what the program writes, for this long."""
        end = time.time() + seconds
        while (left := end - time.time()) > 0:
            ready, _, _ = select.select([self.fd], [], [], left)
            if not ready:
                return
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                return
            if not data:
                return
            self.written += len(data)
            self.stream.feed(data)

    def lines(self):
        return [line.rstrip() for line in self.screen.display]

    def wait_for(self, text, limit=60):
        end = time.time() + limit
        while time.time() < end:
            self.listen(0.2)
            if any(text in line for line in self.lines()):
                return
        sys.exit(f'screenshots.py: "{text}" did not come on screen:\n' + "\n".join(self.lines()))

    def until_quiet(self, pause=3.0, limit=420):
        """Wait until the program has written nothing for a while: a round
        keeps a spinner turning, a program that waits for a key is silent."""
        started, before = time.time(), -1
        while time.time() - started < limit:
            self.listen(pause)
            if self.written == before and time.time() - started > 6:
                return
            before = self.written
        sys.exit("screenshots.py: the program did not come to rest")

    def press(self, keys):
        for key in keys:
            os.write(self.fd, key.encode())
            self.listen(0.05)

    def close(self):
        self.press("q")
        self.listen(0.5)
        self.press("\r")
        self.listen(1.0)
        try:
            os.kill(self.pid, signal.SIGTERM)
            os.waitpid(self.pid, 0)
        except (ProcessLookupError, ChildProcessError):
            pass


def colour(name, default):
    if name == "default":
        return default
    if name in NAMED:
        return NAMED[name]
    return tuple(int(name[i:i + 2], 16) for i in (0, 2, 4))


def blend(one, other, share):
    """This share of one colour, and the rest of the other."""
    return tuple(round(a * share + b * (1 - share)) for a, b in zip(one, other))


class Painter:
    def __init__(self):
        # Menlo.ttc holds regular, bold, italic and bold italic, in that order.
        self.menlo = {
            (bold, italic): ImageFont.truetype(MENLO, SIZE, index=bold + 2 * italic)
            for bold in (0, 1) for italic in (0, 1)
        }
        self.others = [ImageFont.truetype(path, SIZE) for path in OTHER_FONTS if Path(path).exists()]
        self.blanks = {}

    def has(self, font, sign):
        """Whether the font has a shape of its own for this sign."""
        if id(font) not in self.blanks:
            self.blanks[id(font)] = bytes(font.getmask("\U0010fffe"))
        mask = font.getmask(sign)
        return mask.size != (0, 0) and bytes(mask) != self.blanks[id(font)]

    def block(self, draw, sign, x, y, fill):
        """The mark of the banner: quarters of a square, two cells wide."""
        side = CELL_W * 2 - 8
        top = y + (CELL_H - side) // 2
        half_w, half_h = CELL_W // 2, side // 2
        corners = {"a": (x, top), "b": (x + half_w, top),
                   "c": (x, top + half_h), "d": (x + half_w, top + half_h)}
        for quarter in QUARTERS[sign]:
            left, upper = corners[quarter]
            draw.rectangle([left, upper, left + half_w - 1, upper + half_h - 1], fill=fill)

    def paint(self, terminal, top, bottom, path):
        """Rows `top` to `bottom` of the screen, as a picture."""
        rows = [terminal.screen.buffer[y] for y in range(top, bottom + 1)]
        size = (COLUMNS * CELL_W + 2 * PAD_X, len(rows) * CELL_H + 2 * PAD_Y)
        image = Image.new("RGB", size, BLACK)
        draw = ImageDraw.Draw(image)
        ascent, descent = self.menlo[(0, 0)].getmetrics()
        lift = (CELL_H - (ascent + descent)) // 2

        for r, row in enumerate(rows):
            y = PAD_Y + r * CELL_H
            for c in range(COLUMNS):
                cell = row[c]
                x = PAD_X + c * CELL_W
                front, back = colour(cell.fg, WHITE), colour(cell.bg, BLACK)
                if cell.reverse:
                    front, back = back, front
                if cell.blink:  # faint: see Screen
                    front = blend(front, back, FAINT)
                if back != BLACK:
                    draw.rectangle([x, y, x + CELL_W - 1, y + CELL_H - 1], fill=back)
                if cell.underscore:
                    line = y + lift + ascent + 3
                    draw.rectangle([x, line, x + CELL_W - 1, line + 1], fill=front)
                sign = cell.data
                if not sign.strip():
                    continue
                if sign in QUARTERS:
                    self.block(draw, sign, x, y, front)
                    continue
                font = self.menlo[(int(cell.bold), int(cell.italics))]
                if self.has(font, sign):
                    draw.text((x, y + lift), sign, font=font, fill=front)
                    continue
                # A sign from another font goes in the middle of its cell.
                font = next((other for other in self.others if self.has(other, sign)), font)
                box = font.getbbox(sign)
                shift = (CELL_W - (box[2] - box[0])) // 2 - box[0]
                draw.text((x + shift, y + lift), sign, font=font, fill=front)

        image.save(path, optimize=True)
        print(f"  {path}  {size[0]} x {size[1]}")


def last_round(lines):
    """From the heading of the last list of names to the line of keys under it."""
    bottom = max(i for i, line in enumerate(lines) if "enter next" in line)
    top = max(i for i, line in enumerate(lines[:bottom]) if line.split()[:2] == ["#", "name"])
    return top, bottom


def home_screen(lines):
    """From the title to the line of commands."""
    top = min(i for i, line in enumerate(lines) if "DOMAINJAEGER" in line)
    bottom = max(i for i, line in enumerate(lines) if line.strip().startswith("or type:"))
    return top, bottom


def words_tried(lines):
    """From the first word that was typed to the last answer."""
    top = min(i for i, line in enumerate(lines) if "› c " in line)
    filled = [i for i, line in enumerate(lines) if line.strip()]
    # The last line is the prompt, waiting for what comes next.
    bottom = filled[-2] if lines[filled[-1]].strip().endswith("›") else filled[-1]
    return top, bottom


def program(asked):
    built = ROOT / "target" / "release" / "domainjaeger"
    found = asked or shutil.which("domainjaeger") or (str(built) if built.exists() else None)
    if not found:
        sys.exit("screenshots.py: no domainjaeger found. Build one (cargo build --release) or pass --bin.")
    return found


def main():
    parser = argparse.ArgumentParser(description="Makes the screenshots in the README.")
    parser.add_argument("--bin", help="the domainjaeger to run")
    parser.add_argument("--out", default=str(ROOT / "assets" / "screenshots"), help="where the pictures go")
    asked = parser.parse_args()
    binary = program(asked.bin)
    out = Path(asked.out)
    out.mkdir(parents=True, exist_ok=True)
    painter = Painter()

    with tempfile.TemporaryDirectory(prefix="domainjaeger-screenshots-") as folder:
        # A cache and a home of its own, and nothing that says who is hunting.
        cache = str(Path(folder) / "cache.sqlite")
        keep = ("PATH", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR")
        env = {name: value for name, value in os.environ.items() if name in keep}
        env.update(TERM="xterm-256color", HOME=folder)
        env.setdefault("LANG", "en_US.UTF-8")
        print(f"{binary}, on a fresh cache in {folder}")

        hunt = Terminal([binary, "--db", cache, "--tlds", EXTENSIONS], env)
        hunt.wait_for("enter start")
        for number in range(1, ROUNDS + 1):
            print(f"round {number} of {ROUNDS}, a minute or two")
            hunt.press("\r")
            hunt.until_quiet()
        painter.paint(hunt, *last_round(hunt.lines()), out / "hunt.png")
        hunt.close()

        shell = Terminal([binary, "--db", cache], env)
        shell.wait_for("enter start")
        shell.listen(1.0)
        painter.paint(shell, *home_screen(shell.lines()), out / "home.png")
        for word in WORDS:
            print(f"trying {word}")
            shell.press(f"c {word}")
            shell.listen(0.5)
            shell.press("\r")
            shell.until_quiet()
        painter.paint(shell, *words_tried(shell.lines()), out / "check.png")
        shell.close()

    print("Done. Look at the pictures, and check the names they show as available.")


if __name__ == "__main__":
    main()
