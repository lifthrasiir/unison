#!/usr/bin/env python3
"""Populate `font/han-*.unf` with IDC composites that the existing parts allow.

Reads BabelStone's `IDS.TXT` (the decompositions) and Unihan's `kRSUnicode` (the
radical-stroke that decides which slice file and which `## R.S` heading a glyph
goes under), and writes one glyph block per character that

  * has no glyph in `font/` yet,
  * decomposes, at the top level, into one of the four one-dimensional IDCs
    (`⿰⿱⿲⿳` — the only ones `compose.rs` implements) with single-character
    operands, and
  * has every operand drawn in the source, at any size — a size that does not
    fit the box only comments the line out, since a declaration is what says
    which parts are still to be drawn.

An operand that splits along the same axis as the whole is *inlined* rather
than lost: `⿰⿰XYC` is written as `⿲XYC` and `⿱⿱XYC` as `⿳XYC`. Only one
operand of a line can be, since four parts have no IDC of their own;
`--no-inline` turns it off. `--inline-parts` extends it to an operand named by
a *character* that decomposes the same way (`⿰BC` with `B = ⿰XY`), which is
off by default because it gives up on drawing that part -- it would write 刊 as
`⿲干丨亅` rather than as 干 beside a 刂 nobody has drawn yet. Such a line keeps
the un-inlined one it came from above it, commented out:

    glyph han-520a:15x16 15 16 // 刊
    // ⿰ han-5e72 han-5202 // ⿰干刂
    ⿲ han-5e72 han-4e28 han-4e85 // ⿲干丨亅

so that undoing the inline by hand is deleting the last line and commenting the
first out -- what is left is this script's own shape for a request to draw 刂,
and a later run leaves it alone rather than inlining it again. (A sequence's own
nesting, `⿰⿰XYC`, has no such line: its operand is not a character, so nothing
names it. Those keep the `<- ⿰⿰XYC` note instead.)

The emitted line leaves its components *undecided* (no `:WxH` suffix), which is
the initial state `compose.rs` documents for a glyph populated from IDS; run
`cargo run -r -- fix -i font/ --optimize-clearance` afterwards to pick the
variants and the gaps.

A line the parts cannot lay out yet is emitted commented out (`// glyph …`), so
that nothing is lost and nothing unbuildable is added either: either because its
parts are drawn at no size that tiles the box (`--ignore-box` writes it anyway),
or because the only parts it has are themselves composites
(`--include-composite-parts`).

Such a line is a request, not a drawing, so a later run *revives* it: a character
an earlier run left commented out is put through the same feasibility test again,
and once the parts allow the line -- a narrower variant of an operand having been
drawn in the meantime -- the block is rewritten uncommented in place, from
whichever sequence today's parts make best. Only a block this script itself wrote
is rewritten that way; one that has been edited by hand is left alone and
reported.

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

# The two that split the box left to right; the other two split it top to bottom.
HORIZONTAL = {"⿰", "⿲"}

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
    # code points a *commented-out* block declares -- a line an earlier run (or
    # a hand) left as a request to draw the parts. It is not a drawing, so it
    # lends no variant to anything, but it is a declaration, so writing it again
    # would only duplicate it.
    declared: set[int] = field(default_factory=set)
    # cp -> (slice file stem, the block's lines), for a commented-out block that
    # is this script's own output verbatim and so can be *revived*: rewritten
    # uncommented once the parts allow the line. A block written by hand, or one
    # a pattern declares several characters through, is left out of this.
    declared_blocks: dict[int, tuple[str, list[str]]] = field(default_factory=dict)


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
            commented = line.lstrip("/ ")
            if line.startswith("//") and commented.startswith("glyph "):
                toks = commented.split()
                cps = []
                for expanded in expand_pattern(toks[1], parts) if len(toks) > 1 else []:
                    m = NAME_CP_RE.match(split_variant(expanded)[0])
                    if m:
                        cps.append(int(m.group(1), 16))
                inv.declared.update(cps)
                if len(cps) == 1 and (i == 0 or not lines[i - 1].strip()):
                    end = i + 1
                    while end < len(lines) and lines[end].strip():
                        end += 1
                    inv.declared_blocks[cps[0]] = (fname[:-4], lines[i:end])
                continue
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
# inlining a nested component
# --------------------------------------------------------------------------

# `⿰⿰XYC` is the same split of the box as `⿲XYC`, and `⿱⿱XYC` the same as
# `⿳XYC`, so a binary IDC whose own operand splits along the *same* axis is
# rewritten into the ternary one rather than dropped for naming a part nothing
# draws. Only one operand can be inlined -- four parts have no IDC of their own
# -- and only one level deep, for that same reason.
INLINE_OP = {"⿰": "⿲", "⿱": "⿳"}


def render_ids(node: "Node") -> str:
    """A tree back to the sequence it was parsed from."""
    if node.char is not None:
        return node.char
    return node.op + "".join(render_ids(kid) for kid in node.kids)


def binary_split(node: "Node") -> tuple[str, list[str]] | None:
    """`⿰XY` with two single-character operands, or None."""
    if node.op in INLINE_OP and all(kid.char is not None for kid in node.kids):
        return node.op, [kid.char for kid in node.kids]
    return None


def build_split_index(ids: dict[int, "IdsEntry"], allow_ivi: bool) -> dict[int, tuple[str, list[int]]]:
    """code point -> the same-axis binary split its own IDS gives it.

    This is what lets an operand *named by a character* be inlined as well: `A
    = ⿰BC` where the source draws no `B`, but `B` is itself `⿰XY`, becomes
    `⿲XYC`. A character's best-attested sequence wins, as everywhere else here.
    """
    out: dict[int, tuple[str, list[int]]] = {}
    for cp, entry in ids.items():
        for seq, tags in sorted(entry.seqs, key=lambda s: -tag_score(s[1])):
            if "？" in seq or "{" in seq:
                continue
            if "〾" in seq:
                if not allow_ivi:
                    continue
                seq = seq.replace("〾", "")
            tree, used = parse_ids(seq)
            if tree is None or used != len(seq):
                continue
            split = binary_split(tree)
            if split is None:
                continue
            comps = [ord(normalize_component(c)) for c in split[1]]
            if cp in comps:
                continue
            out[cp] = (split[0], comps)
            break
    return out


@dataclass
class Candidate:
    """One way of writing a node as an IDC line the source can hold."""

    op: str
    comps: list[int]
    inlined: bool
    # the line this one was inlined *from*, when that line is writable at all:
    # `⿰BC` for `⿲XYC` where `B = ⿰XY`. A sequence's own nesting (`⿰⿰XYC`)
    # has none -- its operand is not a character, and so has no name to write.
    alt: tuple[str, list[int]] | None = None


def candidates(tree: "Node", inline: bool,
               splits: dict[int, tuple[str, list[int]]] | None) -> list[Candidate]:
    """Every component list one IDS node can be written as, plainest first.

    The plain one is the node itself when every operand is a character; the
    others each inline one operand that splits along the same axis. The
    sequence's own nesting (`⿰⿰XYC`) is inlined whenever `inline` is set, since
    there the sequence itself says the box splits three ways. A *character*
    standing for such a split (`⿰BC` where `B` is itself `⿰XY`) is only
    inlined when `splits` is given, because that one is a judgement about the
    part rather than about this character: 刂 decomposes into 丨 and 亅, but a
    line that says so has given up on ever drawing 刂.
    """
    out: list[Candidate] = []
    if all(kid.char is not None for kid in tree.kids):
        out.append(Candidate(
            tree.op, [ord(normalize_component(kid.char)) for kid in tree.kids], False
        ))
    ternary = INLINE_OP.get(tree.op)
    if ternary is None or not inline:
        return out
    for i, kid in enumerate(tree.kids):
        other = tree.kids[1 - i]
        if other.char is None:
            continue  # both operands nested: four parts, and no IDC for them
        split = binary_split(kid)
        if split is not None:
            if split[0] != tree.op:
                continue  # nested, but splitting the other way
            inner = [ord(normalize_component(c)) for c in split[1]]
            alt = None
        elif kid.char is not None and splits is not None:
            got = splits.get(ord(normalize_component(kid.char)))
            if got is None or got[0] != tree.op:
                continue
            inner = list(got[1])
            alt = (tree.op, [ord(normalize_component(k.char)) for k in tree.kids])
        else:
            continue  # nested, but along the other axis
        rest = ord(normalize_component(other.char))
        comps = inner + [rest] if i == 0 else [rest] + inner
        out.append(Candidate(ternary, comps, True, alt))
    return out


# --------------------------------------------------------------------------
# Unihan kRSUnicode
# --------------------------------------------------------------------------

RS_RE = re.compile(r"^(\d+)('?)\.(-?\d+)$")


def load_rs(path: str) -> tuple[dict[int, tuple[int, int, int]], dict[tuple[int, int], str]]:
    """code point -> (radical, prime, additional strokes), from kRSUnicode.

    Also the character each *prime* radical is written as, which the Kangxi
    radical block does not give: a simplified radical is only ever named by the
    ideograph whose own radical-stroke is `R'.0` (149' -> 讠, 167' -> 钅).
    """
    out: dict[int, tuple[int, int, int]] = {}
    prime_chars: dict[tuple[int, int], str] = {}
    with open_text(path, "utf-8") as f:
        for line in f:
            if not line.startswith("U+") or "kRSUnicode" not in line:
                continue
            cp_s, key, value = line.rstrip("\n").split("\t")
            if key != "kRSUnicode":
                continue
            cp = int(cp_s[2:], 16)
            for i, tok in enumerate(value.split()):
                m = RS_RE.match(tok)
                if not m:
                    continue
                rs = (int(m.group(1)), 1 if m.group(2) else 0, int(m.group(3)))
                if i == 0:
                    out[cp] = rs
                if rs[1] and rs[2] == 0:
                    prev = prime_chars.get(rs[:2])
                    if prev is None or cp < ord(prev):
                        prime_chars[rs[:2]] = chr(cp)
    return out, prime_chars


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


def load_radical_chars(font_dir: str) -> dict[tuple[int, int], str]:
    """What the source already writes each radical heading's character as."""
    out: dict[tuple[int, int], str] = {}
    for fname in sorted(os.listdir(font_dir)):
        if not fname.endswith(".unf"):
            continue
        with open(os.path.join(font_dir, fname), encoding="utf-8") as f:
            for line in f:
                m = RADICAL_HEADING_RE.match(line.rstrip("\r\n"))
                if m:
                    key = (int(m.group("radical")), 1 if m.group("prime") else 0)
                    out.setdefault(key, m.group("char"))
    return out


