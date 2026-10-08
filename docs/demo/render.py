#!/usr/bin/env python3
"""Render docs/demo/demo.gif from docs/demo/demo-data.json (real recorded output).

demo-data.json holds what was recorded from two real, headless `claude -p --model haiku` runs
in a throwaway project (a 5-line cart.js with an off-by-one discount): the prompts, a verbatim
sentence from each final answer, the marker commands, the real output of
`capture.sh --id cart-discount --source claude-code` and of the jq command shown in the last
scene. This script replays them in a drawn terminal window: the window chrome, the shell prompt,
the syntax colours, the "● Bash(...)" tool-call line and the typing animation are a
re-enactment, not a screen recording.

Usage: python3 -m venv .venv && .venv/bin/pip install pillow && .venv/bin/python docs/demo/render.py
"""
import json
import textwrap
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).parent
DATA = json.loads((HERE / "demo-data.json").read_text())

W, H = 860, 480
# Terminal palette (dark, ANSI-like)
BG, TITLEBAR, FG = "#1b1d23", "#2b2e36", "#d4d7dd"
DIM, GREEN, BLUE, YELLOW = "#7d8590", "#7ee787", "#79b8ff", "#e3b341"
ORANGE, CYAN, MAGENTA, ORANGE_CC = "#ff9e64", "#56d4dd", "#d2a8ff", "#d97757"
PANEL = "#262a33"
FONT_PATHS = [
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/Monaco.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
]
FONT_FILE = next((p for p in FONT_PATHS if Path(p).exists()), None)
if FONT_FILE is None:
    raise SystemExit("render.py needs a monospace TrueType font: install DejaVu Sans Mono or Menlo")
font = ImageFont.truetype(FONT_FILE, 15)
small = ImageFont.truetype(FONT_FILE, 13)
CW = font.getlength("M")
LH = 21
LEFT, TOP = 22, 52
COLS = int((W - 2 * LEFT) / CW)
MARKER = ": CAPTURE_MARKER"
ID = DATA["id"]


# --- tiny shell highlighter -------------------------------------------------------------
def highlight(text, in_quote=None, first=True):
    """Split one shell line into (text, colour) segments. Returns (segments, open_quote)."""
    segs, cur = [], ""
    first = first and in_quote is None
    quote = in_quote

    def flush(color):
        nonlocal cur
        if cur:
            segs.append((cur, color))
        cur = ""

    i = 0
    while i < len(text):
        ch = text[i]
        if quote:
            cur += ch
            if ch == quote:
                flush(ORANGE)
                quote = None
            i += 1
            continue
        if ch in "'\"":
            flush(FG)
            quote, cur = ch, ch
            i += 1
            continue
        if ch == " ":
            word = cur
            color = FG
            if word.startswith("-"):
                color = BLUE
            elif first and word:
                color, first = GREEN, False
            elif word in ("|", "&&", "\\"):
                color = MAGENTA
            flush(color)
            segs.append((" ", FG))
            i += 1
            continue
        cur += ch
        i += 1
    if quote:
        flush(ORANGE)
    else:
        color = FG
        if cur.startswith("-"):
            color = BLUE
        elif first and cur:
            color = GREEN
        elif cur in ("|", "&&", "\\"):
            color = MAGENTA
        flush(color)
    return segs, quote


def plain(text, color=FG):
    return {"segs": [(text, color)]}


class Scene:
    def __init__(self, title, step, caption):
        self.title, self.step, self.caption = title, step, caption
        self.lines, self.frames, self.quote = [], [], None

    # a "line" is {"segs": [(text, color)], "bg": optional panel colour, "note": optional}
    def draw(self, partial=None, cursor=False):
        img = Image.new("RGB", (W, H), BG)
        d = ImageDraw.Draw(img)
        d.rectangle([0, 0, W, 36], fill=TITLEBAR)
        for i, c in enumerate(["#ff5f56", "#ffbd2e", "#27c93f"]):
            d.ellipse([16 + i * 22, 13, 27 + i * 22, 24], fill=c)
        tw = small.getlength(self.title)
        d.text(((W - tw) / 2, 11), self.title, font=small, fill=DIM)
        y = TOP
        shown = self.lines + ([partial] if partial else [])
        for k, ln in enumerate(shown):
            if ln.get("bg"):
                d.rectangle([LEFT - 8, y - 2, W - LEFT + 8, y + LH - 3], fill=ln["bg"])
            x = LEFT
            for text, color in ln["segs"]:
                d.text((x, y), text, font=font, fill=color)
                x += CW * len(text)
            if ln.get("note"):
                d.text((x + 14, y + 1), ln["note"], font=small, fill=CYAN)
            if cursor and k == len(shown) - 1:
                d.rectangle([x + 1, y + 1, x + CW - 1, y + LH - 4], fill=FG)
            y += LH
        d.rectangle([0, H - 42, W, H], fill=TITLEBAR)
        badge = f" {self.step} "
        bw = small.getlength(badge)
        d.rounded_rectangle([18, H - 32, 18 + bw + 4, H - 10], radius=5, fill=ORANGE_CC)
        d.text((20, H - 29), badge, font=small, fill="#1b1d23")
        d.text((18 + bw + 16, H - 29), self.caption, font=small, fill=FG)
        return img

    def add(self, ms, cursor=False):
        self.frames.append((self.draw(cursor=cursor), ms))

    def push(self, ln, ms=250):
        self.lines.append(ln)
        self.add(ms)

    def type(self, make, text, step=4, ms=45):
        """Type `text`; `make(prefix_text)` builds the line for a prefix of it."""
        for n in range(step, len(text) + step, step):
            self.frames.append((self.draw(make(text[:n]), cursor=True), ms))
        self.lines.append(make(text))

    def type_shell(self, text, prompt=True, step=3, ms=45):
        quote = self.quote

        def make(prefix):
            segs, _ = highlight(prefix, quote, first=prompt)
            return {"segs": (shell_prompt() if prompt else []) + segs}

        self.type(make, text, step, ms)
        _, self.quote = highlight(text, quote, first=prompt)


