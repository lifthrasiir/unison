#!/usr/bin/env python3
"""Undo `gen_ids_composites.py`'s inlining of one *character* in `font/`.

That script folds an operand that splits along the same axis as the whole into
the ternary IDC -- `⿰X比` written as `⿲X匕匕`, `⿰X化` as `⿲X亻匕` -- and with
`--inline` it does so for an operand named by a character, giving up on drawing
that character. For a part that keeps turning up the trade goes the other way:
the character is worth drawing once, at every width its callers need, so that
the callers name it and its proportions are decided in one place rather than
per call site. This script is that reverse pass.

For each character in `RULES` it

  * finds every line that inlines it -- an `⿲`/`⿳` IDC line, or a run of `ref`
    lines, whose two adjacent components are the character's own parts, in a
    block whose character IDS-decomposes with the character as an operand (that
    last check is what keeps 傾 `⿰亻頃`, drawn `⿲亻匕頁`, from being read as a
    化 inline);
  * works out the width each call site leaves the character, and the internal
    split the rule holds it to at that width;
  * writes the character's glyph blocks for every width its callers need, in
    the block that already defines it; and
  * rewrites each call site to name the character, comment included.

The **span is preserved**: a call site's other parts and gaps are left exactly
where they are, so only the inlined pair's own pixels move. Where the rule
cannot split the span at all -- the parts it asks for are drawn at no such size
-- the span is widened by one and the cell is taken from a gap next to it,
which is why a rule that forces a gap can still be met by a call site that
wrote none. A call site that offers neither is reported and left alone.

A character drawn per region (化) is named `han-XXXX-($-1)` and its callers
become `-($han-regions)` families themselves; the inlining had quietly lost
that variation by naming `匕`'s default shape `han-5315.0` directly. A caller
that named one region's part outright (`han-5315-g`, in a glyph only that
region has) keeps naming that region: `han-5316-g`.

Commented-out blocks -- the requests an earlier run wrote for a character whose
parts do not fit -- are un-inlined too, and take `gen_ids_composites.py`'s own
`-- no-inline` mark, which is what stops a later run of it from inlining the
character there again.

The script is idempotent: a source it has already rewritten has no inlined line
left to find, and the glyph blocks it writes are the ones it would write again.

A rule whose `split` is `None` is a **probe**: nothing is rewritten, and the
call sites are surveyed instead -- every width the character is inlined at, and
how the source divides it there (`9x16: 4-1-4 x3`) -- which is what one reads
to write the rule's `split` in the first place.

Usage:
    python3 scripts/uninline_ids_char.py [-i font] [--dry-run] [CHAR...]
"""

from __future__ import annotations

import argparse
import collections
import os
import re
import sys
from dataclasses import dataclass
from typing import Callable

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import gen_ids_composites as G


# --------------------------------------------------------------------------
# the rules
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class Rule:
    """One character to draw rather than inline, and how wide its parts sit.

    `split` answers the only question the call sites do not: given the width
    the site leaves the character, how that width divides between the two parts
    and the gap between them. It is a rule and not a survey on purpose -- the
    call sites disagree, which is the whole reason for drawing the character
    once -- but it is written to reproduce the sizes already in the source, so
    that adopting it moves no glyph that was already right.

    `split` is `None` while the rule is still being written: the character's
    sites are then surveyed and reported, and nothing is rewritten.
    """

    char: str
    # the IDC the character itself is, and the two characters it composes
    op: str
    parts: tuple[str, str]
    # the character is drawn per region (`han-XXXX-($han-regions)` blocks)
    regional: bool
    # width -> (first part, gap, second part), or None where no split fits;
    # the rule itself is None while it is only being probed
    split: Callable[[int], tuple[int, int, int] | None] | None
    why: str


def 比(w: int) -> tuple[int, int, int]:
    """Two equal parts, the odd cell spent on the gap between them."""
    return w // 2, w % 2, w // 2


def 化(w: int) -> tuple[int, int, int] | None:
    """亻 and 匕 with one cell between them, always.

    Without the gap the two collide at the top, where 亻's fall and 匕's head
    reach for the same cell. The 亻 width follows the sizes the source already
    draws 化 at: 3 up to 12 wide, 5 at 15.
    """
    left = 3 if w <= 12 else 5
    right = w - 1 - left
    return (left, 1, right) if right > 0 else None


