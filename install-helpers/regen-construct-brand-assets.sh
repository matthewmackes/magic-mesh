#!/usr/bin/env bash
# Regenerate the Construct raster brand masters from one source image.
#
# Canonical source:
#   assets/brand/construct/source.png
#
# The source artwork is treated as visual inspiration and mark material only.
# Product text is rendered here so generated text artifacts never ship.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BRAND="$ROOT/assets/brand"
CONSTRUCT="$BRAND/construct"
SRC="${1:-$CONSTRUCT/source.png}"

if [ ! -f "$SRC" ]; then
  echo "missing Construct source image: $SRC" >&2
  exit 2
fi

python3 - "$ROOT" "$SRC" <<'PY'
import os
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

root = Path(sys.argv[1])
src_path = Path(sys.argv[2])
brand = root / "assets" / "brand"
construct = brand / "construct"
construct.mkdir(parents=True, exist_ok=True)

PRODUCT = "Construct"
STUDIO = "Software Studio: MDE"
RELEASE = "Release 1.0 BETA"


def font_path(query: str) -> str:
    return subprocess.check_output(
        ["fc-match", "-f", "%{file}\n", query],
        text=True,
    ).strip()


REGULAR = font_path("DejaVu Sans:style=Book")
BOLD = font_path("DejaVu Sans:style=Bold")


def font(size: int, bold: bool = False) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(BOLD if bold else REGULAR, size=size)


def cover(img: Image.Image, size: tuple[int, int], center: tuple[float, float] = (0.5, 0.5)) -> Image.Image:
    w, h = img.size
    tw, th = size
    scale = max(tw / w, th / h)
    nw, nh = int(round(w * scale)), int(round(h * scale))
    resized = img.resize((nw, nh), Image.Resampling.LANCZOS)
    x = int(round((nw - tw) * center[0]))
    y = int(round((nh - th) * center[1]))
    return resized.crop((x, y, x + tw, y + th))


def fit_text(draw: ImageDraw.ImageDraw, text: str, max_width: int, start_size: int, min_size: int, bold: bool) -> ImageFont.FreeTypeFont:
    for size in range(start_size, min_size - 1, -2):
        candidate = font(size, bold)
        left, top, right, bottom = draw.textbbox((0, 0), text, font=candidate)
        if right - left <= max_width:
            return candidate
    return font(min_size, bold)


def centered(draw: ImageDraw.ImageDraw, xy: tuple[int, int], text: str, fnt: ImageFont.FreeTypeFont, fill: tuple[int, int, int, int]) -> None:
    draw.text(xy, text, font=fnt, fill=fill, anchor="mm")


def left_text(draw: ImageDraw.ImageDraw, xy: tuple[int, int], text: str, fnt: ImageFont.FreeTypeFont, fill: tuple[int, int, int, int]) -> None:
    draw.text(xy, text, font=fnt, fill=fill, anchor="lm")


source = Image.open(src_path).convert("RGB")
w, h = source.size
mark_box = (
    int(w * 0.308),
    0,
    int(w * 0.692),
    int(h * 0.703),
)
mark_source = source.crop(mark_box).convert("RGBA")


