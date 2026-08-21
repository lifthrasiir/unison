#!/usr/bin/env python3
"""Populate `font/han-*.unf` with IDC composites that the existing parts allow.

Reads BabelStone's `IDS.TXT` (the decompositions) and Unihan's `kRSUnicode` (the
radical-stroke that decides which slice file and which `## R.S` heading a glyph
goes under), and writes one glyph block per character that

  * has no glyph in `font/` yet,
  * decomposes, at the top level, into one of the four one-dimensional IDCs
    (`⿰⿱⿲⿳` — the only ones `compose.rs` implements) with single-character
    operands, and
  * has every operand drawn in the source, at any size — the box is not
    consulted, since a declaration is what says which parts are still to be
    drawn.

The emitted line leaves its components *undecided* (no `:WxH` suffix), which is
the initial state `compose.rs` documents for a glyph populated from IDS; run
`cargo run -r -- fix -i font/ --optimize-clearance` afterwards to pick the
variants and the gaps.

A character whose only usable parts are themselves composites is emitted
commented out (`// glyph …`), so nothing is lost but nothing unbuildable is
added either.

Usage:
    python3 scripts/gen_ids_composites.py [--dry-run] [--limit N]
"""

from __future__ import annotations

import argparse
import gzip
import os
import re
import sys
import unicodedata
from collections import defaultdict
from dataclasses import dataclass, field

# The two inputs, kept in `data/` as gzipped copies of what these URLs serve;
# refresh them by hand rather than from the script.
UNIHAN_URL = "https://www.unicode.org/Public/UCD/latest/ucd/Unihan.zip"
IDS_URL = "https://babelstone.co.uk/CJK/IDS.TXT"
UNIHAN_PATH = os.path.join("data", "Unihan_IRCSources-17.0.0.txt.gz")
IDS_PATH = os.path.join("data", "IDS.TXT.gz")


def open_text(path: str, encoding: str):
    """Open a data file, transparently decompressing a `.gz` one."""
    if path.endswith(".gz"):
        return gzip.open(path, "rt", encoding=encoding)
    return open(path, encoding=encoding)

# The four one-dimensional IDCs and their arity (see `src/compose.rs`).
IDC_ARITY = {"⿰": 2, "⿱": 2, "⿲": 3, "⿳": 3}
# Everything else that may appear in an IDS, with its arity, so that a sequence
# can still be *parsed* (and then rejected) rather than mis-read.
OTHER_ARITY = {
    "⿴": 2, "⿵": 2, "⿶": 2, "⿷": 2, "⿸": 2, "⿹": 2, "⿺": 2,
    "⿻": 2, "⿼": 2, "⿽": 2, "㇯": 2, "⿾": 1, "⿿": 1,
}
ARITY = {**IDC_ARITY, **OTHER_ARITY}

# The box every full-size han glyph declares.
BOX_W, BOX_H = 15, 16

# Unicode blocks that `han-XXXX` names cover, in the slice-id order han.unf uses
# (`0` for the URO, `a`--`j` for the extensions).
HAN_BLOCKS = [
    ("0", 0x4E00, 0x9FFF, "CJK unified ideographs"),
    ("a", 0x3400, 0x4DBF, "CJK unified ideographs extension A"),
    ("b", 0x20000, 0x2A6DF, "CJK unified ideographs extension B"),
    ("c", 0x2A700, 0x2B73F, "CJK unified ideographs extension C"),
    ("d", 0x2B740, 0x2B81F, "CJK unified ideographs extension D"),
    ("e", 0x2B820, 0x2CEAF, "CJK unified ideographs extension E"),
    ("f", 0x2CEB0, 0x2EBEF, "CJK unified ideographs extension F"),
    ("g", 0x30000, 0x3134F, "CJK unified ideographs extension G"),
    ("h", 0x31350, 0x323AF, "CJK unified ideographs extension H"),
    ("i", 0x2EBF0, 0x2EE5F, "CJK unified ideographs extension I"),
    ("j", 0x323B0, 0x3347F, "CJK unified ideographs extension J"),
]
BLOCK_RANK = {letter: i for i, (letter, _, _, _) in enumerate(HAN_BLOCKS)}


def block_of(cp: int) -> tuple[str, str] | None:
    for letter, lo, hi, name in HAN_BLOCKS:
        if lo <= cp <= hi:
            return letter, name
    return None