def 此(w: int) -> tuple[int, int, int] | None:
    if 10 <= w:
        return 6, 0, w - 6


RULES = {
    "比": Rule(
        char="比",
        op="⿰",
        parts=("匕", "匕"),
        regional=False,
        split=比,
        why="a pair of one part: the two halves are the same width at every size",
    ),
    "化": Rule(
        char="化",
        op="⿰",
        parts=("亻", "匕"),
        regional=True,
        split=化,
        why="亻 and 匕 always one cell apart",
    ),
    "此": Rule(
        char="此",
        op="⿰",
        parts=("止", "匕"),
        regional=True,
        split=此,
        why="止 always takes 6 cells, 匕 takes the rest",
    )
}


# --------------------------------------------------------------------------
# the source
# --------------------------------------------------------------------------

NUM_RE = re.compile(r"^-?\d+$")
SIZE_RE = re.compile(r"^(\d+)x(\d+)$")
GLYPH_RE = re.compile(r"^glyph\s+(\S+)")
REF_RE = re.compile(r"^ref\s+(\S+)\s+(-?\d+)\s+(-?\d+)\s*(?://\s*(.*))?$")
IDC_OPS = {"⿲": "⿰", "⿳": "⿱"}


def is_comment(line: str) -> bool:
    return line.lstrip().startswith("//")


def uncomment(line: str) -> str:
    """A line's own text, whether or not the line is commented out."""
    s = line.strip()
    return s[2:].strip() if s.startswith("//") else s


def split_comment(body: str) -> tuple[str, str | None]:
    head, sep, rest = body.partition("//")
    return head.strip(), rest.strip() if sep else None


def name_size(tok: str) -> tuple[str, tuple[int, int] | None, str]:
    """A component token as `(family, size, trailing)`.

    `han-961d:4x16-l` is the family `han-961d`, the size `(4, 16)` and the
    trailing variant word `-l`, which is the part's own and is carried through
    untouched.
    """
    stem, _, label = tok.partition(":")
    if not label:
        return stem, None, ""
    m = SIZE_RE.match(label)
    if m:
        return stem, (int(m.group(1)), int(m.group(2))), ""
    m = re.match(r"^(\d+)x(\d+)(.*)$", label)
    if m:
        return stem, (int(m.group(1)), int(m.group(2))), m.group(3)
    return stem, None, ":" + label


def token_cp(tok: str) -> int | None:
    hn = G.parse_han_name(tok)
    return hn.cp if hn else None


@dataclass
class Site:
    """One place the character is inlined, and what replacing it comes to."""

    path: str
    line: int  # 0-based index into the file's lines
    kind: str  # "idc" or "ref"
    cp: int  # the character whose block this is
    char: str
    header: int  # 0-based index of the block header this body belongs to
    commented: bool
    seq: str  # the IDS this block decomposes by, with the character in it
    size: tuple[int, int] | None  # the span the inlined pair fills, if sized
    split: tuple[int, int, int] | None  # how that span divides, if sized
    naming: str  # how the second part was named: "plain", "backref" or a region


def load_sizes(files: dict[str, list[str]]) -> dict[str, set[tuple[int, int]]]:
    """family name -> the sizes the source draws it at, aliases included."""
    out: dict[str, set[tuple[int, int]]] = collections.defaultdict(set)
    for lines in files.values():
        for line in lines:
            if is_comment(line):
                continue
            m = GLYPH_RE.match(line.strip())
            if not m:
                continue
            stem, size, _ = name_size(m.group(1))
            if size:
                out[stem].add(size)
    return out


def load_files(font_dir: str) -> dict[str, list[str]]:
    out = {}
    for name in sorted(os.listdir(font_dir)):
        if name.startswith(".") or not name.endswith(".unf"):
            continue
        path = os.path.join(font_dir, name)
        with open(path, encoding="utf-8") as f:
            out[path] = f.read().split("\n")
    return out


