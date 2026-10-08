#!/usr/bin/env python3
"""Render docs/demo/demo.gif from docs/demo/demo-data.json (real recorded output).

demo-data.json holds what was recorded from two real, headless `claude -p --model haiku` runs
in a throwaway project (a 5-line cart.js with an off-by-one discount): the prompts, a verbatim
sentence from each final answer, the marker commands, the real output of
`capture.sh --id cart-discount --source claude-code` and of the jq command shown in the last
scene. This script replays them in a drawn terminal/chat window: the window chrome, the
"● Bash(...)" tool-call line and the typing animation are a re-enactment, not a screen recording.

Usage: python3 -m venv .venv && .venv/bin/pip install pillow && .venv/bin/python docs/demo/render.py
"""
import json
import textwrap
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).parent
DATA = json.loads((HERE / "demo-data.json").read_text())

W, H = 860, 470
BG, BAR, FG, DIM = "#0d1117", "#161b22", "#c9d1d9", "#6e7681"
GREEN, YELLOW, CYAN, BLUE, MARK_BG = "#3fb950", "#f2cc60", "#79c0ff", "#58a6ff", "#3b2f00"
FONT_PATHS = ["/System/Library/Fonts/Menlo.ttc", "/System/Library/Fonts/Monaco.ttf",
              "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"]
FONT_PATHS.append("/usr/share/fonts/dejavu/DejaVuSansMono.ttf")
FONT_FILE = next((p for p in FONT_PATHS if Path(p).exists()), None)
if FONT_FILE is None:
    raise SystemExit("render.py needs a monospace TrueType font: install DejaVu Sans Mono or Menlo")
font = ImageFont.truetype(FONT_FILE, 15)
small = ImageFont.truetype(FONT_FILE, 13)
CW = font.getlength("M")
LH = 21
COLS = int((W - 40) / CW)
MARKER = ": CAPTURE_MARKER"


def line(text, color=FG, marker=False):
    return {"text": text, "color": color, "marker": marker or MARKER in text}


class Scene:
    def __init__(self, title, caption):
        self.title, self.caption, self.lines, self.frames = title, caption, [], []

    def draw(self, partial=None):
        img = Image.new("RGB", (W, H), BG)
        d = ImageDraw.Draw(img)
        d.rectangle([0, 0, W, 34], fill=BAR)
        for i, c in enumerate(["#ff5f56", "#ffbd2e", "#27c93f"]):
            d.ellipse([14 + i * 22, 12, 24 + i * 22, 22], fill=c)
        d.text((100, 9), self.title, font=small, fill=DIM)
        y = 48
        shown = self.lines + ([partial] if partial else [])
        for ln in shown:
            x = 20
            if ln["marker"]:
                d.rectangle([x - 4, y - 2, x + CW * len(ln["text"]) + 4, y + LH - 3], fill=MARK_BG)
            d.text((x, y), ln["text"], font=font, fill=YELLOW if ln["marker"] else ln["color"])
            y += LH
        d.rectangle([0, H - 44, W, H], fill=BAR)
        d.text((20, H - 31), self.caption, font=small, fill=CYAN)
        return img

    def add(self, ms):
        self.frames.append((self.draw(), ms))

    def type(self, text, color=FG, step=4, ms=45):
        full = line(text, color)
        for n in range(step, len(text) + step, step):
            part = dict(full, text=text[:n])
            self.frames.append((self.draw(part), ms))
        self.lines.append(full)

    def show(self, text, color=FG, hold=250):
        self.lines.append(line(text, color))
        self.add(hold)


def wrap(text, prefix=""):
    out = []
    for para in text.split("\n"):
        out += textwrap.wrap(para, COLS - len(prefix) - 2) or [""]
    return out


def chat_scene(n, runtime, prompt, outcome):
    s = Scene(f"{runtime} — session {n}", f"{n}/4  Paste into the agent chat. The agent runs the marker (the highlighted line): tagged.")
    s.add(500)
    for i, text in enumerate(wrap(prompt)):
        s.type(("> " if i == 0 else "  ") + text, GREEN, step=6, ms=30)
    s.add(500)
    s.show("")
    s.show(f"● Bash(: CAPTURE_MARKER v=1 id={DATA['id']})", FG, hold=900)
    s.show("")
    for k, part in enumerate(wrap(outcome, "● ")):
        s.show(("● " if k == 0 else "  ") + part, FG, hold=500)
    s.add(1500)
    return s


def build():
    scenes = [
        chat_scene(1, "Claude Code", DATA["prompts"][0], DATA["outcomes"][0]),
        chat_scene(2, "Claude Code", DATA["prompts"][1], DATA["outcomes"][1]),
    ]
    cmd = f"capture.sh --id {DATA['id']} --source {DATA['source']}"
    s3 = Scene("terminal — your project", "3/4  Later: one command finds every session that ran the marker.")
    s3.add(500)
    s3.type("$ " + cmd, GREEN, step=3, ms=45)
    s3.add(600)
    s3.show(DATA["export_output"], FG, hold=2000)
    scenes.append(s3)
    s4 = Scene("terminal — your project", "4/4  One chronological file: both sessions, every shell command, in order (long ones shortened).")
    s4.add(300)
    jq_lines = [
        "$ jq -r 'select(.kind==\"tool_call\" and (.content|startswith(\"{\")|not))",
        "    | \"\\(.ts[11:19]) \\(.conversation_id[0:4]) \\(.content)\"' \\",
        f"    .agent-captures/{DATA['id']}.jsonl",
    ]
    for text in jq_lines:
        s4.type(text, GREEN, step=6, ms=30)
    s4.add(400)
    for text in DATA["jq_output"]:
        if len(text) > COLS:
            text = text[: COLS - 1] + "…"
        s4.show(text, FG, hold=420)
    s4.add(2600)
    scenes.append(s4)
    return scenes


def main():
    frames, durations = [], []
    for sc in build():
        for img, ms in sc.frames:
            frames.append(img)
            durations.append(ms)
        frames.append(sc.draw())  # brief clean hold between scenes
        durations.append(300)
    pal = [f.quantize(colors=48, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE) for f in frames]
    out = HERE / "demo.gif"
    pal[0].save(out, save_all=True, append_images=pal[1:], duration=durations, loop=0, optimize=True, disposal=1)
    print(f"wrote {out} ({out.stat().st_size // 1024} KB, {len(frames)} frames, {sum(durations) / 1000:.1f}s)")


if __name__ == "__main__":
    main()
