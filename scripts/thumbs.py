# /// script
# requires-python = ">=3.10"
# dependencies = ["playwright>=1.40", "pillow>=10"]
# ///
"""Thumbnail tools: research a channel, render HTML designs, and check them at feed size.

    uv run thumbs.py channel <channel or search URL> [--n 40] [--out research/channel]
    uv run thumbs.py render NAME=page.html [...] [--out .]
    uv run thumbs.py feed a.png b.png [--refs past/*.jpg] [--out feed-check.jpg]

channel: id, views, date and title of the last --n videos (videos.tsv, by views), their
         thumbnails, and sheet.jpg labelled with views. A search works too: "ytsearch30:<topic>".
         Needs yt-dlp on PATH.
render:  each page at 1280x720 CSS px, screenshotted at 2x: NAME@2x.png (2560x1440, to upload)
         and NAME.png (1280x720). Writes NAME.jpg too when the PNG is over YouTube's 2 MB limit.
feed:    feed-check.jpg: each image at 320x180 (phone feed) and 168x94 (desktop up next), with
         rounded corners and a duration badge, beside --refs (the channel's past thumbnails).
First run: `uv run --with playwright playwright install chromium`.
"""

import argparse
import subprocess
import sys
import urllib.request
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

BG = (15, 15, 15)  # YouTube's dark theme
FG = (241, 241, 241)


def font(size: int) -> ImageFont.ImageFont:
    for name in ("Helvetica.ttc", "Arial.ttf", "DejaVuSans.ttf"):
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            pass
    return ImageFont.load_default(size)


def channel(url: str, n: int, out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    # --flat-playlist is faster but gives no view counts for a channel's videos tab.
    lines = subprocess.run(
        ["yt-dlp", "--no-update", "--skip-download", "--ignore-errors", "--playlist-end", str(n),
         "--print", "%(id)s\t%(view_count)s\t%(upload_date)s\t%(title)s", url],
        capture_output=True, text=True).stdout.splitlines()
    rows = [line.split("\t", 3) for line in lines if line.count("\t") == 3]
    rows.sort(key=lambda r: -int(r[1]) if r[1].isdigit() else 0)
    (out / "videos.tsv").write_text("".join("\t".join(r) + "\n" for r in rows))
    tiles = []
    for vid, views, date, title in rows:
        path = out / f"{vid}.jpg"
        if not path.exists():
            for size in ("maxresdefault", "hqdefault"):
                try:
                    urllib.request.urlretrieve(f"https://i.ytimg.com/vi/{vid}/{size}.jpg", path)
                    break
                except OSError:
                    continue
        if path.exists():
            label = f"{int(views):,} · {date}" if views.isdigit() else date
            tiles.append((Image.open(path).convert("RGB").resize((320, 180)), label))
    if tiles:
        grid(tiles, 6, out / "sheet.jpg")
    print(f"{len(rows)} videos → {out}/videos.tsv, sheet.jpg")


def render(pages: dict[str, Path], out: Path) -> None:
    from playwright.sync_api import sync_playwright

    out.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as p:
        browser = p.chromium.launch()
        page = browser.new_page(viewport={"width": 1280, "height": 720}, device_scale_factor=2)
        for name, html in pages.items():
            page.goto(html.resolve().as_uri())
            page.evaluate("document.fonts.ready")
            page.wait_for_timeout(300)
            big = out / f"{name}@2x.png"
            page.screenshot(path=str(big), clip={"x": 0, "y": 0, "width": 1280, "height": 720})
            small = out / f"{name}.png"
            Image.open(big).convert("RGB").resize((1280, 720), Image.LANCZOS).save(small, optimize=True)
            note = f"{small.stat().st_size / 1e6:.2f} MB"
            if small.stat().st_size > 2_000_000:
                jpg = small.with_suffix(".jpg")
                Image.open(small).save(jpg, quality=90)
                note += f", over 2 MB: {jpg.name} is {jpg.stat().st_size / 1e6:.2f} MB"
            print(f"{big}\n{small} ({note})")
        browser.close()


def tile(path: Path, w: int, h: int) -> Image.Image:
    img = Image.open(path).convert("RGB").resize((w, h), Image.LANCZOS)
    mask = Image.new("L", (w, h), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, w - 1, h - 1), radius=max(4, w // 32), fill=255)
    card = Image.new("RGB", (w, h), BG)
    card.paste(img, mask=mask)
    d = ImageDraw.Draw(card)
    s = w / 320  # the duration badge, scaled with the tile
    d.rounded_rectangle((w - 44 * s, h - 22 * s, w - 6 * s, h - 6 * s), radius=3, fill=(0, 0, 0))
    d.text((w - 40 * s, h - 20 * s), "12:34", fill="white", font=font(max(8, int(11 * s))))
    return card


def feed(images: list[Path], refs: list[Path], out: Path) -> None:
    tiles = [(tile(p, 320, 180), p.stem) for p in images]
    tiles += [(tile(p, 320, 180), "past: " + p.stem) for p in refs]
    small = [(tile(p, 168, 94), p.stem) for p in images]
    grid(tiles, 3, out, extra=small)
    print(out)


def grid(tiles, cols: int, out: Path, extra=()) -> None:
    """Tiles with a caption under each, `cols` to a row; `extra` (smaller tiles) as a last row."""
    gap, cap = 16, 26
    w, h = tiles[0][0].size
    rows = (len(tiles) + cols - 1) // cols
    ew, eh = extra[0][0].size if extra else (0, 0)
    sheet = Image.new("RGB", (gap + cols * (w + gap), gap + rows * (h + cap + gap) + (eh + cap + gap if extra else 0)), BG)
    d = ImageDraw.Draw(sheet)
    for i, (img, label) in enumerate(tiles):
        x, y = gap + (i % cols) * (w + gap), gap + (i // cols) * (h + cap + gap)
        sheet.paste(img, (x, y))
        d.text((x + 2, y + h + 5), label[:44], fill=FG, font=font(15))
    y = gap + rows * (h + cap + gap)
    for i, (img, label) in enumerate(extra):
        x = gap + i * (ew + gap)
        if x + ew > sheet.width:
            break
        sheet.paste(img, (x, y))
        d.text((x + 2, y + eh + 5), label[:22], fill=FG, font=font(12))
    sheet.save(out, quality=90)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("channel")
    c.add_argument("url")
    c.add_argument("--n", type=int, default=40)
    c.add_argument("--out", type=Path, default=Path("research/channel"))
    r = sub.add_parser("render")
    r.add_argument("pages", nargs="+", help="NAME=page.html")
    r.add_argument("--out", type=Path, default=Path("."))
    f = sub.add_parser("feed")
    f.add_argument("images", nargs="+", type=Path)
    f.add_argument("--refs", nargs="*", type=Path, default=[])
    f.add_argument("--out", type=Path, default=Path("feed-check.jpg"))
    a = ap.parse_args()
    if a.cmd == "channel":
        channel(a.url, a.n, a.out)
    elif a.cmd == "render":
        bad = [s for s in a.pages if "=" not in s]
        if bad:
            sys.exit(f"expected NAME=page.html, got {bad}")
        render({k: Path(v) for k, v in (s.split("=", 1) for s in a.pages)}, a.out)
    else:
        feed(a.images, a.refs, a.out)


if __name__ == "__main__":
    main()