def radical_char(radical: int, prime: int, known: dict[tuple[int, int], str]) -> str:
    """The character a `# ...: X (R)` heading names its radical by.

    What the source already writes wins, so a new heading matches the ones
    beside it; a radical no file has yet falls back to the Kangxi radical (or,
    for a prime one, to what `load_rs` harvested).
    """
    ch = known.get((radical, prime))
    if ch is not None:
        return ch
    if prime:
        raise KeyError(f"no character known for radical {radical}'")
    return unicodedata.normalize("NFKC", chr(0x2F00 + radical - 1))


# --------------------------------------------------------------------------
# feasibility
# --------------------------------------------------------------------------

@dataclass
class Verdict:
    """What the parts allow for one line."""

    kind: str | None  # "handdrawn", "composite", or None when nothing does
    fits: bool  # some combination of what is drawn tiles the box today
    reason: str  # why `kind` is None


def feasible(inv: Inventory, op: str, comps: list[int]) -> Verdict:
    """Ask the parts about one line, in two independent questions.

    *Existence* decides whether the line is written at all. A declaration is
    what says which character is to be drawn, so it comes before the drawing: a
    line whose parts are all drawn at 15x16 and so cannot tile the box yet is
    exactly the line that asks for a narrow variant to be drawn, and dropping it
    would hide the request.

    *Fit* decides whether it is written commented out, and mirrors
    `compose::fits_slot`: a variant can go in a slot when its extent across the
    split is the glyph's and its extent along the split is strictly shorter than
    the glyph's — a part as long as the glyph fills it on its own and leaves the
    rest of the line nowhere to stand — and the line fits when the smallest such
    variants still tile the box together. Which variant is actually written is
    `uniform fix --optimize-clearance`'s to choose.
    """
    horizontal = op in HORIZONTAL
    axis, cross = (BOX_W, BOX_H) if horizontal else (BOX_H, BOX_W)
    all_hand = True
    total = 0
    fits = True
    for cp in comps:
        cands = inv.variants.get(han_name(cp), [])
        if not cands:
            return Verdict(None, False, "component not drawn")
        if not any(v.handdrawn for v in cands):
            all_hand = False
        along = [
            (v.w if horizontal else v.h)
            for v in cands
            if (v.h if horizontal else v.w) == cross
            and (v.w if horizontal else v.h) < axis
        ]
        if along:
            total += min(along)
        else:
            fits = False
    return Verdict("handdrawn" if all_hand else "composite", fits and total <= axis, "")


