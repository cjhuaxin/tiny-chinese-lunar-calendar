#!/usr/bin/env python3
"""Regenerate the embedded LXGW WenKai subsets in ui/fonts/.

Weather-alert text arrives from the network and can contain any common hanzi,
so the subset covers GB2312 level-1 plus the GB2312 symbol rows rather than a
hand-maintained character list. Anything outside the subset falls back to a
system font mid-string, which is visible as mixed typefaces.

Usage:
    pip install fonttools
    python3 scripts/subset-fonts.py /path/to/full/LXGWWenKai-{Regular,Medium}.ttf

Full TTFs: https://github.com/lxgw/LxgwWenKai/releases
"""

import sys
from pathlib import Path

from fontTools import subset as ftsubset
from fontTools.ttLib import TTFont

REPO = Path(__file__).resolve().parent.parent
OUT_DIR = REPO / "ui" / "fonts"


def gb2312_rows(first_lo: int, first_hi: int) -> set[str]:
    """Characters from a range of GB2312 leading bytes."""
    chars = set()
    for lead in range(first_lo, first_hi + 1):
        for trail in range(0xA1, 0xFF):
            try:
                chars.add(bytes([lead, trail]).decode("gb2312"))
            except UnicodeDecodeError:
                continue
    return chars


def target_charset() -> set[str]:
    symbols = gb2312_rows(0xA1, 0xA9)  # punctuation, roman numerals, kana, etc.
    level1 = gb2312_rows(0xB0, 0xD7)  # 3755 most common hanzi
    ascii_printable = {chr(c) for c in range(0x20, 0x7F)}
    # Degree sign and friends used by the weather badge.
    extras = set("°·—…‰′″℃№")
    return symbols | level1 | ascii_printable | extras


def existing_charset(weight: str) -> set[str]:
    path = OUT_DIR / f"LXGWWenKai-{weight}.ttf"
    if not path.exists():
        return set()
    return {chr(c) for c in TTFont(path, lazy=True).getBestCmap()}


def build(full_ttf: Path, chars: set[str]) -> None:
    weight = "Medium" if "Medium" in full_ttf.name else "Regular"
    out = OUT_DIR / f"LXGWWenKai-{weight}.ttf"

    options = ftsubset.Options()
    options.layout_features = ["*"]
    options.name_IDs = ["*"]
    options.notdef_outline = True
    options.recalc_bounds = True

    font = ftsubset.load_font(str(full_ttf), options)
    subsetter = ftsubset.Subsetter(options=options)
    subsetter.populate(text="".join(sorted(chars)))
    subsetter.subset(font)
    ftsubset.save_font(font, str(out), options)

    print(f"{out.name}: {len(chars)} chars, {out.stat().st_size / 1024:.0f} KB")


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 1

    chars = target_charset()
    # Never drop a glyph the current subsets already ship.
    for weight in ("Regular", "Medium"):
        chars |= existing_charset(weight)

    for arg in sys.argv[1:]:
        build(Path(arg), chars)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
