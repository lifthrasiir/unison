#!/usr/bin/env python3
"""Populate `font/han-*.unf` with IDC composites that the existing parts allow.

Reads BabelStone's `IDS.TXT` (the decompositions) and Unihan's `kRSUnicode` (the
radical-stroke that decides which slice file and which `## R.S` heading a glyph
goes under), and writes one glyph block per character that

  * has no glyph in `font/` yet,
  * decomposes, at the top level, into one of the IDCs `compose.rs` implements
    (the four splits `⿰⿱⿲⿳` or the nine enclosures `⿴⿵⿶⿷⿸⿹⿺⿼⿽`) with
    single-character operands, and
  * has every operand drawn in the source, at any size — a size that does not
    fit the box only comments the line out, since a declaration is what says
    which parts are still to be drawn.

An **enclosure** line is written `⿷ han-531a han-4e00` with no offsets, which is
the state `compose.rs` calls unplaced: the components have picked no variant and
the line has picked no placement, and both are for `--optimize-clearance` to
decide. What holds such a line back is not the box but the *cavity*: it is
written uncommented only where the source draws the outer part at 15x16 with a
`:15x16.NxM` cavity and draws the inner part small enough to sit in it. An outer
character drawn only as an ordinary full-size glyph therefore comments the line
out, which is this script's shape for "draw the enclosing variant of 匚".

Inlining (below) is a split's transform and never applies to an enclosure: there
is no ternary enclosure for a nested operand to be folded into.

An operand that splits along the same axis as the whole is *inlined* rather
than lost: `⿰⿰XYC` is written as `⿲XYC` and `⿱⿱XYC` as `⿳XYC`. Only one
operand of a line can be, since four parts have no IDC of their own;
`--no-inline` turns it off. `--inline` extends it to an operand named by a
*character* that decomposes the same way (`⿰BC` with `B = ⿰XY`), which is off
by default because it gives up on drawing that part -- it would write 㟯 as
`⿳山宀各` rather than as 山 over a 客 nobody has drawn yet.

A character the source draws **in pixels** is never opened up that way, however
plainly its own IDS decomposes: those pixels are a drawing, and cutting one in
two so that the halves can be sized apart is a judgement for a hand rather than
for this script -- 出 stays 出 and does not become 屮 over 凵. Only a character
with no pixels of its own is inlined, which is why a compound *containing* such
a drawing still is: 客 is composed rather than drawn, so opening it up says
nothing about how 宀 or 各 are drawn.

Such a line keeps the un-inlined one it came from above it, commented out:

    glyph han-37ef:15x16 15 16 // 㟯
    // ⿱ han-5c71 han-5ba2 // ⿱山客 -- no-inline
    ⿳ han-5c71 han-5b80 han-5404 // ⿳山宀各

Such a block does not parse, deliberately: a comment between a `glyph` header
and its body ends the block, so `uniform` refuses the source until every one of
them has been settled by hand -- an inline is a judgement about a part that no
rule here can make. Undoing one is deleting the last line and commenting the
first out, and what is left is this script's own shape for a request to draw 客,
carrying the `-- no-inline` mark that makes a later run leave it alone rather
than inline it again. The mark is written rather than inferred because a block
a hand undid an inline in is character for character the block an ordinary run
writes for a line the box holds back, so nothing about the block itself could
tell the two apart; dropping the mark by hand is how a block already in the file
asks to be inlined after all. (A sequence's own nesting, `⿰⿰XYC`, has no such
line: its operand is not a character, so nothing names it. Those keep the
`<- ⿰⿰XYC` note instead, and being nobody's judgement they parse and build.)

Passing `--inline` twice writes the inlined line *outright* instead: no
un-inlined line above it, no mark, and no `<- ⿰⿰XYC` note on either kind of
inline, so that a line's own comment is the sequence it draws and nothing else.
Such a block parses and builds, and so has no review gate at all: it is the form
for a run that means to inline rather than to propose it.

The emitted line leaves its components *undecided* (no `:WxH` suffix), which is
the initial state `compose.rs` documents for a glyph populated from IDS; run
`cargo run -r -- fix -i font/ --optimize-clearance` afterwards to pick the
variants and the gaps.

A part whose shape differs by **region** has no plain name to write. The source
draws such a character as `han-XXXX-R` blocks, or as `han-XXXX.S` shapes with
`exists`-scoped aliases tying each region to one of them, and a line that uses
it names it `han-XXXX-($-1)` -- the region the block's own name is expanding
for. So a block with any such component is written as a *family*:

    glyph han-4f36-($han-regions):15x16 15 16 // 伶
    ⿰ han-4ebb han-4ee4-($-1) // ⿰亻令

and what such a component may be drawn at is the labels **every** region draws,
since the one name has to resolve in each of them (`Family.shared`). A
character only some regions draw is no part at all here, and the line asking
for it is commented out like any other whose parts are missing. `HAN_NAME_RE`
is the whole of the name grammar this reads; it is Unison's own convention and
nothing outside this script depends on it.

A line the parts cannot lay out yet is emitted commented out (`// glyph …`), so
that nothing is lost and nothing unbuildable is added either: its parts are drawn
at no size that tiles the box (`--ignore-box` writes it anyway).

A part that is itself a composite is *not* such a reason. A composite drawing is
a drawing: the line has something to place, and an IDC line is routinely the
thing a hand then `Inline once`s to adjust. What the hand-drawn/composite
distinction is for here is the *other* question -- whether a part is a box to be
reopened, which is `Inventory.pixel_drawn` and `candidates`, and is asked
separately.

An *inlined* line is the exception, and is held only to its own parts: whether
their smallest drawings add up to the box is not asked, since that total is not
a reason to hold the line back but the very thing the line was written to
measure -- run `--optimize-clearance` over it and a total that overruns the box
shows as ink out of the box in the editor and as a negative clearance in the
report, which says how much narrower a part has to be drawn. A part drawn at no
size that could sit in a slot at all still comments the line out, inlined or
not: that one is a request for a drawing, and there is nothing to look at yet.

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

# The IDCs `compose.rs` lays out, and their arity: the four one-dimensional
# splits and the nine enclosures.
SPLIT_ARITY = {"⿰": 2, "⿱": 2, "⿲": 3, "⿳": 3}
ENCLOSE_ARITY = {
    "⿴": 2, "⿵": 2, "⿶": 2, "⿷": 2, "⿸": 2, "⿹": 2, "⿺": 2, "⿼": 2, "⿽": 2,
}
IDC_ARITY = {**SPLIT_ARITY, **ENCLOSE_ARITY}
# Everything else that may appear in an IDS, with its arity, so that a sequence
# can still be *parsed* (and then rejected) rather than mis-read. `⿻` says two
# drawings share a box and nothing about where; `⿾`/`⿿` transform one drawing
# rather than composing two; `㇯` is a subtraction. None of the three is a
# layout, which is why `compose.rs` does not implement them.
OTHER_ARITY = {"⿻": 2, "㇯": 2, "⿾": 1, "⿿": 1}
ARITY = {**IDC_ARITY, **OTHER_ARITY}

# Of the splits, the two that divide the box left to right; the other two divide
# it top to bottom. An *enclosure* has no split axis at all and is asked a
# different question entirely -- see `feasible`.
HORIZONTAL = {"⿰", "⿲"}
ENCLOSING = set(ENCLOSE_ARITY)

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
    `src/pattern.rs`). A name with no group expands to itself, and one whose
    groups this cannot read (a `($1)` an `exists` binds, a `($-1)` back
    reference) expands to nothing -- it is not this function's to guess.
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


# A size token is `WxH`, or `WxH.NxM` for an enclosure's outer part, where
# `NxM` is the cavity the drawing promises to leave (`compose.rs`, D1).
SIZE_RE = re.compile(r"^(\d+)x(\d+)(?:\.(\d+)x(\d+))?$")


def parse_label(label: str) -> tuple[tuple[int, int] | None, tuple[int, int] | None, str | None]:
    """`5x16-l` -> ((5, 16), None, `l`); see `compose.rs` (D1).

    The second member is the cavity a `WxH.NxM` label promises, which is what
    marks a drawing as an enclosure's *outer* part -- the enclosure's answer to
    the `l`/`r` a split's name carries.
    """
    size = None
    cavity = None
    direction = None
    for tok in label.split("-"):
        m = SIZE_RE.match(tok)
        if m and size is None:
            size = (int(m.group(1)), int(m.group(2)))
            if m.group(3) is not None:
                cavity = (int(m.group(3)), int(m.group(4)))
        elif tok in ("l", "r", "u", "d", "c") and direction is None:
            direction = tok
    return size, cavity, direction


# --------------------------------------------------------------------------
# han glyph names
# --------------------------------------------------------------------------

# The one place that knows what a han glyph name may look like. Every form the
# source writes, in full:
#
#     han-XXXX                 the character, drawn one way for every region
#     han-XXXX.S               one *shape* of it, where the regions differ
#     han-XXXX-R               the drawing one region uses, as a name of its own
#     han-XXXX-(R1|R2|…)       several regions at once, as a written pattern
#     han-XXXX-($han-regions)  every region, as a written pattern
#     han-XXXX-($-1)           the region the enclosing block is expanding for
#
# each optionally followed by `:LABEL`, the `WxH[.NxM][-l]` variant spec
# `compose.rs` reads. A name never carries a shape *and* a region: the regions
# are tied to the shapes by `exists`-scoped aliases instead (`exists
# han-XXXX\.0:(…)` over `glyph han-XXXX-(j|k):($1) = ($0)`), which is how
# `load_inventory` reads them.
HAN_NAME_RE = re.compile(
    r"^han-(?P<cp>[0-9a-f]{4,5})"
    r"(?:\.(?P<shape>[0-9a-z]+)|-(?P<region>\([^()]*\)|[a-z]))?"
    r"(?::(?P<label>.*))?$"
)

# The region letters `$han-regions` names, for a source that states none.
DEFAULT_REGIONS = ["g", "h", "t", "j", "k", "p", "v"]

# What a component names when the region is the enclosing block's own, and the
# group the block's name has to carry for it to mean anything.
BACKREF = "($-1)"
REGION_GROUP = "($han-regions)"


@dataclass(frozen=True)
class HanName:
    """One han glyph name, taken apart."""

    cp: int
    shape: str | None
    # a single region letter, or the written group (`(g|h|t)`, `($-1)`) when
    # the name is a pattern rather than one glyph's
    region: str | None
    label: str | None

    @property
    def family(self) -> str:
        """The name with its label dropped: `han-4ee4-g:9x16` -> `han-4ee4-g`."""
        stem = f"han-{self.cp:04x}"
        if self.shape is not None:
            return f"{stem}.{self.shape}"
        if self.region is not None:
            return f"{stem}-{self.region}"
        return stem


def parse_han_name(name: str) -> HanName | None:
    """One han glyph name, or `None` for anything that is not one at all."""
    m = HAN_NAME_RE.match(name)
    if m is None:
        return None
    return HanName(
        int(m.group("cp"), 16), m.group("shape"), m.group("region"), m.group("label")
    )


# --------------------------------------------------------------------------
# the font source
# --------------------------------------------------------------------------

@dataclass
class Variant:
    """One drawing, under one of the names that reach it."""

    name: str
    label: str
    w: int
    h: int
    direction: str | None
    handdrawn: bool
    # The `NxM` an enclosure's outer part promises to leave clear; `None` for
    # every ordinary drawing, which is what `compose::enclosure_rank` reads to
    # tell the two slots' candidates apart.
    cavity: tuple[int, int] | None = None


@dataclass
class Family:
    """What one character offers a line that names it as a part.

    `shared` is what a *single* written component name can stand for. For a
    character drawn one way for everyone that is simply its own drawings; for
    one whose shape differs by region it is the labels **every** region draws,
    because a `han-XXXX-($-1)` component is one name that has to resolve in
    each of them -- a label only some regions draw is no use to such a line.

    `regional` is what decides how the component is written and, through it,
    whether the block that names it is a pattern block: a character with any
    region-suffixed name of its own has no plain name to write, so a line using
    it says `han-XXXX-($-1)` and its own header says `-($han-regions)`.
    """

    cp: int
    regional: bool
    shared: list[Variant]


@dataclass
class Inventory:
    regions: list[str] = field(default_factory=lambda: list(DEFAULT_REGIONS))
    # full name (`han-4ee4.0:9x16`) -> the drawing it is
    drawings: dict[str, Variant] = field(default_factory=dict)
    # full name -> the name it is a second name for (`alias.rs`)
    aliases: dict[str, str] = field(default_factory=dict)
    # what each character offers a line, once the two above are read together
    families: dict[int, Family] = field(default_factory=dict)
    # code points that already have a glyph of any kind
    covered: set[int] = field(default_factory=set)
    # code points that have a full-size (15x16) glyph
    covered_full: set[int] = field(default_factory=set)
    # code points some glyph block draws *in pixels* -- a body that is neither
    # an IDC line nor a `ref`. Such a character is a drawing to be placed, not a
    # box to be reopened, so nothing is inlined into it (see `candidates`).
    pixel_drawn: set[int] = field(default_factory=set)
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

    def resolve(self, name: str) -> Variant | None:
        """Follow a name through the aliases to the drawing it reaches."""
        seen = set()
        while name not in self.drawings:
            if name in seen:
                return None  # a cycle; `alias.rs` reports it, we just stop
            seen.add(name)
            target = self.aliases.get(name)
            if target is None:
                return None
            name = target
        return self.drawings[name]


NAME_PARTS_RE = re.compile(r"^name-parts\s+\$(\S+)\s*=\s*(.*?)(?://.*)?$")
EXISTS_RE = re.compile(r"^exists\s+(\S+)\s*(?://.*)?$")


def load_name_parts(font_dir: str) -> dict[str, list[str]]:
    parts: dict[str, list[str]] = {}
    for fname in sorted(os.listdir(font_dir)):
        if not fname.endswith(".unf"):
            continue
        with open(os.path.join(font_dir, fname), encoding="utf-8") as f:
            for line in f:
                m = NAME_PARTS_RE.match(line.strip())
                if m:
                    parts[m.group(1)] = m.group(2).split()
    return parts


def load_inventory(font_dir: str, parts: dict[str, list[str]]) -> Inventory:
    """Read every `.unf` in the directory into an [`Inventory`].

    Three kinds of `glyph` line matter and they are read in three passes,
    because each rests on the one before: a **drawing** (`glyph NAME W H` over a
    grid), a plain **alias** (`glyph A = B`), and an alias under an **`exists`**
    scope, which names a whole family at once and so has to wait until the
    names it searches are known.
    """
    inv = Inventory(regions=list(parts.get("han-regions", DEFAULT_REGIONS)))
    plain_aliases: list[tuple[str, str]] = []
    # (the `exists` pattern, the scoped `glyph A = B` line's two sides)
    scoped_aliases: list[tuple[str, str, str]] = []

    for fname in sorted(os.listdir(font_dir)):
        if not fname.endswith(".unf"):
            continue
        path = os.path.join(font_dir, fname)
        with open(path, encoding="utf-8") as f:
            lines = f.read().split("\n")
        # `exists` governs exactly one following item and does not stack
        # (`exists.rs`, "# Scope"), so the scope is cleared by the very next
        # line that is one.
        scope: str | None = None
        for i, line in enumerate(lines):
            commented = line.lstrip("/ ")
            if line.startswith("//") and commented.startswith("glyph "):
                toks = commented.split()
                cps = set()
                for expanded in expand_pattern(toks[1], parts) if len(toks) > 1 else []:
                    hn = parse_han_name(expanded)
                    if hn is not None:
                        cps.add(hn.cp)
                inv.declared.update(cps)
                if len(cps) == 1 and (i == 0 or not lines[i - 1].strip()):
                    end = i + 1
                    while end < len(lines) and lines[end].strip():
                        end += 1
                    inv.declared_blocks[next(iter(cps))] = (fname[:-4], lines[i:end])
                continue
            if not line.strip() or line.lstrip().startswith("//"):
                continue
            m = EXISTS_RE.match(line.strip())
            if m:
                scope = m.group(1)
                continue
            here, scope = scope, None
            if not line.startswith("glyph "):
                continue
            head = line.split("//")[0].strip()
            toks = head.split()[1:]
            if not toks:
                continue
            name = toks[0]
            if len(toks) > 2 and toks[1] == "=":
                # `glyph A = B`: a second *name* for a drawing, and the only way
                # the source says which shape a region uses.
                if here is not None:
                    scoped_aliases.append((here, name, toks[2]))
                else:
                    plain_aliases.append((name, toks[2]))
                continue
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
                hn = parse_han_name(expanded)
                if hn is None:
                    continue
                inv.covered.add(hn.cp)
                if handdrawn:
                    inv.pixel_drawn.add(hn.cp)
                if (w, h) == (BOX_W, BOX_H):
                    inv.covered_full.add(hn.cp)
                if hn.label is None:
                    continue  # a name with no variant spec is nothing to place
                size, cavity, direction = parse_label(hn.label)
                if size is not None and size != (w, h):
                    continue  # a lying name; `compose.rs` checks this itself
                inv.drawings.setdefault(
                    expanded,
                    Variant(expanded, hn.label, w, h, direction, handdrawn, cavity),
                )

    for lhs, rhs in plain_aliases:
        add_aliases(inv, lhs, rhs, parts)
    resolve_scoped_aliases(inv, scoped_aliases, parts)
    build_families(inv)
    return inv


def add_aliases(inv: Inventory, lhs: str, rhs: str, parts: dict[str, list[str]]) -> None:
    """`glyph A = B` for every name the two patterns declare, in lock-step."""
    left = expand_pattern(lhs, parts)
    right = expand_pattern(rhs, parts)
    if not left or not right:
        return
    for i, name in enumerate(left):
        target = right[min(i, len(right) - 1)]
        if name != target:
            inv.aliases.setdefault(name, target)
        inv.covered.update(
            hn.cp for hn in [parse_han_name(name)] if hn is not None
        )


# An `exists` pattern, in the one shape the han source writes: a name with the
# regex metacharacters escaped, a `:`, and one group over the variant label.
# `exists.rs` allows more than this; what more would mean here is a search this
# script cannot answer, and the alias under it is then simply not read.
def exists_matches(inv: Inventory, spec: str) -> list[tuple[str, str]]:
    """The `($0)`/`($1)` bindings one `exists` pattern finds, as (name, label).

    The names searched are the ones the source *declares* -- drawings and the
    aliases already resolved -- exactly as `exists.rs` describes, and never the
    on-demand ones.
    """
    base, sep, label_re = spec.partition(":")
    if not sep:
        return []
    base = base.replace("\\", "")
    try:
        compiled = re.compile(label_re)
    except re.error:
        return []
    out = []
    for name in sorted(set(inv.drawings) | set(inv.aliases)):
        hn = parse_han_name(name)
        if hn is None or hn.label is None or hn.family != base:
            continue
        if compiled.fullmatch(hn.label):
            out.append((name, hn.label))
    return out


# How many rounds of `exists` resolution to run: a search may find a name an
# earlier search declared, which `exists.rs` settles as a fixpoint with a cycle
# budget. The han source never nests them more than one deep; a couple of extra
# rounds cost nothing and stop the count from being a rule.
EXISTS_ROUNDS = 4


def resolve_scoped_aliases(
    inv: Inventory, scoped: list[tuple[str, str, str]], parts: dict[str, list[str]]
) -> None:
    """`exists BASE:(…)` over `glyph han-XXXX-(j|k):($1) = ($0)`.

    This is the whole of how a region is tied to a shape: the search binds `$0`
    to a drawing's full name and `$1` to its label, and the line under it
    declares that name again for each region it lists. Substituting the
    bindings *before* expanding the pattern is what keeps `($1)` from being
    read as an alternation.
    """
    for _ in range(EXISTS_ROUNDS):
        before = len(inv.aliases)
        for spec, lhs, rhs in scoped:
            for name, label in exists_matches(inv, spec):
                add_aliases(
                    inv,
                    lhs.replace("($1)", label).replace("($0)", name),
                    rhs.replace("($1)", label).replace("($0)", name),
                    parts,
                )
        if len(inv.aliases) == before:
            return


def build_families(inv: Inventory) -> None:
    """Fold the drawings and the aliases into one answer per character.

    A character with any region-suffixed name is *regional*: the source draws it
    per region and has no plain name for it, so what a line may name is
    `han-XXXX-($-1)` and what it may pick from is the labels every region
    draws. A character with none is written plainly and offers its own labels.
    """
    # family name (`han-4ee4-g`) -> {label: the full name that reaches it}
    by_family: dict[str, dict[str, str]] = defaultdict(dict)
    cps: set[int] = set()
    for name in sorted(set(inv.drawings) | set(inv.aliases)):
        hn = parse_han_name(name)
        if hn is None:
            continue
        cps.add(hn.cp)
        if hn.label is not None:
            by_family[hn.family].setdefault(hn.label, name)

    for cp in sorted(cps):
        stem = han_name(cp)
        per_region = {r: by_family.get(f"{stem}-{r}", {}) for r in inv.regions}
        regional = any(per_region.values())
        shared: list[Variant] = []
        if not regional:
            for label, name in sorted(by_family.get(stem, {}).items()):
                got = inv.resolve(name)
                if got is not None:
                    shared.append(got)
        elif all(per_region.values()):
            labels = set.intersection(*(set(m) for m in per_region.values()))
            for label in sorted(labels):
                drawn = [inv.resolve(per_region[r][label]) for r in inv.regions]
                if any(v is None for v in drawn):
                    continue
                first = drawn[0]
                # One name stands for all of them, so what it promises is what
                # every region keeps: hand-drawn only where each of them is.
                shared.append(
                    Variant(
                        f"{stem}-{BACKREF}:{label}",
                        label,
                        first.w,
                        first.h,
                        first.direction,
                        all(v.handdrawn for v in drawn),
                        first.cavity,
                    )
                )
        inv.families[cp] = Family(cp, regional, shared)


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
               splits: dict[int, tuple[str, list[int]]] | None,
               pixel_drawn: frozenset[int] | set[int] = frozenset()) -> list[Candidate]:
    """Every component list one IDS node can be written as, plainest first.

    The plain one is the node itself when every operand is a character; the
    others each inline one operand that splits along the same axis. The
    sequence's own nesting (`⿰⿰XYC`) is inlined whenever `inline` is set, since
    there the sequence itself says the box splits three ways. A *character*
    standing for such a split (`⿰BC` where `B` is itself `⿰XY`) is only
    inlined when `splits` is given, because that one is a judgement about the
    part rather than about this character: 客 decomposes into 宀 and 各, but a
    line that says so has given up on ever drawing 客.

    A character in `pixel_drawn` is never opened up, whatever its own IDS says:
    the source draws it, so the line has a part to place and this script has no
    business cutting that drawing in two for the halves to be sized apart.
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
            inner_cp = ord(normalize_component(kid.char))
            if inner_cp in pixel_drawn:
                continue  # drawn in pixels: a part to place, not a box to reopen
            got = splits.get(inner_cp)
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
                # `1`/`2` are the compatibility blocks, whose slice ids are not
                # radical-stroke and whose characters IDS.TXT gives no sequence
                # for at all. Nothing here can place a character in one, so the
                # slices this script knows are exactly the ones `block_of` can
                # name -- reading the rest would only be a `BLOCK_RANK` miss.
                if m and m.group(1) in BLOCK_RANK:
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