def ids_operand_seq(ids: dict, cp: int, char: str) -> str | None:
    """The character's best IDS that has `char` as a top-level operand.

    A block's own decomposition is what tells an inlined character from a
    coincidence: 傾 is `⿰亻頃` and 𩑭 is `⿰化頁`, and the two are written the
    same three parts in the same order.
    """
    entry = ids.get(cp)
    if entry is None:
        return None
    for seq, tags in sorted(entry.seqs, key=lambda s: -G.tag_score(s[1])):
        tree, used = G.parse_ids(seq)
        if tree is None or used != len(seq) or tree.op not in G.SPLIT_ARITY:
            continue
        if any(kid.char is None for kid in tree.kids):
            continue
        if char in [G.normalize_component(kid.char) for kid in tree.kids]:
            return seq
    return None


# --------------------------------------------------------------------------
# finding the call sites
# --------------------------------------------------------------------------


def scan(files: dict[str, list[str]], rule: Rule, ids: dict) -> list[Site]:
    part_cps = tuple(ord(c) for c in rule.parts)
    sites: list[Site] = []
    for path, lines in sorted(files.items()):
        header = None
        taken = -1  # the second line of a `ref` pair already claimed above
        for i, line in enumerate(lines):
            if not line.strip():
                header = None
                continue
            if i == taken:
                continue
            body = uncomment(line)
            if body.startswith("glyph "):
                m = GLYPH_RE.match(body)
                stem, _, _ = name_size(m.group(1))
                cp = token_cp(stem)
                header = (i, cp) if cp is not None else None
                continue
            if header is None or header[1] is None:
                continue
            cp = header[1]
            seq = ids_operand_seq(ids, cp, rule.char)
            if seq is None:
                continue
            site = match_idc(path, lines, i, header, rule, part_cps, cp, seq)
            if site is None:
                site = match_refs(path, lines, i, header, rule, part_cps, cp, seq)
            if site is not None:
                sites.append(site)
                if site.kind == "ref":
                    taken = i + 1
    return sites


def parse_idc(body: str) -> tuple[str, list[tuple[str, int]], int] | None:
    """`⿲ X 1 Y Z` as its operator, its `(token, gap before it)` list and the
    gap after the last one. An `assume`d line reads the same; `idc_prefix` is
    what keeps the keyword through a rewrite."""
    head, _ = split_comment(body)
    idc = G.idc_tokens(head.split())
    if idc is None:
        return None
    toks, _ = idc
    items: list[tuple[str, int]] = []
    gap = 0
    for tok in toks[1:]:
        if NUM_RE.match(tok):
            gap += int(tok)
        else:
            items.append((tok, gap))
            gap = 0
    return toks[0], items, gap


def idc_prefix(body: str) -> str:
    """The keyword an IDC line is written behind, with its space, or `""`."""
    idc = G.idc_tokens(split_comment(body)[0].split())
    return "" if idc is None else idc[1]


def naming_of(tok: str) -> str:
    """How a part was named: the region it picked, or that it picked none."""
    hn = G.parse_han_name(tok)
    if hn is None or hn.region is None:
        return "plain"
    return "backref" if hn.region == G.BACKREF else hn.region


def match_idc(path, lines, i, header, rule, part_cps, cp, seq) -> Site | None:
    line = lines[i]
    parsed = parse_idc(uncomment(line))
    if parsed is None:
        return None
    op, items, _ = parsed
    if IDC_OPS.get(op) != rule.op or len(items) != G.IDC_ARITY[op]:
        return None
    for j in range(len(items) - 1):
        if (token_cp(items[j][0]), token_cp(items[j + 1][0])) != part_cps:
            continue
        (t1, _), (t2, gap) = items[j], items[j + 1]
        _, s1, _ = name_size(t1)
        _, s2, _ = name_size(t2)
        size = split = None
        if s1 and s2 and s1[1] == s2[1]:
            size = (s1[0] + gap + s2[0], s1[1])
            split = (s1[0], gap, s2[0])
        elif s1 or s2:
            return None  # half-sized: not a shape this script knows
        return Site(path, i, "idc", cp, chr(cp), header[0], is_comment(line),
                    seq, size, split, naming_of(t2))
    return None


def ref_at(lines, i) -> tuple[str, int, int, str | None] | None:
    body = uncomment(lines[i])
    m = REF_RE.match(body)
    if not m:
        return None
    return m.group(1), int(m.group(2)), int(m.group(3)), m.group(4)


