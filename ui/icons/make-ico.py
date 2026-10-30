"""Draws satchel.ico, the exe's own icon, from the same shapes as satchel.svg (a 32-unit grid):
each size is drawn on its own, eight times larger, then scaled down, so the small sizes stay sharp.
Needs Pillow; run it after changing the shapes: python3 make-ico.py (it writes beside itself).
"""

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

GREEN = (0x1F, 0x8A, 0x4C, 255)
SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
OVER = 8


def bezier(p0, p1, p2, p3, steps=24):
    """Points along a cubic Bézier curve."""
    points = []
    for i in range(steps + 1):
        t = i / steps
        u = 1 - t
        points.append(tuple(u**3 * a + 3 * u * u * t * b + 3 * u * t * t * c + t**3 * d for a, b, c, d in zip(p0, p1, p2, p3)))
    return points


def draw(size):
    big = size * OVER
    k = big / 32  # one grid unit, in pixels
    s = lambda *xy: tuple(v * k for v in xy)
    image = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    pen = ImageDraw.Draw(image)

    # The handle: two posts and a rounded top, 3 units wide with round ends.
    w = round(3 * k)
    pen.line([s(11, 10), s(11, 7.5)], fill=GREEN, width=w)
    pen.line([s(21, 10), s(21, 7.5)], fill=GREEN, width=w)
    pen.line([s(14.5, 4), s(17.5, 4)], fill=GREEN, width=w)
    pen.arc([*s(11 - 1.5, 4 - 1.5), *s(18 + 1.5, 11 + 1.5)], 180, 270, fill=GREEN, width=w)
    pen.arc([*s(14 - 1.5, 4 - 1.5), *s(21 + 1.5, 11 + 1.5)], 270, 360, fill=GREEN, width=w)
    for x, y in [(11, 10), (21, 10)]:
        pen.ellipse([*s(x - 1.5, y - 1.5), *s(x + 1.5, y + 1.5)], fill=GREEN)

    # The body.
    body = Image.new("L", (big, big), 0)
    ImageDraw.Draw(body).rounded_rectangle([*s(3, 9), *s(29, 29)], radius=5 * k, fill=255)
    image.paste(GREEN, (0, 0), body)

    # The flap: the body's top, down to a curve, a quarter darker.
    flap = Image.new("L", (big, big), 0)
    curve = bezier((29, 15.5), (21, 19.5), (11, 19.5), (3, 15.5))
    ImageDraw.Draw(flap).polygon([s(3, 0), s(29, 0)] + [s(x, y) for x, y in curve], fill=255)
    flap = ImageChops.multiply(flap, body)
    shade = Image.new("RGBA", (big, big), (0, 0, 0, 64))
    image = Image.alpha_composite(image, Image.composite(shade, Image.new("RGBA", (big, big), (0, 0, 0, 0)), flap))

    # The clasp.
    ImageDraw.Draw(image).rounded_rectangle([*s(13, 15), *s(19, 22)], radius=1.5 * k, fill=(255, 255, 255, 255))
    return image.resize((size, size), Image.Resampling.LANCZOS)


if __name__ == "__main__":
    here = Path(__file__).resolve().parent
    images = [draw(size) for size in SIZES]
    images[-1].save(here / "satchel.ico", format="ICO", sizes=[(s, s) for s in SIZES], append_images=images[:-1])
    images[-1].save(here / "satchel-256.png")
    print(here / "satchel.ico")