def han_name(cp: int) -> str:
    return f"han-{cp:04x}"


# --------------------------------------------------------------------------
# name patterns
# --------------------------------------------------------------------------

GROUP_RE = re.compile(r"\(([^()]*)\)")


def expand_pattern(name: str, parts: dict[str, list[str]]) -> list[str]:
    """Expand a glyph-block name pattern into the names it declares.

    Only the subset `font/han-*.unf` uses: `(a|b|c)` alternations and
    `($name-parts)` references, several of which run in lock-step (see
    `src/pattern.rs`). A name with no group expands to itself.
    """
    groups = GROUP_RE.findall(name)
    if not groups:
        return [name]
    choices = []
    for g in groups:
        if g.startswith("$"):
            alts = parts.get(g[1:])
            if alts is None:
                return []  # unknown name-parts: not ours to guess
        else:
            alts = g.split("|")
        choices.append(alts)
    # several groups run in lock-step, and a ragged one repeats its last choice
    # (`pattern.rs`: the groups combine by the largest).
    width = max(len(c) for c in choices)
    out = []
    for i in range(width):
        pieces = []
        rest = name
        for alts in choices:
            m = GROUP_RE.search(rest)
            pieces.append(rest[: m.start()])
            pieces.append(alts[min(i, len(alts) - 1)])
            rest = rest[m.end():]
        pieces.append(rest)
        out.append("".join(pieces))
    return out


SIZE_RE = re.compile(r"^(\d+)x(\d+)$")


def split_variant(name: str) -> tuple[str, tuple[int, int] | None, str | None]:
    """`han-6728:5x16-l` -> (`han-6728`, (5, 16), `l`); see `compose.rs` (D1)."""
    base, sep, spec = name.partition(":")
    if not sep:
        return name, None, None
    size = None
    direction = None
    for tok in spec.split("-"):
        m = SIZE_RE.match(tok)
        if m and size is None:
            size = (int(m.group(1)), int(m.group(2)))
        elif tok in ("l", "r", "u", "d", "c") and direction is None:
            direction = tok
    return base, size, direction


# --------------------------------------------------------------------------
# the font source
# --------------------------------------------------------------------------

@dataclass
class Variant:
    name: str
    w: int
    h: int
    direction: str | None
    handdrawn: bool


@dataclass
class Inventory:
    # base name (`han-6728`) -> the variants the source draws for it
    variants: dict[str, list[Variant]] = field(default_factory=lambda: defaultdict(list))
    # code points that already have a glyph of any kind
    covered: set[int] = field(default_factory=set)
    # code points that have a full-size (15x16) glyph
    covered_full: set[int] = field(default_factory=set)


NAME_CP_RE = re.compile(r"^han-([0-9a-f]{4,5})(?:-[a-z])?(?:\.[0-9a-f]{1,2})?$")


def load_name_parts(font_dir: str) -> dict[str, list[str]]:
    parts: dict[str, list[str]] = {}
    for fname in sorted(os.listdir(font_dir)):
        if not fname.endswith(".unf"):
            continue
        with open(os.path.join(font_dir, fname), encoding="utf-8") as f:
            for line in f:
                m = re.match(r"^name-parts\s+\$(\S+)\s*=\s*(.*?)(?://.*)?$", line.strip())
                if m:
                    parts[m.group(1)] = m.group(2).split()
    return parts


def load_inventory(font_dir: str, parts: dict[str, list[str]]) -> Inventory:
    inv = Inventory()
    for fname in sorted(os.listdir(font_dir)):
        if not fname.endswith(".unf"):
            continue
        path = os.path.join(font_dir, fname)
        with open(path, encoding="utf-8") as f:
            lines = f.read().split("\n")
        for i, line in enumerate(lines):
            if not line.startswith("glyph "):
                continue
            head = line.split("//")[0].strip()
            toks = head.split()[1:]
            if not toks:
                continue
            name = toks[0]
            if len(toks) > 1 and toks[1] == "=":
                continue  # `glyph A = B`: an alias, not a drawing
            if "$1" in name or "$2" in name:
                continue  # an `exists`-scoped wrapper, not a part
            if len(toks) < 3 or not (toks[1].isdigit() and toks[2].isdigit()):
                continue  # no declared `W H`: nothing a component could rest on
            w, h = int(toks[1]), int(toks[2])
            # the body decides whether this is a drawing or a derived glyph
            body = ""
            for j in range(i + 1, len(lines)):
                s = lines[j].strip()
                if not s:
                    break
                if s.startswith("//"):
                    continue
                body = s
                break
            handdrawn = bool(body) and body[0] not in IDC_ARITY and not body.startswith("ref ")
            for expanded in expand_pattern(name, parts):
                base, size, direction = split_variant(expanded)
                m = NAME_CP_RE.match(base)
                if not m:
                    continue
                cp = int(m.group(1), 16)
                inv.covered.add(cp)
                if (w, h) == (BOX_W, BOX_H):
                    inv.covered_full.add(cp)
                if size is not None and size != (w, h):
                    continue  # a lying name; `compose.rs` checks this itself
                inv.variants[base].append(
                    Variant(expanded, w, h, direction, handdrawn)
                )
    return inv