def feasible(inv: Inventory, op: str, comps: list[int], inlined: bool = False) -> Verdict:
    """Ask the parts about one line, in two independent questions.

    *Existence* decides whether the line is written at all. A declaration is
    what says which character is to be drawn, so it comes before the drawing: a
    line whose parts are all drawn at 15x16 and so cannot tile the box yet is
    exactly the line that asks for a narrow variant to be drawn, and dropping it
    would hide the request.

    *Fit* decides whether it is written commented out. What "fit" means is the
    operator's to say, and the two answers are `feasible_split` and
    `feasible_enclosure` below; both mirror what `compose.rs` will demand of the
    line, so that nothing is written uncommented that the build then refuses.

    Those are two questions, not one, and an `inlined` line is only asked the
    first — see `feasible_split`, which is the only place inlining arises.

    What a component *is* asked is [`Family.shared`], so a character whose
    shape differs by region is held to the labels every region draws: the line
    names it once and that one name has to resolve in each of them.
    """
    families = [inv.families.get(cp) for cp in comps]
    if any(f is None or not f.shared for f in families):
        return Verdict(None, False, "component not drawn")
    cands = [f.shared for f in families]
    kind = "handdrawn" if all(any(v.handdrawn for v in c) for c in cands) else "composite"
    fits = (
        feasible_enclosure(cands)
        if op in ENCLOSING
        else feasible_split(op, cands, inlined)
    )
    return Verdict(kind, fits, "")