# --------------------------------------------------------------------------
# the .unf files
# --------------------------------------------------------------------------

# The two heading levels a slice file is written in: `#` opens a radical and
# `##` a stroke count within it (both are comments to every build stage --
# `document_io.rs`). A `## R.S` only ever stands under the `# ...: X (R)` of its
# own radical, so a section for a radical the file does not have yet has to open
# that radical's heading first, in radical order, rather than being dropped
# under whichever heading happens to precede it.
RADICAL_HEADING_RE = re.compile(
    r"^#\s+(?P<block>.*?):\s*(?P<char>\S+)\s*\((?P<radical>\d+)(?P<prime>'?)\)\s*$"
)
HEADING_RE = re.compile(r"^##\s+(\d+)('?)\.(-?\d+)\s*$")


@dataclass
class Section:
    heading: str
    key: tuple  # (radical, prime, strokes)
    blocks: list[list[str]]


@dataclass
class Group:
    """One `# ...: 火 (86)` radical heading and the `## R.S` sections under it."""

    heading: str
    key: tuple  # (radical, prime)
    sections: list[Section]


@dataclass
class SliceFile:
    path: str
    letter: str
    # (radical, prime) -> the character a heading names it by
    rad_chars: dict[tuple[int, int], str]
    groups: list[Group]
    # anything before the first `## R.S`, kept verbatim: a file that has any
    # fails the round-trip check below and is left alone rather than reshuffled
    stray: list[list[str]]
    # blank lines a file happens to end with, kept so a rewrite is a pure
    # insertion rather than a whitespace tidy-up
    trailing: int = 0

    @classmethod
    def load(cls, path: str, letter: str, rad_chars: dict) -> "SliceFile":
        with open(path, encoding="utf-8") as f:
            text = f.read()
        lines = text.split("\n")
        trailing = 0
        while lines and lines[-1] == "":
            lines.pop()
            trailing += 1
        groups: list[Group] = []
        stray: list[list[str]] = []
        cur_group: Group | None = None
        cur_sec: Section | None = None
        block: list[str] = []

        def flush():
            nonlocal block
            if block:
                (cur_sec.blocks if cur_sec else stray).append(block)
                block = []

        for line in lines:
            m = HEADING_RE.match(line)
            if m:
                flush()
                key = (int(m.group(1)), 1 if m.group(2) else 0, int(m.group(3)))
                cur_sec = Section(line.rstrip(), key, [])
                if cur_group is None or cur_group.key != key[:2]:
                    # a section under no heading of its own: leave it where it
                    # is (the round-trip check will skip the file)
                    cur_group = Group("", key[:2], [])
                    groups.append(cur_group)
                cur_group.sections.append(cur_sec)
                continue
            m = RADICAL_HEADING_RE.match(line)
            if m:
                flush()
                cur_sec = None
                cur_group = Group(
                    line.rstrip(),
                    (int(m.group("radical")), 1 if m.group("prime") else 0),
                    [],
                )
                groups.append(cur_group)
                continue
            if not line.strip():
                flush()
                continue
            block.append(line)
        flush()
        return cls(path, letter, rad_chars, groups, stray, max(0, trailing - 1))

    def dumps(self) -> str:
        out: list[str] = []
        for b in self.stray:
            if out:
                out.append("")
            out.extend(b)
        for g in self.groups:
            if g.heading:
                if out:
                    out.append("")
                out.append(g.heading)
            for sec in g.sections:
                if out:
                    out.append("")
                out.append(sec.heading)
                for b in sec.blocks:
                    out.append("")
                    out.extend(b)
        if not out:
            return ""
        return "\n".join(out) + "\n" * (1 + self.trailing)

    def group_for(self, key: tuple) -> Group:
        for g in self.groups:
            if g.key == key:
                return g
        radical, prime = key
        heading = (f"# {block_of_letter_name(self.letter)}: "
                   f"{radical_char(radical, prime, self.rad_chars)} "
                   f"({radical}{chr(39) if prime else ''})")
        g = Group(heading, key, [])
        pos = len(self.groups)
        for i, other in enumerate(self.groups):
            if other.key > key:
                pos = i
                break
        self.groups.insert(pos, g)
        return g

    def section_for(self, key: tuple) -> Section:
        group = self.group_for(key[:2])
        for sec in group.sections:
            if sec.key == key:
                return sec
        radical, prime, strokes = key
        heading = f"## {radical}{chr(39) if prime else ''}.{strokes}"
        sec = Section(heading, key, [])
        pos = len(group.sections)
        for i, s in enumerate(group.sections):
            if s.key > key:
                pos = i
                break
        group.sections.insert(pos, sec)
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