def brand_field(size: tuple[int, int], darken: float = 0.76, blur: float = 18.0) -> Image.Image:
    base = cover(source, size).filter(ImageFilter.GaussianBlur(blur)).convert("RGBA")
    dark = Image.new("RGBA", size, (9, 12, 15, 255))
    img = Image.blend(base, dark, darken)
    overlay = Image.new("RGBA", size, (0, 0, 0, 0))
    draw = ImageDraw.Draw(overlay, "RGBA")
    tw, th = size
    draw.rectangle((0, 0, tw, int(th * 0.22)), fill=(255, 255, 255, 10))
    draw.rectangle((0, int(th * 0.62), tw, th), fill=(0, 0, 0, 90))
    draw.ellipse((-tw * 0.18, -th * 0.32, tw * 0.72, th * 0.88), fill=(66, 101, 235, 30))
    draw.ellipse((tw * 0.35, -th * 0.20, tw * 1.12, th * 0.76), fill=(154, 84, 238, 26))
    draw.line((0, int(th * 0.72), tw, int(th * 0.62)), fill=(102, 124, 156, 32), width=max(1, th // 140))
    return Image.alpha_composite(img, overlay)


def mark_badge(size: int, ring: bool = True) -> Image.Image:
    mark = cover(mark_source, (size, size), center=(0.5, 0.44)).convert("RGBA")
    mask = Image.new("L", (size, size), 0)
    md = ImageDraw.Draw(mask)
    pad = max(2, size // 90)
    md.ellipse((pad, pad, size - pad, size - pad), fill=255)
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(mark, (0, 0), mask)
    if ring:
        draw = ImageDraw.Draw(out, "RGBA")
        draw.ellipse((pad, pad, size - pad, size - pad), outline=(236, 239, 244, 185), width=max(2, size // 80))
        draw.ellipse((pad * 5, pad * 5, size - pad * 5, size - pad * 5), outline=(123, 99, 242, 90), width=max(1, size // 150))
    return out


def paste_center(dst: Image.Image, src: Image.Image, center_xy: tuple[int, int]) -> None:
    x = int(center_xy[0] - src.width / 2)
    y = int(center_xy[1] - src.height / 2)
    dst.alpha_composite(src, (x, y))


def main_lockup() -> Image.Image:
    img = brand_field((1408, 768), darken=0.80)
    draw = ImageDraw.Draw(img, "RGBA")
    paste_center(img, mark_badge(372), (704, 238))
    draw.rounded_rectangle((368, 502, 1040, 710), radius=28, fill=(7, 10, 13, 164), outline=(255, 255, 255, 24), width=2)
    title = fit_text(draw, PRODUCT, 620, 104, 72, True)
    centered(draw, (704, 558), PRODUCT, title, (247, 249, 252, 255))
    centered(draw, (704, 630), STUDIO, font(34, True), (198, 207, 219, 245))
    centered(draw, (704, 676), RELEASE, font(24), (135, 146, 160, 230))
    return img


def wallpaper_left() -> Image.Image:
    img = brand_field((1408, 768), darken=0.74)
    draw = ImageDraw.Draw(img, "RGBA")
    img.alpha_composite(mark_badge(260), (112, 210))
    left_text(draw, (430, 300), PRODUCT, font(92, True), (247, 249, 252, 255))
    left_text(draw, (435, 386), STUDIO, font(34, True), (201, 210, 222, 245))
    left_text(draw, (436, 435), RELEASE, font(24), (143, 154, 166, 235))
    draw.line((438, 466, 935, 466), fill=(126, 151, 250, 92), width=4)
    return img


def wallpaper_mark_only() -> Image.Image:
    img = brand_field((1408, 768), darken=0.70)
    mark = mark_badge(620)
    mark.putalpha(mark.getchannel("A").point(lambda a: int(a * 0.62)))
    paste_center(img, mark, (704, 372))
    return img


def wallpaper_quiet() -> Image.Image:
    img = brand_field((1408, 768), darken=0.86, blur=28)
    mark = mark_badge(300)
    mark.putalpha(mark.getchannel("A").point(lambda a: int(a * 0.30)))
    img.alpha_composite(mark, (1030, 418))
    draw = ImageDraw.Draw(img, "RGBA")
    draw.line((0, 628, 1408, 584), fill=(255, 255, 255, 22), width=2)
    return img


def wallpaper_highlight() -> Image.Image:
    img = brand_field((1408, 768), darken=0.64, blur=14)
    draw = ImageDraw.Draw(img, "RGBA")
    draw.rounded_rectangle((94, 94, 1314, 674), radius=38, fill=(5, 8, 11, 104), outline=(247, 249, 252, 38), width=2)
    paste_center(img, mark_badge(330), (704, 306))
    centered(draw, (704, 550), PRODUCT, font(86, True), (247, 249, 252, 255))
    centered(draw, (704, 622), STUDIO, font(31, True), (206, 214, 226, 245))
    return img


def square_lockup() -> Image.Image:
    img = brand_field((1270, 1270), darken=0.82)
    draw = ImageDraw.Draw(img, "RGBA")
    paste_center(img, mark_badge(520), (635, 424))
    centered(draw, (635, 820), PRODUCT, font(124, True), (247, 249, 252, 255))
    centered(draw, (635, 934), STUDIO, font(44, True), (198, 207, 219, 245))
    centered(draw, (635, 1000), RELEASE, font(32), (137, 148, 162, 235))
    return img


def wordmark_banner() -> Image.Image:
    img = brand_field((2508, 627), darken=0.82, blur=22)
    draw = ImageDraw.Draw(img, "RGBA")
    img.alpha_composite(mark_badge(410), (162, 108))
    left_text(draw, (665, 274), PRODUCT, font(146, True), (247, 249, 252, 255))
    left_text(draw, (675, 395), STUDIO, font(54, True), (202, 211, 223, 246))
    left_text(draw, (676, 464), RELEASE, font(36), (139, 150, 164, 235))
    draw.line((676, 512, 1700, 512), fill=(126, 151, 250, 86), width=5)
    return img


def greeter_hero() -> Image.Image:
    img = brand_field((1672, 941), darken=0.77, blur=22)
    mark = mark_badge(520)
    mark.putalpha(mark.getchannel("A").point(lambda a: int(a * 0.44)))
    paste_center(img, mark, (836, 420))
    return img


def watermark() -> Image.Image:
    img = Image.new("RGBA", (420, 140), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img, "RGBA")
    img.alpha_composite(mark_badge(96), (20, 22))
    left_text(draw, (132, 64), PRODUCT, font(42, True), (245, 247, 250, 220))
    left_text(draw, (134, 104), STUDIO, font(18, True), (190, 199, 211, 195))
    return img


outputs = {
    brand / "CONSTRUCT-MAIN.png": main_lockup(),
    brand / "CONSTRUCT-WALLPAPER1.png": main_lockup(),
    brand / "CONSTRUCT-WALLPAPER2.png": wallpaper_left(),
    brand / "CONSTRUCT-WALLPAPER3.png": wallpaper_mark_only(),
    brand / "CONSTRUCT-WALLPAPER4.png": wallpaper_quiet(),
    brand / "CONSTRUCT-WALLPAPER5.png": wallpaper_highlight(),
    brand / "logo-lockup.png": square_lockup(),
    brand / "wordmark.png": wordmark_banner(),
    brand / "wordmark-hero.png": wordmark_banner(),
    brand / "greeter-wordmark.png": wordmark_banner(),
    brand / "greeter-hero.png": greeter_hero(),
    brand / "watermark.png": watermark(),
}

for path, img in outputs.items():
    img.save(path, "PNG", optimize=True)
    print(f"wrote {path} ({img.width}x{img.height})")

mark_path = construct / "mark-source.png"
mark_badge(768).save(mark_path, "PNG", optimize=True)
print(f"wrote {mark_path} (768x768)")
PY

"$ROOT/install-helpers/regen-app-icon.sh" "$CONSTRUCT/mark-source.png"