def feasible_split(op: str, cands: list[list[Variant]], inlined: bool) -> bool:
    """Whether the parts drawn today tile the box along the split's axis.

    Mirrors `compose::fits_slot`: a variant can go in a slot when its extent
    across the split is the glyph's and its extent along the split is strictly
    shorter than the glyph's — a part as long as the glyph fills it on its own
    and leaves the rest of the line nowhere to stand — and the line fits when
    the smallest such variants still tile the box together. Which variant is
    actually written is `uniform fix --optimize-clearance`'s to choose.

    An `inlined` line is held only to the first half. A line written *as it
    stands* is held to the box because the box is all that says the parts are
    the wrong size: nothing else would ever ask for a narrower 項 than the one
    15x16 drawing. An inlined line has already answered that -- it is written
    because its own smaller parts are drawn, and what its total says is how much
    narrower they have to become, which is a thing to be *seen*:
    `--optimize-clearance` lays it out anyway, with the overflow showing as a
    negative clearance and as ink out of the box in the editor. Commenting it
    out instead hides exactly the measurement that would have been acted on.
    `--ignore-box` is the same relaxation for the un-inlined line, where it is a
    much blunter thing to ask for.
    """
    horizontal = op in HORIZONTAL
    axis, cross = (BOX_W, BOX_H) if horizontal else (BOX_H, BOX_W)
    total = 0
    for variants in cands:
        along = [
            (v.w if horizontal else v.h)
            for v in variants
            if (v.h if horizontal else v.w) == cross
            and (v.w if horizontal else v.h) < axis
        ]
        if not along:
            return False
        total += min(along)
    return inlined or total <= axis