def idc_line(op: str, comps: list[int]) -> str:
    """One IDC line: the component names, and the sequence they spell."""
    names = " ".join(han_name(c) for c in comps)
    return f"{op} {names} // " + op + "".join(chr(c) for c in comps)


def build_blocks(op: str, comps: list[int], cp: int, char: str, commented: bool,
                 origin: str | None = None,
                 alt: tuple[str, list[int]] | None = None) -> list[str]:
    head = f"glyph {han_name(cp)}:{BOX_W}x{BOX_H} {BOX_W} {BOX_H} // {char}"
    if commented:
        # an inlined line keeps the sequence it came from beside the one it
        # draws, since the two are the same split but not the same text
        body = idc_line(op, comps) + (f" <- {origin}" if origin else "")
        return [f"// {head}", f"// {body}"]
    if alt is not None:
        # An inline that gave up on a part *nameable* keeps that un-inlined line
        # above the one it drew, commented out. Undoing the inline by hand is
        # then the whole of deleting the last line and commenting the first out:
        # what is left is a request for the part, in the shape this script
        # writes one -- which `is_script_block` recognizes, and which the revive
        # below then declines to inline again.
        return [head, f"// {idc_line(*alt)}", idc_line(op, comps)]
    return [head, idc_line(op, comps) + (f" <- {origin}" if origin else "")]