# --------------------------------------------------------------------------
# IDS.TXT
# --------------------------------------------------------------------------

@dataclass
class Node:
    op: str | None
    char: str | None
    kids: list["Node"]


def parse_ids(seq: str) -> tuple[Node | None, int]:
    """Parse one IDS into a tree; returns (tree, consumed) or (None, _)."""
    def go(pos: int) -> tuple[Node | None, int]:
        if pos >= len(seq):
            return None, pos
        ch = seq[pos]
        arity = ARITY.get(ch)
        if arity is None:
            return Node(None, ch, []), pos + 1
        kids = []
        pos += 1
        for _ in range(arity):
            kid, pos = go(pos)
            if kid is None:
                return None, pos
            kids.append(kid)
        return Node(ch, None, kids), pos

    node, pos = go(0)
    return node, pos


KANGXI_LO, KANGXI_HI = 0x2F00, 0x2FD5


def normalize_component(ch: str) -> str:
    """A Kangxi radical stands for its unified ideograph; nothing else moves."""
    if KANGXI_LO <= ord(ch) <= KANGXI_HI:
        nfkc = unicodedata.normalize("NFKC", ch)
        if len(nfkc) == 1:
            return nfkc
    return ch


@dataclass
class IdsEntry:
    cp: int
    char: str
    seqs: list[tuple[str, str]]  # (sequence, source tags)


SEQ_RE = re.compile(r"^\^(.*)\$\((.*)\)$")


def load_ids(path: str) -> dict[int, IdsEntry]:
    out: dict[int, IdsEntry] = {}
    with open_text(path, "utf-8-sig") as f:
        for line in f:
            line = line.rstrip("\r\n")
            if not line or line.startswith("#"):
                continue
            fields = line.split("\t")
            if len(fields) < 3 or not fields[0].startswith("U+"):
                continue
            cp = int(fields[0][2:], 16)
            seqs = []
            for raw in fields[2:]:
                m = SEQ_RE.match(raw.strip())
                if m:
                    seqs.append((m.group(1), m.group(2)))
            if seqs:
                out[cp] = IdsEntry(cp, fields[1], seqs)
    return out


# --------------------------------------------------------------------------
# Unihan kRSUnicode
# --------------------------------------------------------------------------

RS_RE = re.compile(r"^(\d+)('?)\.(-?\d+)$")


def load_rs(path: str) -> dict[int, tuple[int, int, int]]:
    """code point -> (radical, prime, additional strokes), from kRSUnicode."""
    out: dict[int, tuple[int, int, int]] = {}
    with open_text(path, "utf-8") as f:
        for line in f:
            if not line.startswith("U+") or "kRSUnicode" not in line:
                continue
            cp_s, key, value = line.rstrip("\n").split("\t")
            if key != "kRSUnicode":
                continue
            first = value.split()[0]
            m = RS_RE.match(first)
            if not m:
                continue
            out[int(cp_s[2:], 16)] = (int(m.group(1)), 1 if m.group(2) else 0, int(m.group(3)))
    return out


# --------------------------------------------------------------------------
# slices (which file a glyph goes to)
# --------------------------------------------------------------------------

SLICE_TOK_RE = re.compile(r"^(?:han-)?([0-9a-j])(\d{3})(?:\.(\d{2}))?$")