def feasible_enclosure(cands: list[list[Variant]]) -> bool:
    """Whether some drawn outer part offers a cavity some drawn inner part fits.

    Mirrors `compose::fits_enclosure_slot` and the candidate lists
    `fix::clearance::Inventory::enclosure_candidates` builds from it, so that a
    line written uncommented is one the fixer can actually place:

    * the **outer** part is the glyph exactly and promises a cavity. The
      promise is what marks a drawing as one made to enclose, and a `匚` drawn
      only as an ordinary 15x16 glyph is a request to draw the enclosing variant
      rather than a part this line can use;
    * the **inner** part promises none and fits inside that cavity.

    The cavity is a lower bound the drawing keeps (`compose::cavity_fits`), so
    comparing boxes to it is the same arithmetic the splits do against the axis.
    """
    outer, inner = cands
    cavities = [
        v.cavity for v in outer if (v.w, v.h) == (BOX_W, BOX_H) and v.cavity is not None
    ]
    held = [(v.w, v.h) for v in inner if v.cavity is None]
    return any(w <= n and h <= m for n, m in cavities for w, h in held)


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
    "not an IDC this source lays out",
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


@dataclass(frozen=True)
class Line:
    """One IDC line, ready to be written: what it composes and how it names it.

    A component whose character is drawn per region is written `han-XXXX-($-1)`
    -- the region the block's own name is expanding for -- and a block with any
    such component is a *pattern* block, its header naming `-($han-regions)`.
    `patterned` is kept beside the flags rather than derived from them because
    an inlined block's header answers to two lines (the one it drew and the
    un-inlined one it kept above it), and because it is what
    `script_block_idc` reads back out of a block it is about to regenerate.
    """

    op: str
    comps: tuple[int, ...]
    # per component: write it as `han-XXXX-($-1)` rather than `han-XXXX`
    regional: tuple[bool, ...]
    patterned: bool

    def part_names(self) -> list[str]:
        return [
            f"{han_name(cp)}-{BACKREF}" if r else han_name(cp)
            for cp, r in zip(self.comps, self.regional)
        ]