def shell_prompt():
    return [("~/shop", BLUE), (" ", FG), ("(main)", MAGENTA), (" $ ", GREEN)]


def wrap(text, width):
    out = []
    for para in text.split("\n"):
        out += textwrap.wrap(para, width) or [""]
    return out


def is_marker(text):
    return MARKER in text


def chat_scene(n, prompt, outcome):
    s = Scene("shop — claude", f"{n}/4", "Paste into the agent chat. The agent runs the marker line: this session is tagged.")
    s.add(500)
    first = True
    for part in wrap(prompt, COLS - 6):
        prefix = "> " if first else "  "
        first = False
        color = ORANGE if is_marker(part) else FG
        note = "◀ the tag" if is_marker(part) else None

        def make(prefix_text, prefix=prefix, color=color, note=note, full=part):
            return {"segs": [(prefix, ORANGE_CC), (prefix_text, color)], "bg": PANEL,
                    "note": note if prefix_text == full else None}

        s.type(make, part, step=6, ms=28)
    s.add(500)
    s.push(plain(""))
    s.push({"segs": [("● ", GREEN), ("Bash", FG), (f"(: CAPTURE_MARKER v=1 id={ID})", DIM)]}, 900)
    s.push(plain(""))
    for k, part in enumerate(wrap(outcome, COLS - 4)):
        s.push({"segs": [("● " if k == 0 else "  ", FG), (part, FG)]}, 500)
    s.add(1500)
    return s


def build():
    scenes = [
        chat_scene(1, DATA["prompts"][0], DATA["outcomes"][0]),
        chat_scene(2, DATA["prompts"][1], DATA["outcomes"][1]),
    ]

    s3 = Scene("shop — -zsh", "3/4", "Later: one command finds every session that ran the marker.")
    s3.add(500, cursor=True)
    s3.type_shell(f"capture.sh --id {ID} --source {DATA['source']}", step=3, ms=45)
    s3.add(600)
    s3.push(plain(DATA["export_output"]), 2000)
    s3.push({"segs": shell_prompt()}, 1)
    s3.add(1200, cursor=True)

    s4 = Scene("shop — -zsh", "4/4", "One chronological file: both sessions, every shell command, in order (long ones shortened).")
    s4.add(300, cursor=True)
    jq_lines = [
        "jq -r 'select(.kind==\"tool_call\" and (.content|startswith(\"{\")|not))",
        "    | \"\\(.ts[11:19]) \\(.conversation_id[0:4]) \\(.content)\"' \\",
        f"    .agent-captures/{ID}.jsonl",
    ]
    for k, text in enumerate(jq_lines):
        s4.type_shell(text, prompt=(k == 0), step=6, ms=30)
    s4.add(400)
    for text in DATA["jq_output"]:
        text = text if len(text) <= COLS else text[: COLS - 1] + "…"
        time, sid, rest = text.split(" ", 2)
        segs = [(time, DIM), (" ", FG), (sid, CYAN), (" ", FG)]
        segs += [(rest, ORANGE if is_marker(rest) else FG)]
        s4.push({"segs": segs, "note": "◀ the tag" if is_marker(rest) else None}, 420)
    s4.push({"segs": shell_prompt()}, 1)
    s4.add(2600, cursor=True)
    scenes += [s3, s4]
    return scenes


def main():
    frames, durations = [], []
    for sc in build():
        for img, ms in sc.frames:
            frames.append(img)
            durations.append(ms)
    pal = [f.quantize(colors=64, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE) for f in frames]
    out = HERE / "demo.gif"
    pal[0].save(out, save_all=True, append_images=pal[1:], duration=durations, loop=0, optimize=True, disposal=1)
    print(f"wrote {out} ({out.stat().st_size // 1024} KB, {len(frames)} frames, {sum(durations) / 1000:.1f}s)")


if __name__ == "__main__":
    main()