@dataclass(order=True)
class SliceId:
    sort_key: tuple
    letter: str = field(compare=False)
    radical: int = field(compare=False)
    strokes: int | None = field(compare=False)

    @property
    def name(self) -> str:
        stem = f"han-{self.letter}{self.radical:03d}"
        return stem if self.strokes is None else f"{stem}.{self.strokes:02d}"


def make_slice(letter: str, radical: int, strokes: int | None) -> SliceId:
    # a slice id names its *first* character, so an absent `.SS` is -infinity
    key = (BLOCK_RANK[letter], radical, 0, -10**6 if strokes is None else strokes)
    return SliceId(key, letter, radical, strokes)


def load_slices(font_dir: str) -> list[SliceId]:
    """The 150 slice ids han.unf lists, in order."""
    out = []
    with open(os.path.join(font_dir, "han.unf"), encoding="utf-8") as f:
        for line in f:
            s = line.strip()
            if not s.startswith("//"):
                continue
            for tok in s[2:].split():
                m = SLICE_TOK_RE.match(tok)
                if m:
                    out.append(make_slice(
                        m.group(1), int(m.group(2)),
                        int(m.group(3)) if m.group(3) else None,
                    ))
    # the prose above the list names a few slices twice
    seen: dict[str, SliceId] = {}
    for s in out:
        seen.setdefault(s.name, s)
    return sorted(seen.values())


def slice_for(slices: list[SliceId], letter: str, rs: tuple[int, int, int]) -> SliceId | None:
    radical, prime, strokes = rs
    key = (BLOCK_RANK[letter], radical, prime, strokes)
    best = None
    for s in slices:
        if s.letter == letter and s.sort_key <= key:
            best = s
        elif s.sort_key > key:
            break
    return best


def radical_char(radical: int) -> str:
    return unicodedata.normalize("NFKC", chr(0x2F00 + radical - 1))


# --------------------------------------------------------------------------
# feasibility
# --------------------------------------------------------------------------

def feasible(inv: Inventory, op: str, comps: list[int]) -> str | None:
    kind, _ = feasible_detail(inv, op, comps)
    return kind


def feasible_detail(inv: Inventory, op: str, comps: list[int]) -> tuple[str | None, str]:
    """`"handdrawn"`, `"composite"` or None — what the parts allow for this line.

    Only *existence* is asked, never a size. A declaration is what says which
    character is to be drawn, so it comes before the drawing: a line whose parts
    are all drawn at 15x16 and so cannot tile the box yet is exactly the line
    that asks for a narrow variant to be drawn, and dropping it would hide the
    request. Sizes are `compose.rs`'s to check and
    `uniform fix --optimize-clearance`'s to choose, once the variants exist.
    """
    all_hand = True
    for cp in comps:
        cands = inv.variants.get(han_name(cp), [])
        if not cands:
            return None, "component not drawn"
        if not any(v.handdrawn for v in cands):
            all_hand = False
    return ("handdrawn" if all_hand else "composite"), ""


# --------------------------------------------------------------------------
# the .unf files
# --------------------------------------------------------------------------

HEADING_RE = re.compile(r"^##\s+(\d+)('?)\.(-?\d+)\s*$")


@dataclass
class Section:
    heading: str
    key: tuple
    blocks: list[list[str]]


@dataclass
class SliceFile:
    path: str
    preamble: list[str]
    sections: list[Section]
    # blank lines a file happens to end with, kept so a rewrite is a pure
    # insertion rather than a whitespace tidy-up
    trailing: int = 0

    @classmethod
    def load(cls, path: str) -> "SliceFile":
        with open(path, encoding="utf-8") as f:
            text = f.read()
        lines = text.split("\n")
        trailing = 0
        while lines and lines[-1] == "":
            lines.pop()
            trailing += 1
        preamble: list[str] = []
        sections: list[Section] = []
        cur: Section | None = None
        block: list[str] = []

        def flush():
            nonlocal block
            if block:
                (cur.blocks if cur else preamble_blocks).append(block)
                block = []

        preamble_blocks: list[list[str]] = []
        for line in lines:
            m = HEADING_RE.match(line)
            if m:
                flush()
                cur = Section(line.rstrip(), (int(m.group(1)), 1 if m.group(2) else 0, int(m.group(3))), [])
                sections.append(cur)
                continue
            if not line.strip():
                flush()
                continue
            block.append(line)
        flush()
        pre = []
        for b in preamble_blocks:
            if pre:
                pre.append("")
            pre.extend(b)
        return cls(path, pre, sections, max(0, trailing - 1))

    def dumps(self) -> str:
        out = list(self.preamble)
        for sec in self.sections:
            if out:
                out.append("")
            out.append(sec.heading)
            for b in sec.blocks:
                out.append("")
                out.extend(b)
        if not out:
            return ""
        return "\n".join(out) + "\n" * (1 + self.trailing)

    def section_for(self, key: tuple) -> Section:
        for sec in self.sections:
            if sec.key == key:
                return sec
        radical, prime, strokes = key
        heading = f"## {radical}{"'" if prime else ''}.{strokes}"
        sec = Section(heading, key, [])
        pos = len(self.sections)
        for i, s in enumerate(self.sections):
            if s.key > key:
                pos = i
                break
        self.sections.insert(pos, sec)
        return sec