def make_line(inv: Inventory, op: str, comps: list[int], also: list[int] = ()) -> Line:
    """The line those components spell, asking the inventory how to name each.

    `also` is a second component list the same block will carry (an inlined
    block's un-inlined line): a region in either of them is one the header has
    to bind, since a `($-1)` means nothing under a header that has no group.
    """
    regional = tuple(regional_flags(inv, comps))
    patterned = any(regional) or any(regional_flags(inv, list(also)))
    return Line(op, tuple(comps), regional, patterned)


def regional_flags(inv: Inventory, comps: list[int]) -> list[bool]:
    return [cp in inv.families and inv.families[cp].regional for cp in comps]


def glyph_head(cp: int, char: str, patterned: bool) -> str:
    """The block's own header: a family's name where any component names one."""
    name = han_name(cp) + (f"-{REGION_GROUP}" if patterned else "")
    return f"glyph {name}:{BOX_W}x{BOX_H} {BOX_W} {BOX_H} // {char}"


def idc_line(line: Line) -> str:
    """One IDC line: the component names, and the sequence they spell."""
    names = " ".join(line.part_names())
    return f"{line.op} {names} // " + line.op + "".join(chr(c) for c in line.comps)


# What an un-inlined line carries so that a hand can leave it un-inlined, and
# the only thing that stops a later run from inlining that character again. It
# has to be written rather than inferred: a block a hand undid an inline in is
# *character for character* the block an ordinary run writes for a line the box
# holds back, so nothing about the block itself can tell the two apart.
NO_INLINE_MARK = "-- no-inline"


