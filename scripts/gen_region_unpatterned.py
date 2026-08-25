#!/usr/bin/env python3
"""List the drawn han characters whose IDS has a GHTJKPV region difference that
`font/` does not write as a `$han-regions` pattern.

A character with a region difference is drawn once per region in Unison -- as a
`glyph han-XXXX-($han-regions):15x16` block whose components pick the region up
(`han-4ee4-($-1)`), or as an explicit `han-XXXX-k` and friends. A character that
BabelStone's IDS.TXT gives more than one region-tagged sequence for, and that
`font/` draws with a plain `han-XXXX:15x16` name and no region-suffixed sibling
at all, is one of two things: a region difference nobody has noticed yet, or one
the drawing happens not to need. This script lists them, grouped by *what*
differs between the sequences, so that a whole family can be settled at once.

The grouping key is the multiset difference: the non-IDC components every
variant shares are dropped, and what each variant is left with is sorted and
joined with ` / `. Variants left with nothing differ only in structure.

The report goes to standard output; it is a thing to read rather than a build
input, so nothing writes it to a file.

Usage: python3 scripts/gen_region_unpatterned.py
"""

from __future__ import annotations

import gzip
import os
import re
import sys
from collections import Counter, defaultdict

IDS_PATH = os.path.join("data", "IDS.TXT.gz")
FONT_DIR = "font"

# The seven region tags `$han-regions` names. Every other tag in the file
# (M, U, S, B, X, Z, UCS2003) is a source Unison does not slice by, so a
# sequence carrying only those is not a region difference.
REGIONS = "GHTJKPV"

# Everything that is structure rather than a component: the IDCs, the variation
# indicator, and the unrepresentable-component mark.
STRUCTURE = set("⿰⿱⿲⿳⿴⿵⿶⿷⿸⿹⿺⿻⿼⿽⿾⿿㇯〾？")

# IDS.TXT writes an unencoded component as `{NN}`, which is several characters
# for one component; sorting a variant's leftovers would tear it into `88{}`.
# So a component is carried as *one* character throughout -- `{NN}` standing in
# as `PUA_BASE + NN` in a supplementary private-use area -- and put back only
# where the key is finally written out. The plane is this script's own choice
# and means nothing outside it: BabelStone's own PUA assignments are not
# contiguous, and a component's number is the order worth sorting by anyway.
PUA_BASE = 0xF0000

GLYPH_RE = re.compile(r"^glyph\s+han-([0-9a-f]{4,5})(-\S*?|\.\S*?)?:(\d+)x(\d+)")
IDS_LINE_RE = re.compile(r"^U\+([0-9A-F]{4,6})\t(\S+)\t(.*)$")
SEQ_RE = re.compile(r"\^(.*?)\$(?:\(([^)]*)\))?")


def open_text(path: str):
    if path.endswith(".gz"):
        return gzip.open(path, "rt", encoding="utf-8")
    return open(path, encoding="utf-8")


def scan_font() -> tuple[set[int], set[int]]:
    """Return (drawn at the full box, has any region-suffixed block)."""
    drawn: set[int] = set()
    regioned: set[int] = set()
    for name in sorted(os.listdir(FONT_DIR)):
        if not (name.startswith("han-") and name.endswith(".unf")):
            continue
        with open(os.path.join(FONT_DIR, name), encoding="utf-8") as f:
            for line in f:
                m = GLYPH_RE.match(line)
                if not m:
                    continue
                cp = int(m.group(1), 16)
                suffix = m.group(2) or ""
                if suffix.startswith("-"):
                    regioned.add(cp)
                elif not suffix and (m.group(3), m.group(4)) == ("15", "16"):
                    drawn.add(cp)
    return drawn, regioned


def components(ids: str) -> list[str]:
    """The non-structural components of a sequence, one character each.

    An unencoded `{NN}` becomes its private-use stand-in, so that everything
    downstream -- the multiset arithmetic and the sort -- sees one component as
    one character. `spell` is the way back.
    """
    out: list[str] = []
    i = 0
    while i < len(ids):
        c = ids[i]
        if c == "{":
            j = ids.index("}", i)
            out.append(chr(PUA_BASE + int(ids[i + 1 : j])))
            i = j + 1
        elif c in STRUCTURE:
            i += 1
        else:
            out.append(c)
            i += 1
    return out


def spell(text: str) -> str:
    """Undo `components`' stand-ins, writing each one back as `{NN}`."""
    return "".join(
        f"{{{ord(c) - PUA_BASE}}}" if ord(c) >= PUA_BASE else c for c in text
    )


def read_ids(wanted: set[int]) -> dict[int, list[tuple[str, str]]]:
    """`cp -> [(sequence, its region tags)]`, only where two or more differ."""
    out: dict[int, list[tuple[str, str]]] = {}
    with open_text(IDS_PATH) as f:
        for line in f:
            if line.startswith("#"):
                continue
            m = IDS_LINE_RE.match(line.rstrip("\r\n"))
            if not m:
                continue
            cp = int(m.group(1), 16)
            if cp not in wanted:
                continue
            variants: list[tuple[str, str]] = []
            for seq, tags in SEQ_RE.findall(m.group(3)):
                tags = "".join(t for t in (tags or "") if t in REGIONS)
                if tags:
                    variants.append((seq, tags))
            if len(variants) > 1 and len({s for s, _ in variants}) > 1:
                out[cp] = variants
    return out


def group_key(variants: list[tuple[str, str]]) -> str:
    """What each variant is left with once the shared components are dropped."""
    parts = [components(seq) for seq, _ in variants]
    shared = parts[0]
    for p in parts[1:]:
        shared = list((Counter(shared) & Counter(p)).elements())
    keys = []
    for p in parts:
        rest = Counter(p) - Counter(shared)
        keys.append(spell("".join(sorted(rest.elements()))))
    if not any(keys):
        return "(structure only)"
    return " / ".join(keys)


def main() -> int:
    drawn, regioned = scan_font()
    ids = read_ids(drawn - regioned)

    groups: dict[str, list[int]] = defaultdict(list)
    for cp, variants in ids.items():
        groups[group_key(variants)].append(cp)

    version = "?"
    with open_text(IDS_PATH) as f:
        for line in f:
            if line.startswith("# Unicode Version:"):
                version = line.split(":", 1)[1].strip().split(" ")[0]
                break

    lines = [
        f"Drawn han characters with a GHTJKPV difference in IDS.TXT ({version}) "
        f"that font/ does not write as a $han-regions pattern: {len(ids)}"
    ]
    for key in sorted(groups, key=lambda k: (-len(groups[k]), min(groups[k]))):
        cps = sorted(groups[key])
        lines.append("")
        lines.append(f"== {key}  ({len(cps)})")
        for cp in cps:
            shown = "  ".join(f"{seq}({tags})" for seq, tags in ids[cp])
            lines.append(f"   U+{cp:05X} {chr(cp)}  {shown}")

    sys.stdout.write("\n".join(lines) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