BLOCK_CP_RE = re.compile(r"^(?://\s*)?glyph\s+han-([0-9a-f]{4,5})")


def block_cp(block: list[str]) -> int | None:
    for line in block:
        m = BLOCK_CP_RE.match(line.strip())
        if m:
            return int(m.group(1), 16)
    return None


def insert_block(sec: Section, cp: int, block: list[str]) -> None:
    pos = len(sec.blocks)
    for i, b in enumerate(sec.blocks):
        other = block_cp(b)
        if other is not None and other > cp:
            pos = i
            break
    sec.blocks.insert(pos, block)


# --------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------

# Why a character was skipped, closest-to-usable first: a character with
# several sequences is reported under the one that came nearest to working.
REASON_ORDER = [
    "component not drawn",
    "a nested component nothing names",
    "not a one-dimensional IDC",
    "marked 〾",
    "an unrepresentable component",
    "unparsable IDS",
    "self-referential IDS",
    "no decomposition at all",
]


def reason_rank(reason: str) -> int:
    for i, prefix in enumerate(REASON_ORDER):
        if reason.startswith(prefix):
            return i
    return len(REASON_ORDER)


def tag_score(tags: str) -> int:
    return len(set(re.sub(r"\[.*?\]", "", tags)) & set("GHTJKPV"))