def build_blocks(line: Line, cp: int, char: str, commented: bool,
                 origin: str | None = None,
                 alt: Line | None = None,
                 no_inline: bool = False) -> list[str]:
    head = glyph_head(cp, char, line.patterned)
    mark = f" {NO_INLINE_MARK}" if no_inline else ""
    if commented:
        # an inlined line keeps the sequence it came from beside the one it
        # draws, since the two are the same split but not the same text
        body = idc_line(line) + (f" <- {origin}" if origin else "") + mark
        return [f"// {head}", f"// {body}"]
    if alt is not None:
        # An inline that gave up on a part *nameable* keeps that un-inlined line
        # above the one it drew, commented out, and marked. Undoing the inline
        # by hand is then the whole of deleting the last line and commenting the
        # first out: what is left is a request for the part, in the shape this
        # script writes one -- which `is_script_block` recognizes -- and the
        # mark it kept is what makes the revive below decline to inline it
        # again. Dropping the mark by hand asks for the opposite.
        #
        # The block this writes does not parse, on purpose: a comment between a
        # `glyph` header and its body ends the block, so the drawn line is read
        # as a directive of its own and `uniform` refuses the file
        # (`document_io.rs`). That is the review gate -- an inline is a
        # judgement about a part that no rule here can make, so every block this
        # form writes has to be looked at and settled by hand, one of the two
        # ways above, before the source builds again.
        return [head, f"// {idc_line(alt)} {NO_INLINE_MARK}", idc_line(line)]
    return [head, idc_line(line) + (f" <- {origin}" if origin else "") + mark]


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
    line, origin, no_inline = got
    return block == build_blocks(line, cp, char, True, origin, no_inline=no_inline)


