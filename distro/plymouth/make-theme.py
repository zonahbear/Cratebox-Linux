#!/usr/bin/env python3
"""Draws the Cratebox OS boot splash (a Plymouth two-step theme): the app icon's jog wheel with its
marker going round as the throbber, and the CRATEBOX wordmark at the bottom. Needs Pillow.
    python3 distro/plymouth/make-theme.py distro/overlay/usr/share/plymouth/themes/cratebox
The PNGs are committed, so the ISO build doesn't need Pillow; rerun this after changing the look."""
import math, os, sys
from PIL import Image, ImageDraw, ImageFont

BG = (11, 13, 16)          # the icon's background
DISC = (20, 23, 28)
BLUE = (57, 167, 255)
ORANGE = (255, 138, 61)
FG = (230, 236, 243)
SIZE, SS, FRAMES = 200, 4, 60
here = os.path.dirname(os.path.abspath(__file__))
fonts = os.path.join(here, "..", "..", "assets", "fonts")

def wheel(angle):
    s = SIZE * SS
    im = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    c, r, w = s / 2, s * 0.40, s * 0.035
    d.ellipse([c - r, c - r, c + r, c + r], fill=DISC + (255,), outline=BLUE, width=round(w))
    # the marker: a short bar from the ring towards the middle, like the icon's 12 o'clock tick
    a = math.radians(angle - 90)
    for t in range(0, 101):
        k = r - w / 2 - (r * 0.18) * t / 100
        x, y = c + k * math.cos(a), c + k * math.sin(a)
        d.ellipse([x - w / 2, y - w / 2, x + w / 2, y + w / 2], fill=BLUE)
    rr = s * 0.12
    d.ellipse([c - rr, c - rr, c + rr, c + rr], fill=ORANGE)
    return im.resize((SIZE, SIZE), Image.LANCZOS)

def wordmark():
    f = ImageFont.truetype(os.path.join(fonts, "Inter-Bold.otf"), 26 * SS)
    a, b = "CRATE", "BOX"
    wa, wb = f.getlength(a), f.getlength(b)
    h = 34 * SS
    im = Image.new("RGBA", (int(wa + wb) + 4 * SS, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    d.text((2 * SS, h / 2), a, font=f, fill=FG, anchor="lm")
    d.text((2 * SS + wa, h / 2), b, font=f, fill=BLUE, anchor="lm")
    return im.resize((im.width // SS, im.height // SS), Image.LANCZOS)

def main(out):
    os.makedirs(out, exist_ok=True)
    for i in range(FRAMES):
        wheel(360 * i / FRAMES).save(os.path.join(out, f"throbber-{i + 1:04d}.png"), optimize=True)
    wheel(0).save(os.path.join(out, "animation-0001.png"), optimize=True)
    wordmark().save(os.path.join(out, "watermark.png"), optimize=True)

if __name__ == "__main__":
    main(sys.argv[1])