def build_blocks(op: str, comps: list[int], cp: int, char: str, commented: bool) -> list[str]:
    names = " ".join(han_name(c) for c in comps)
    ids = op + "".join(chr(c) for c in comps)
    head = f"glyph {han_name(cp)}:{BOX_W}x{BOX_H} {BOX_W} {BOX_H} // {char}"
    body = f"{op} {names} // {ids}"
    if commented:
        return [f"// {head}", f"// {body}"]
    return [head, body]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-i", "--font-dir", default="font")
    ap.add_argument("--ids", default=IDS_PATH)
    ap.add_argument("--unihan", default=UNIHAN_PATH)
    ap.add_argument("--dry-run", action="store_true", help="report only; write nothing")
    ap.add_argument("--limit", type=int, default=0, help="stop after N new glyphs")
    ap.add_argument("--allow-ivi", action="store_true",
                    help="also use sequences marked 〾 (a component differs in some minor way)")
    ap.add_argument("--include-composite-parts", action="store_true",
                    help="write composite-part glyphs uncommented as well")
    ap.add_argument("-v", "--verbose", action="store_true")
    args = ap.parse_args()

    if not os.path.exists(args.unihan):
        print(f"error: {args.unihan} not found; it is Unihan_IRGSources.txt out of\n"
              f"       {UNIHAN_URL}, gzipped", file=sys.stderr)
        return 1
    if not os.path.exists(args.ids):
        print(f"error: {args.ids} not found; it is {IDS_URL}, gzipped", file=sys.stderr)
        return 1

    parts = load_name_parts(args.font_dir)
    inv = load_inventory(args.font_dir, parts)
    slices = load_slices(args.font_dir)
    ids = load_ids(args.ids)
    rs = load_rs(args.unihan)

    print(f"{len(inv.variants)} part families, {len(inv.covered)} characters drawn, "
          f"{len(slices)} slices, {len(ids)} IDS entries", file=sys.stderr)

    stats = defaultdict(int)
    # slice name -> list of (rs key, cp, block lines)
    plan: dict[str, list[tuple[tuple, int, list[str]]]] = defaultdict(list)
    made = 0

    for cp in sorted(ids):
        if cp in inv.covered:
            stats["already drawn"] += 1
            continue
        blk = block_of(cp)
        if blk is None:
            stats["outside the han slices"] += 1
            continue
        letter, block_name = blk
        entry = ids[cp]

        best: tuple[str, list[int], str] | None = None
        why = "no decomposition at all"
        for seq, tags in sorted(entry.seqs, key=lambda s: -tag_score(s[1])):
            if "？" in seq or "{" in seq:
                why = min(why, "an unrepresentable component", key=reason_rank)
                continue
            if "〾" in seq:
                if not args.allow_ivi:
                    why = min(why, "marked 〾 (use --allow-ivi)", key=reason_rank)
                    continue
                seq = seq.replace("〾", "")
            tree, used = parse_ids(seq)
            if tree is None or used != len(seq):
                why = min(why, "unparsable IDS", key=reason_rank)
                continue
            if tree.op is None:
                why = min(why, "no decomposition at all", key=reason_rank)
                continue
            if tree.op not in IDC_ARITY:
                why = min(why, f"not a one-dimensional IDC ({tree.op})", key=reason_rank)
                continue
            if any(kid.char is None for kid in tree.kids):
                why = min(why, "a nested component nothing names", key=reason_rank)
                continue
            comps = [ord(normalize_component(kid.char)) for kid in tree.kids]
            if cp in comps:
                why = min(why, "self-referential IDS", key=reason_rank)
                continue
            kind, reason = feasible_detail(inv, tree.op, comps)
            if kind is None:
                why = min(why, reason, key=reason_rank)
                continue
            best = (tree.op, comps, kind)
            break
        if best is None:
            stats["skipped: " + why] += 1
            continue
        op, comps, kind = best

        key = rs.get(cp)
        if key is None:
            stats["no kRSUnicode"] += 1
            continue
        sl = slice_for(slices, letter, key)
        if sl is None:
            stats["no slice"] += 1
            continue

        commented = kind == "composite" and not args.include_composite_parts
        block = build_blocks(op, comps, cp, entry.char, commented)
        plan[sl.name].append((key, cp, block))
        stats["composite parts" if kind == "composite" else "hand-drawn parts"] += 1
        made += 1
        if args.verbose:
            print(f"  {entry.char} U+{cp:04X} -> {sl.name} {key[0]}{"'" if key[1] else ''}.{key[2]}"
                  f"  {op}{''.join(chr(c) for c in comps)} ({kind})", file=sys.stderr)
        if args.limit and made >= args.limit:
            break

    for name, items in sorted(plan.items()):
        path = os.path.join(args.font_dir, name + ".unf")
        if os.path.exists(path):
            sf = SliceFile.load(path)
            before = sf.dumps()
            with open(path, encoding="utf-8") as f:
                original = f.read()
            if before != original:
                print(f"error: {path} does not round-trip through this script; "
                      f"skipping it rather than reformatting it", file=sys.stderr)
                stats["files skipped (round-trip)"] += 1
                continue
        else:
            sf = SliceFile(path, [], [])
        if not sf.preamble:
            # a slice with no file yet, or the empty placeholder one
            sl = next(s for s in slices if s.name == name)
            sf.preamble = [f"# {block_of_letter_name(sl.letter)}: "
                           f"{radical_char(sl.radical)} ({sl.radical})"]
        for key, cp, block in sorted(items, key=lambda it: (it[0], it[1])):
            insert_block(sf.section_for(key), cp, block)
        if not args.dry_run:
            with open(path, "w", encoding="utf-8") as f:
                f.write(sf.dumps())
        print(f"{'would write' if args.dry_run else 'wrote'} {path}: +{len(items)}", file=sys.stderr)

    print("\nsummary:", file=sys.stderr)
    for k in sorted(stats):
        print(f"  {stats[k]:7d}  {k}", file=sys.stderr)
    print(f"  {made:7d}  new glyph blocks in {len(plan)} files", file=sys.stderr)
    if not args.dry_run and made:
        print("\nnow run: cargo run -r -- fix -i font/ --optimize-clearance", file=sys.stderr)
    return 0


def block_of_letter_name(letter: str) -> str:
    for l, _, _, name in HAN_BLOCKS:
        if l == letter:
            return name
    raise KeyError(letter)


if __name__ == "__main__":
    sys.exit(main())