def script_block_idc(block: list[str]) -> tuple[Line, str | None, bool] | None:
    """The `(line, origin, no_inline)` a commented-out block states.

    Read back out of the block's own text rather than out of today's inventory,
    including whether the header names a family: `is_script_block` regenerates
    the block from this and compares, and an inventory that has moved on since
    would otherwise make every earlier block look like a hand edit.
    """
    if len(block) != 2:
        return None
    head_toks = block[0].lstrip("/ ").split()
    if len(head_toks) < 2 or head_toks[0] != "glyph":
        return None
    header = parse_han_name(head_toks[1])
    if header is None:
        return None
    patterned = header.region == REGION_GROUP
    body = block[1].lstrip("/ ")
    head, _, rest = body.partition("//")
    no_inline = False
    if "--" in rest:
        rest, note = rest.split("--", 1)
        # Anything else is a hand's own note, and leaving it in `rest` is what
        # makes `is_script_block` say so: the block will not regenerate.
        no_inline = note.strip() == NO_INLINE_MARK[3:]
        if not no_inline:
            rest = rest + "--" + note
    toks = head.split()
    if not toks or toks[0] not in IDC_ARITY:
        return None
    comps: list[int] = []
    regional: list[bool] = []
    for tok in toks[1:]:
        hn = parse_han_name(tok)
        if hn is None or hn.label is not None or hn.shape is not None:
            return None
        if hn.region is not None and hn.region != BACKREF:
            return None
        comps.append(hn.cp)
        regional.append(hn.region == BACKREF)
    origin = rest.split("<-", 1)[1].strip() if "<-" in rest else None
    return Line(toks[0], tuple(comps), tuple(regional), patterned), origin, no_inline


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-i", "--font-dir", default="font")
    ap.add_argument("--ids", default=IDS_PATH)
    ap.add_argument("--unihan", default=UNIHAN_PATH)
    ap.add_argument("--dry-run", action="store_true", help="report only; write nothing")
    ap.add_argument("--limit", type=int, default=0, help="stop after N new glyphs")
    ap.add_argument("--allow-ivi", action="store_true",
                    help="also use sequences marked 〾 (a component differs in some minor way)")
    ap.add_argument("--no-inline", action="store_true",
                    help="do not rewrite a same-axis nested operand into ⿲/⿳")
    ap.add_argument("--inline", action="count", default=0,
                    help="also inline an operand named by a character whose own IDS "
                         "splits the same way (⿰BC with B = ⿰XY -> ⿲XYC), never one "
                         "the source draws in pixels; the un-inlined line is kept "
                         "above it, commented out and marked `-- no-inline`, which is "
                         "what keeps a later run off it. Twice (`--inline --inline`) "
                         "writes the inlined line outright instead: no un-inlined "
                         "line, no mark, and no `<- SEQ` note on any inline")
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
    # `--inline` once inlines a *character*'s own split behind the review gate
    # `build_blocks` writes; twice drops the gate and writes the inlined line as
    # the block's whole body, the sequence it draws being all its comment says.
    splits = build_split_index(ids, args.allow_ivi) if inline and args.inline else None
    outright = args.inline >= 2
    rad_chars = {**prime_chars, **load_radical_chars(args.font_dir)}

    regional = sum(1 for f in inv.families.values() if f.regional)
    print(f"{len(inv.families)} part families ({regional} drawn per region), "
          f"{len(inv.covered)} characters drawn, "
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
                why = min(why, f"not an IDC this source lays out ({tree.op})",
                          key=reason_rank)
                continue
            cands = candidates(tree, inline, splits, inv.pixel_drawn)
            if not cands:
                why = min(why, "a nested component nothing names", key=reason_rank)
                continue
            for cand in cands:
                if cp in cand.comps:
                    why = min(why, "self-referential IDS", key=reason_rank)
                    continue
                verdict = feasible(inv, cand.op, cand.comps, cand.inlined)
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
        if not verdict.fits and not args.ignore_box:
            # What the parts fell short of is the operator's to say: a split's
            # parts have to tile the box, an enclosure's have to be a cavity and
            # something that goes in it.
            holds.append(
                "no drawn cavity holds the inner part"
                if op in ENCLOSING
                else "parts do not fit the box"
            )
        # An `--inline --inline` run writes the inlined line and nothing else:
        # no un-inlined line above it and no `<- SEQ` note, so neither has a
        # component to name or a region to bind.
        if outright:
            origin, cand_alt = None, None
        else:
            cand_alt = cand.alt
        # How each component is named, and so whether this block is a family:
        # the un-inlined line the block may keep beside the drawn one is asked
        # too, since its `($-1)` needs the same header group.
        line = make_line(inv, op, comps, list(cand_alt[1]) if cand_alt else [])
        alt = (
            Line(cand_alt[0], tuple(cand_alt[1]),
                 tuple(regional_flags(inv, list(cand_alt[1]))), line.patterned)
            if cand_alt is not None
            else None
        )
        block = build_blocks(line, cp, entry.char, bool(holds), origin, alt)
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
            if cand.inlined and got is not None and got[2]:
                # the block is the un-inlined line, marked: a hand undid the
                # inline (or declined one), and writing the inlined line back is
                # exactly what that asked not to happen
                stats["declared, marked `-- no-inline`"] += 1
                continue
            removals[old[0]].add(cp)
            stats["revived (was commented out)"] += 1
        plan[sl.name].append((key, cp, block))
        stats["composite parts" if verdict.kind == "composite" else "hand-drawn parts"] += 1
        if cand.inlined:
            stats["inlined a nested operand"] += 1
        for hold in holds:
            stats["commented out: " + hold] += 1
        made += 1
        if args.verbose:
            note = ", ".join([verdict.kind] + (["revived"] if revive else [])
                             + (["inlined"] if cand.inlined else []) + holds)
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