def match_refs(path, lines, i, header, rule, part_cps, cp, seq) -> Site | None:
    """Two `ref` lines that place the parts where the character would sit."""
    if i + 1 >= len(lines):
        return None
    first, second = ref_at(lines, i), ref_at(lines, i + 1)
    if first is None or second is None:
        return None
    if (token_cp(first[0]), token_cp(second[0])) != part_cps:
        return None
    _, s1, _ = name_size(first[0])
    _, s2, _ = name_size(second[0])
    if not s1 or not s2 or s1[1] != s2[1] or first[2] != second[2]:
        return None
    gap = second[1] - (first[1] + s1[0])
    if gap < 0:
        return None
    return Site(path, i, "ref", cp, chr(cp), header[0], is_comment(lines[i]),
                seq, (s1[0] + gap + s2[0], s1[1]), (s1[0], gap, s2[0]),
                naming_of(second[0]))


# --------------------------------------------------------------------------
# what each site becomes
# --------------------------------------------------------------------------


@dataclass
class Plan:
    site: Site
    size: tuple[int, int] | None  # the size the character is named at
    borrow: int  # cells taken from the gap after the pair (0 or 1)


def plan_site(site: Site, rule: Rule, sizes: dict, lines: list[str]) -> Plan | str:
    """The size the site names the character at, or why it cannot."""
    if site.size is None:
        return Plan(site, None, 0)  # an undecided line: names alone to rewrite
    w, h = site.size
    for extra in (0, 1):
        parts = rule.split(w + extra)
        if parts is None:
            continue
        a, _, b = parts
        if (a, h) not in part_sizes(rule, 0, sizes):
            continue
        if (b, h) not in part_sizes(rule, 1, sizes):
            continue
        if extra and not can_borrow(site, rule, lines):
            continue
        return Plan(site, (w + extra, h), extra)
    return f"no split of {w}x{h} the source draws both parts at"


def merge_pair(body: str, rule: Rule, tok: str):
    """The IDC line with the pair merged into one token named `tok`.

    Returns `(op, tokens, j, gaps)`: the operator the line had, the components
    it is left with, where the character sits among them, and the gaps -- one
    before each component and one after the last, which is where a bearing at
    either end of the box lives. The gap the pair had between them is gone,
    since it is inside the character now; that is why the borrow below asks
    this rather than the line's own tokens.
    """
    parsed = parse_idc(body)
    if parsed is None:
        return None
    op, items, trail = parsed
    part_cps = tuple(ord(c) for c in rule.parts)
    j = next((j for j in range(len(items) - 1)
              if (token_cp(items[j][0]), token_cp(items[j + 1][0])) == part_cps), None)
    if j is None:
        return None
    merged = [*items[:j], (tok, items[j][1]), *items[j + 1 + 1:]]
    return op, [t for t, _ in merged], j, [g for _, g in merged] + [trail]


def can_borrow(site: Site, rule: Rule, lines: list[str]) -> bool:
    """Whether a cell can come from a gap beside the pair on an IDC line."""
    if site.kind != "idc":
        return False
    merged = merge_pair(uncomment(lines[site.line]), rule, "?")
    return merged is not None and any(g > 0 for g in merged[3])


def part_sizes(rule: Rule, which: int, sizes: dict) -> set[tuple[int, int]]:
    """Every size the source draws the part at, under any of its names."""
    stem = G.han_name(ord(rule.parts[which]))
    out: set[tuple[int, int]] = set()
    for name, got in sizes.items():
        hn = G.parse_han_name(name)
        if hn is not None and G.han_name(hn.cp) == stem:
            out |= got
    return out


def part_name(rule: Rule, which: int, sizes: dict) -> str:
    """How a body line names the part: `han-4ebb`, `han-5315.0`, `-($-1)`.

    A part drawn per region is named `-($-1)` inside a family block, and only
    there; one drawn under a shape label alone (`han-5315.0`, the name a region
    alias points at) is named by that label, since the bare name draws nothing.
    """
    stem = G.han_name(ord(rule.parts[which]))
    cp = ord(rule.parts[which])
    if rule.regional and cp in REGIONAL_PARTS:
        return f"{stem}-{G.BACKREF}"
    if sizes.get(stem):
        return stem
    for name in sorted(sizes):
        hn = G.parse_han_name(name)
        if hn is not None and G.han_name(hn.cp) == stem and hn.shape is not None:
            return name
    return stem