def is_script_block(cp: int, char: str, block: list[str]) -> bool:
    """Whether a commented-out block is this script's own output.

    Not by comparing it to what today's parts produce: an earlier run may well
    have written the character from another of its sequences, and picking a
    different one now is precisely what a newly drawn part does. So the block is
    regenerated from *itself* -- a block that survives that is one this script
    wrote, and so one it may rewrite; anything else is a hand edit and says
    something this script does not know.
    """
    got = script_block_idc(block)
    if got is None:
        return False
    op, comps, origin = got
    return block == build_blocks(op, comps, cp, char, True, origin)


def script_block_idc(block: list[str]) -> tuple[str, list[int], str | None] | None:
    """The `(op, components, origin)` a commented-out two-line block states."""
    if len(block) != 2:
        return None
    body = block[1].lstrip("/ ")
    head, _, rest = body.partition("//")
    toks = head.split()
    if not toks or toks[0] not in IDC_ARITY:
        return None
    comps = []
    for tok in toks[1:]:
        m = NAME_CP_RE.match(tok)
        if not m:
            return None
        comps.append(int(m.group(1), 16))
    origin = rest.split("<-", 1)[1].strip() if "<-" in rest else None
    return toks[0], comps, origin


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
    ap.add_argument("--no-inline", action="store_true",
                    help="do not rewrite a same-axis nested operand into ⿲/⿳")
    ap.add_argument("--inline-parts", action="store_true",
                    help="also inline an operand named by a character whose own IDS "
                         "splits the same way (⿰BC with B = ⿰XY -> ⿲XYC)")
    ap.add_argument("--ignore-box", action="store_true",
                    help="write a line whose parts cannot tile the 15x16 box uncommented as well")
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
    rs, prime_chars = load_rs(args.unihan)
    inline = not args.no_inline
    splits = build_split_index(ids, args.allow_ivi) if inline and args.inline_parts else None
    rad_chars = {**prime_chars, **load_radical_chars(args.font_dir)}

    print(f"{len(inv.variants)} part families, {len(inv.covered)} characters drawn, "
          f"{len(slices)} slices, {len(ids)} IDS entries", file=sys.stderr)

    stats = defaultdict(int)
    # slice name -> list of (rs key, cp, block lines)
    plan: dict[str, list[tuple[tuple, int, list[str]]]] = defaultdict(list)
    # slice name -> the code points whose commented-out block that file is to
    # lose, because the plan writes the same line uncommented instead
    removals: dict[str, set[int]] = defaultdict(set)
    made = 0

    for cp in sorted(ids):
        if cp in inv.covered:
            stats["already drawn"] += 1
            continue
        # A commented-out block is a request, not a drawing: the day a part it
        # was waiting for is drawn, the very same line becomes writable. So a
        # declared character is reconsidered rather than skipped -- skipping it
        # is what left a newly drawn narrow variant unreachable by every line
        # that had asked for one.
        revive = cp in inv.declared
        blk = block_of(cp)
        if blk is None:
            stats["outside the han slices"] += 1
            continue
        letter, block_name = blk
        entry = ids[cp]

        best: tuple[tuple[bool, bool, bool], Candidate, Verdict] | None = None
        best_origin: str | None = None
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
            cands = candidates(tree, inline, splits)
            if not cands:
                why = min(why, "a nested component nothing names", key=reason_rank)
                continue
            for cand in cands:
                if cp in cand.comps:
                    why = min(why, "self-referential IDS", key=reason_rank)
                    continue
                verdict = feasible(inv, cand.op, cand.comps)
                if verdict.kind is None:
                    why = min(why, verdict.reason, key=reason_rank)
                    continue
                # A character with several sequences takes the one that can be
                # laid out today over one that only could be later, source tags
                # being the tie-break the sort already applied; and, all else
                # equal, the sequence as written over one that inlined an
                # operand into it.
                rank = (verdict.fits, verdict.kind == "handdrawn", not cand.inlined)
                if best is None or rank > best[0]:
                    best = (rank, cand, verdict)
                    best_origin = render_ids(tree) if cand.inlined else None
            if best is not None and best[0] == (True, True, True):
                break
        if best is None:
            stats["skipped: " + why] += 1
            continue
        _, cand, verdict = best
        op, comps, origin = cand.op, cand.comps, best_origin

        key = rs.get(cp)
        if key is None:
            stats["no kRSUnicode"] += 1
            continue
        sl = slice_for(slices, letter, key)
        if sl is None:
            stats["no slice"] += 1
            continue

        holds = []
        if verdict.kind == "composite" and not args.include_composite_parts:
            holds.append("composite parts")
        if not verdict.fits and not args.ignore_box:
            holds.append("parts do not fit the box")
        block = build_blocks(op, comps, cp, entry.char, bool(holds), origin, cand.alt)
        if revive:
            # Only a line that is now unheld is worth rewriting, and only where
            # the block in the file is this script's own output: anything else
            # is a hand edit, and replacing it would throw away what it says.
            if holds:
                stats["already declared (commented out)"] += 1
                continue
            old = inv.declared_blocks.get(cp)
            if old is None or not is_script_block(cp, entry.char, old[1]):
                stats["declared, now writable, but not this script's own block"] += 1
                continue
            got = script_block_idc(old[1])
            if cand.alt is not None and got is not None and got[:2] == (
                    cand.alt[0], list(cand.alt[1])):
                # the block already *is* the un-inlined line this candidate came
                # from, commented out: a hand undid the inline, and writing the
                # inlined line back is exactly what that asked not to happen
                stats["declared, un-inlined by hand"] += 1
                continue
            removals[old[0]].add(cp)
            stats["revived (was commented out)"] += 1
        plan[sl.name].append((key, cp, block))
        stats["composite parts" if verdict.kind == "composite" else "hand-drawn parts"] += 1
        if origin:
            stats["inlined a nested operand"] += 1
        for hold in holds:
            stats["commented out: " + hold] += 1
        made += 1
        if args.verbose:
            note = ", ".join([verdict.kind] + (["revived"] if revive else [])
                             + (["inlined"] if origin else []) + holds)
            print(f"  {entry.char} U+{cp:04X} -> {sl.name} {key[0]}{"'" if key[1] else ''}.{key[2]}"
                  f"  {op}{''.join(chr(c) for c in comps)} ({note})", file=sys.stderr)
        if args.limit and made >= args.limit:
            break

    for name in sorted(set(plan) | set(removals)):
        items = plan.get(name, [])
        drop = removals.get(name, set())
        path = os.path.join(args.font_dir, name + ".unf")
        sl = next((s for s in slices if s.name == name), None)
        if sl is None:
            print(f"error: {path} is no slice han.unf lists; skipping it", file=sys.stderr)
            stats["files skipped (unknown slice)"] += 1
            continue
        if os.path.exists(path):
            sf = SliceFile.load(path, sl.letter, rad_chars)
            before = sf.dumps()
            with open(path, encoding="utf-8") as f:
                original = f.read()
            if before != original:
                print(f"error: {path} does not round-trip through this script; "
                      f"skipping it rather than reformatting it", file=sys.stderr)
                stats["files skipped (round-trip)"] += 1
                continue
        else:
            # a slice with no file yet: `section_for` opens its headings
            sf = SliceFile(path, sl.letter, rad_chars, [], [])
        if drop:
            # the revived line replaces the commented-out one wherever it sits,
            # which is not necessarily the section the new one goes into
            for g in sf.groups:
                for sec in g.sections:
                    sec.blocks = [
                        b for b in sec.blocks
                        if not (block_cp(b) in drop and b[0].lstrip().startswith("//"))
                    ]
        for key, cp, block in sorted(items, key=lambda it: (it[0], it[1])):
            insert_block(sf.section_for(key), cp, block)
        if not args.dry_run:
            with open(path, "w", encoding="utf-8") as f:
                f.write(sf.dumps())
        note = f"+{len(items)}" + (f", {len(drop)} revived" if drop else "")
        print(f"{'would write' if args.dry_run else 'wrote'} {path}: {note}", file=sys.stderr)

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