def target_name(rule: Rule, site: Site) -> str:
    stem = G.han_name(ord(rule.char))
    if not rule.regional:
        return stem
    if site.naming in ("plain", "backref"):
        return f"{stem}-{G.BACKREF}"
    return f"{stem}-{site.naming}"


# --------------------------------------------------------------------------
# rewriting
# --------------------------------------------------------------------------


def rewrite_idc(line: str, rule: Rule, plan: Plan) -> str:
    """The IDC line with the pair replaced by the character it spells."""
    site = plan.site
    name = target_name(rule, site)
    tok = name if plan.size is None else f"{name}:{plan.size[0]}x{plan.size[1]}"
    op, merged, j, gaps = merge_pair(uncomment(line), rule, tok)
    if plan.borrow:
        # the cell the split needed and the span did not have: take it from the
        # first gap after the character, else from the last one before it
        after = [k for k in range(j + 1, len(gaps)) if gaps[k] > 0]
        before = [k for k in range(j, -1, -1) if gaps[k] > 0]
        gaps[(after + before)[0]] -= 1
    out = [IDC_OPS[op]]
    for k, name in enumerate(merged):
        if gaps[k]:
            out.append(str(gaps[k]))
        out.append(name)
    if gaps[-1]:
        out.append(str(gaps[-1]))
    comps = "".join(chr(token_cp(t)) for t in merged)
    text = idc_prefix(uncomment(line)) + " ".join(out) + f" // {IDC_OPS[op]}{comps}"
    if site.commented:
        text += f" {G.NO_INLINE_MARK}"
        return "// " + text
    return text


def block_bounds(lines: list[str], i: int) -> tuple[int, int]:
    """The blank-line-delimited block `i` sits in, as a half-open range."""
    lo = i
    while lo > 0 and lines[lo - 1].strip():
        lo -= 1
    hi = i
    while hi < len(lines) and lines[hi].strip():
        hi += 1
    return lo, hi


def ref_ids_comment(lines: list[str], lo: int, hi: int, seq: str) -> str | None:
    """The sequence a `ref` block's remaining lines spell, from its own IDS."""
    tree, used = G.parse_ids(seq)
    if tree is None or used != len(seq):
        return None
    cps = []
    for k in range(lo, hi):
        got = ref_at(lines, k)
        if got is None:
            continue
        cp = token_cp(got[0])
        if cp is None:
            return None
        cps.append(cp)
    if len(cps) != len(tree.kids):
        return None
    return tree.op + "".join(chr(c) for c in cps)


def promote_header(line: str) -> str | None:
    """A block header as the `-($han-regions)` family of itself, or None."""
    body = uncomment(line)
    m = GLYPH_RE.match(body)
    if not m:
        return None
    stem, size, trail = name_size(m.group(1))
    hn = G.parse_han_name(stem)
    if hn is None or hn.region is not None or hn.shape is not None:
        return None
    name = f"{stem}-{G.REGION_GROUP}"
    label = "" if size is None else f":{size[0]}x{size[1]}{trail}"
    rest = body[m.end():]
    text = f"glyph {name}{label}{rest}"
    return ("// " + text) if is_comment(line) else text


# --------------------------------------------------------------------------
# the character's own blocks
# --------------------------------------------------------------------------


def target_blocks(rule: Rule, want: set[tuple[int, int]], sizes: dict) -> list[str]:
    """The character's glyph blocks, one pair of lines per size."""
    stem = G.han_name(ord(rule.char))
    head = f"{stem}-{G.REGION_GROUP}" if rule.regional else stem
    p0, p1 = (part_name(rule, k, sizes) for k in (0, 1))
    out = []
    for w, h in sorted(want):
        a, gap, b = rule.split(w)
        out.append(f"glyph {head}:{w}x{h} {w} {h} // {rule.char}")
        mid = f" {gap} " if gap else " "
        out.append(f"{rule.op} {p0}:{a}x{h}{mid}{p1}:{b}x{h}"
                   f" // {rule.op}{rule.parts[0]}{rule.parts[1]}")
    return out


# Parts whose drawing differs by region, and so are named `-($-1)` inside a
# family block. Read from the source rather than listed: a part with region
# aliases has them.
REGIONAL_PARTS: set[int] = set()


def load_regional_parts(files: dict[str, list[str]]) -> set[int]:
    out = set()
    for lines in files.values():
        for line in lines:
            if is_comment(line):
                continue
            m = re.match(r"^glyph\s+han-([0-9a-f]{4,5})-\(([^)]*)\)", line.strip())
            if m:
                out.add(int(m.group(1), 16))
    return out


def existing_target(files: dict[str, list[str]], rule: Rule) -> tuple[str, int, int, set]:
    """Where the character is defined now: `(path, lo, hi, sizes)`.

    The blocks have to be one contiguous run for this script to write the run
    back; a source that has split them says something this script does not know
    and is left alone.
    """
    stem = G.han_name(ord(rule.char))
    found: list[tuple[str, int, tuple[int, int]]] = []
    for path, lines in sorted(files.items()):
        for i, line in enumerate(lines):
            if is_comment(line):
                continue
            m = GLYPH_RE.match(line.strip())
            if not m:
                continue
            name, size, _ = name_size(m.group(1))
            hn = G.parse_han_name(name)
            if hn is None or G.han_name(hn.cp) != stem or size is None:
                continue
            if hn.shape is not None:
                raise SystemExit(f"{path}:{i + 1}: {rule.char} has shape variants; "
                                 "this script writes one block per size")
            found.append((path, i, size))
    if not found:
        raise SystemExit(f"{rule.char} is drawn nowhere; nothing to un-inline into")
    paths = {p for p, _, _ in found}
    if len(paths) != 1:
        raise SystemExit(f"{rule.char} is defined in {sorted(paths)}; expected one file")
    path = found[0][0]
    rows = sorted(i for _, i, _ in found)
    if rows != list(range(rows[0], rows[0] + 2 * len(rows), 2)):
        raise SystemExit(f"{path}: {rule.char}'s blocks are not one contiguous run")
    return path, rows[0], rows[-1] + 2, {s for _, _, s in found}


# --------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------


def probe_rule(files: dict[str, list[str]], rule: Rule, ids: dict,
               sizes: dict, report: list[str]) -> None:
    """Survey a rule with no `split`: what the call sites do today.

    The point is to write the rule's `split` from what the source already says,
    so the report is grouped the way the rule is written -- by the width the
    site leaves the character, then by the division it chose there, commonest
    first, with one call site named per division to go and look at.
    """
    sites = scan(files, rule, ids)
    if not sites:
        report.append("  nothing inlined")
        return
    by_size: dict[tuple[int, int], collections.Counter] = collections.defaultdict(
        collections.Counter)
    where: dict[tuple[tuple[int, int], tuple[int, int, int]], str] = {}
    unsized: list[Site] = []
    for site in sites:
        if site.size is None or site.split is None:
            unsized.append(site)
            continue
        by_size[site.size][site.split] += 1
        where.setdefault((site.size, site.split),
                         f"{site.path}:{site.line + 1} {site.char}")
    total = sum(sum(c.values()) for c in by_size.values())
    report.append(f"  {total} sized call site(s)"
                  + (f", {len(unsized)} unsized" if unsized else ""))
    for size in sorted(by_size):
        counts = by_size[size]
        report.append(f"  {size[0]}x{size[1]}:"
                      + (" (one split)" if len(counts) == 1 else ""))
        for split, n in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0])):
            a, gap, b = split
            report.append(f"    {a}-{gap}-{b}  x{n}"
                          f"   ({where[(size, split)]})")
    # and the same divisions read across widths, which is what a `split` is
    gaps = collections.Counter(s.split[1] for s in sites if s.split)
    report.append("  gaps: " + " ".join(f"{g}x{n}" for g, n in sorted(gaps.items())))
    lefts = collections.Counter((s.size[0], s.split[0]) for s in sites if s.split)
    report.append("  first part by width: "
                  + " ".join(f"{w}->{a}x{n}" for (w, a), n in sorted(lefts.items())))
    for which in (0, 1):
        got = sorted(part_sizes(rule, which, sizes))
        report.append(f"  {rule.parts[which]} is drawn at: "
                      + " ".join(f"{w}x{h}" for w, h in got))
    for site in unsized:
        report.append(f"  unsized {site.path}:{site.line + 1} {site.char}")


def apply_rule(files: dict[str, list[str]], rule: Rule, ids: dict,
               sizes: dict, report: list[str]) -> bool:
    sites = scan(files, rule, ids)
    if not sites:
        report.append(f"{rule.char}: nothing inlined")
        return False
    plans: list[Plan] = []
    for site in sites:
        got = plan_site(site, rule, sizes, files[site.path])
        if isinstance(got, str):
            report.append(f"  skipped {site.path}:{site.line + 1} {site.char}: {got}")
            continue
        plans.append(got)
    wanted = {p.size for p in plans if p.size}

    # every call site, back to front within each file so the line numbers hold
    # -- a `ref` pair becoming one line is the only edit that moves any, and a
    # block's header, which the same site may promote, is always above it
    by_file: dict[str, list[Plan]] = collections.defaultdict(list)
    for p in plans:
        by_file[p.site.path].append(p)
    for fpath, group in by_file.items():
        lines = files[fpath]
        for p in sorted(group, key=lambda p: -p.site.line):
            site = p.site
            if site.kind == "idc":
                lines[site.line] = rewrite_idc(lines[site.line], rule, p)
            else:
                _, x, y, _ = ref_at(lines, site.line)
                tok = f"{target_name(rule, site)}:{p.size[0]}x{p.size[1]}"
                lines[site.line] = f"ref {tok} {x} {y}"
                del lines[site.line + 1]
                blo, bhi = block_bounds(lines, site.line)
                seq = ref_ids_comment(lines, blo, bhi, site.seq)
                if seq:
                    first = next(k for k in range(blo, bhi) if ref_at(lines, k))
                    head, _ = split_comment(uncomment(lines[first]))
                    lines[first] = f"{head} // {seq}"
            if rule.regional and site.naming == "plain":
                got = promote_header(lines[site.header])
                if got is None:
                    report.append(f"  ! {fpath}:{site.header + 1}: "
                                  "cannot promote the block to a family")
                else:
                    lines[site.header] = got
            report.append(f"  {fpath}:{site.line + 1} {site.char} "
                          f"{'' if p.size is None else '%dx%d' % p.size}"
                          f"{' (+1 borrowed)' if p.borrow else ''}")

    # and the character's own blocks, once every line number has settled
    path, lo, hi, have = existing_target(files, rule)
    lines = files[path]
    lines[lo:hi] = target_blocks(rule, have | wanted, sizes)
    report.append(f"  {path}: {rule.char} now drawn at "
                  + " ".join(f"{w}x{h}" for w, h in sorted(have | wanted))
                  + (f" (new: {' '.join('%dx%d' % s for s in sorted(wanted - have))})"
                     if wanted - have else ""))
    return True


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-i", "--font-dir", default="font")
    ap.add_argument("--ids", action="append", help=G.IDS_HELP)
    ap.add_argument("--dry-run", action="store_true", help="report only; write nothing")
    ap.add_argument("chars", nargs="*", default=None,
                    help=f"characters to un-inline (default: {' '.join(RULES)})")
    args = ap.parse_args()

    chars = args.chars or list(RULES)
    for c in chars:
        if c not in RULES:
            raise SystemExit(f"no rule for {c}; known: {' '.join(RULES)}")

    ids = G.load_ids(args.ids or G.IDS_PATHS)
    files = load_files(args.font_dir)
    before = {p: list(l) for p, l in files.items()}
    REGIONAL_PARTS.update(load_regional_parts(files))
    sizes = load_sizes(files)

    report: list[str] = []
    for c in chars:
        report.append(f"{c}: {RULES[c].why}")
        if RULES[c].split is None:
            probe_rule(files, RULES[c], ids, sizes, report)
        else:
            apply_rule(files, RULES[c], ids, sizes, report)
    print("\n".join(report))

    changed = [p for p in files if files[p] != before[p]]
    if args.dry_run:
        print(f"[dry run] {len(changed)} file(s) would change")
        return 0
    for path in changed:
        with open(path, "w", encoding="utf-8") as f:
            f.write("\n".join(files[path]))
    print(f"{len(changed)} file(s) written")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
