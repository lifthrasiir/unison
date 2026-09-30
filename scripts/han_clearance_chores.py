#!/usr/bin/env python3
"""Rank the part variants whose drawing would clear the most clearance chores.

`uniform fix --optimize-clearance` already picks the best layout out of the
variants the source draws, so a clearance chore it leaves behind is a line no
existing variant can put right: the parts are too wide together (the total is
under the `audit ideal-clearance` band, an *overrun*) or too narrow together (it
is over the band, a *canyon*). Either way what is wanted is a new variant of one
of the parts, a size the source does not draw yet -- smaller for an overrun, in
between two drawn sizes (or larger) for a canyon. This asks which ones.

The model, per chore line (one glyph's IDC line; one line is usually two or
three chores, and the counts here are lines):

  * The total telescopes down to the parent's extent less the parts' ink, so a
    part redrawn `d` cells shorter along the axis raises the total by `d`, and
    once the total is inside the band the fixer can always spread it (the gaps
    are arithmetic, see `fix/clearance/mod.rs`). A *plan* for the line is then
    a set of new part sizes whose changes sum into the band. Ink is assumed to
    follow the box, which holds for parts drawn tight and is the model's main
    approximation.
  * A part is only resized within `--ratio` of its current length, and by at
    most `--max-step` cells (default 1). Without the step bound a plan that
    changes one part by two cells always outscores two parts changing by one,
    so the list fills up with the most common part squeezed as far as the
    ratio allows (木 5x16 -> 3x16) rather than the spread-out change a designer
    would make. A line no plan within both bounds solves -- a ⿲ of three
    ⿰-sized parts overrunning by five -- wants a different decomposition, and
    is listed apart.
  * A new size that some variant already draws costs nothing. One that does not
    is either drawn outright, or -- when the part is itself an IDC split --
    *composed*: along its own axis a composite shrinks for free as far as its
    own clearances allow, and past that one of its parts has to shrink (the
    same question again, one level down); across its axis every part has to
    shrink with it. That is where the sub-part credit comes from: a plan that
    needs `n` drawings gives each of them `1/n` of the line.
  * A line counts for the best plan a drawing is part of, and the drawings are
    picked greedily: each pick is the drawing with the most credit over the
    lines still open, after which it is treated as drawn and the credit
    recomputed. So the list is a work order, and a drawing that only helps
    lines an earlier pick already cleared sinks.

The clearances of every composite, chores or not, come from a second `uniform
test` run over a copy of the source whose band nobody can meet, which makes
every IDC line report its clearances.

Known problems, as of 2026-09-30 (973 chore lines, 1931 chores), for whoever
picks this up next:

  * **Undrawable sizes.** Nothing knows how small a part can be drawn, so the
    list asks for 口 2x12, 氵 2x12 and 刂 2x16. Nor does anything know about
    parity: 木 is drawn only at odd widths (5/7/9/11/15) and 魚 at 7/9,
    presumably for the centre stroke, so 木 4x16 is likely no help either. A
    fix: floor a part at the smallest length any of its variants draws along
    the axis, keep the parity when every drawn length shares one, and read a
    hand-kept exclusion list for the rest.
  * **⿲ is one problem, not seven hundred.** About 700 of the lines are ⿲
    overruns of parts sized for ⿰ (5+7+5 in a 15-wide box). Asked per line,
    the greedy answers with scattered variants; the more useful question is
    one ⿲ width per part, decided once, and which lines that clears.
  * **Some lines are a decomposition problem.** A line no plan within
    `--max-step` solves (180 at the default; e.g. 𩹾 puts an 11-wide part in
    a ⿲, total -10) is not something to draw parts for: it wants writing as a
    ⿰ over a composite instead. The list should say so and name the
    composite, rather than only listing the line.
  * **Ink is assumed to follow the box.** Right for 束 (the one case checked),
    wrong for a part with a bearing or hardblanks of its own. Exact numbers
    would need the Rust fixer to answer "what if this variant existed".
  * **A regional pattern line counts once per region**: 𩹾's g/t/j/p lines are
    four lines here although the source writes them once or twice.
  * **`fix --optimize-clearance` misses some swaps.** Lines the "no new
    drawing" section calls a swap were not rewritten by the fixer: 𣉚
    (`han-b071.unf`), 𦝆 (`han-b130.unf`), and 蝲 (`han-0141.unf`), whose line
    loses every chore once `han-675f:9x16` is written `7x16` -- a two-cell
    swap, so it shows only with `--max-step 2`. Cause not investigated.

Usage: python3 scripts/han_clearance_chores.py [-i font] [-n 60] [--ratio 0.6]
           [--max-step 1] [--depth 2] [--list-free 30] [--list-unsolved 30] [-v]

Needs `target/release/uniform` (or `--uniform PATH`); takes about ten seconds.
"""

from __future__ import annotations

import argparse
import itertools
import math
import os
import re
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict
from dataclasses import dataclass, field

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_ids_composites as G  # noqa: E402

AUDIT_RE = re.compile(r"^audit\s+ideal-clearance\s+(\S+)\s+(-?\d+)\s+(-?\d+)(?:\s+(-?\d+)\s+(-?\d+))?")
TOTAL_RE = re.compile(
    r"^chore: (?P<file>[^:]+):(?P<line>\d+): glyph '(?P<glyph>[^']+)': "
    r"`(?P<op>\S+)` leaves (?P<total>-?\d+)(?: (?P<dir>down|across))? in total, "
    r"outside the ideal (?P<lo>-?\d+)\.\.(?P<hi>-?\d+)[^—]*(?:— (?P<rest>.*))?$"
)
CHORE_RE = re.compile(r"^chore: ([^:]+:\d+): glyph '([^']+)'")
SEGMENT_RE = re.compile(r"-?\d+ between (the \w+ edge|'[^']+') and (the \w+ edge|'[^']+')")
# Any single-cell budget beyond this many plans per line is noise: the greedy
# only ever reads each line's best few.
MAX_WAYS = 256


# --------------------------------------------------------------------------
# running uniform
# --------------------------------------------------------------------------

def uniform_cmd(args) -> list[str]:
    if args.uniform:
        return [args.uniform]
    exe = os.path.join("target", "release", "uniform")
    if not os.path.exists(exe):
        sys.exit(f"{exe} is missing; `cargo build -r` first, or pass --uniform")
    return [exe]


def run_test(cmd: list[str], font_dir: str) -> list[str]:
    out = subprocess.run(
        cmd + ["test", "-i", font_dir, "--chores"],
        capture_output=True, text=True, encoding="utf-8",
    )
    return (out.stdout + out.stderr).splitlines()


def forced_copy(font_dir: str, tmp: str) -> str:
    """A copy of the source every IDC line fails the clearance band of.

    Symlinks except for the file(s) stating `audit ideal-clearance`, whose band
    is replaced with one no clearance reaches.
    """
    dst = os.path.join(tmp, "font")
    os.mkdir(dst)
    for fname in os.listdir(font_dir):
        src = os.path.abspath(os.path.join(font_dir, fname))
        text = ""
        if fname.endswith(".unf"):
            with open(src, encoding="utf-8") as f:
                text = f.read()
        if not re.search(r"(?m)^audit\s+ideal-clearance", text):
            os.symlink(src, os.path.join(dst, fname))
            continue
        lines = [
            re.sub(r"^(audit\s+ideal-clearance\s+\S+).*$", r"\1 999 999 999 999", line)
            for line in text.split("\n")
        ]
        with open(os.path.join(dst, fname), "w", encoding="utf-8") as f:
            f.write("\n".join(lines))
    return dst


# --------------------------------------------------------------------------
# the lines
# --------------------------------------------------------------------------

def label_of(name: str):
    """`(base, (w, h), cavity, direction)`, or `None` for a name with no size."""
    if ":" not in name or "|" in name:
        return None
    base, label = name.split(":", 1)
    size, cavity, direction = G.parse_label(label)
    if size is None:
        return None
    return base, size, cavity, direction


@dataclass
class Line:
    """One IDC line's clearances along one axis, as `uniform test` reports them."""

    where: str        # file:line
    glyph: str
    op: str
    axis: str         # "x" or "y"
    total: int
    parts: list[str]  # in written order; for an enclosure, outer first

    @property
    def enclosing(self) -> bool:
        return self.op in G.ENCLOSING

    def key(self) -> tuple[str, str]:
        return self.glyph, self.axis


def parse_totals(lines: list[str]) -> list[tuple[Line, int, int]]:
    out = []
    for text in lines:
        m = TOTAL_RE.match(text)
        if m is None:
            continue
        op = m["op"]
        if op in G.ENCLOSING:
            axis = "y" if m["dir"] == "down" else "x"
        else:
            axis = "x" if op in G.HORIZONTAL else "y"
        # the segments run edge to edge, each naming its two sides, so the
        # parts are the first segment's near side and every segment's far side
        # -- a part a split names twice (⿲ 木 X 木) is two parts
        segs = SEGMENT_RE.findall(m["rest"] or "")
        sides = [segs[0][0]] + [far for _, far in segs] if segs else []
        parts = [s[1:-1] for s in sides if s.startswith("'")]
        if op in G.ENCLOSING:
            parts = list(dict.fromkeys(parts))
            # the outer part is the one promising a cavity; the message lists
            # the two in the order the walls meet them
            parts.sort(key=lambda n: (label_of(n) or (0, 0, None))[2] is None)
        line = Line(f"{m['file']}:{m['line']}", m["glyph"], op, axis, int(m["total"]), parts)
        out.append((line, int(m["lo"]), int(m["hi"])))
    return out


# --------------------------------------------------------------------------
# what is drawn
# --------------------------------------------------------------------------

@dataclass(frozen=True)
class Demand:
    """A variant to draw: `base:WxH[.NxM][-dir]`."""

    base: str
    size: tuple[int, int]
    cavity: tuple[int, int] | None
    direction: str | None

    def __str__(self) -> str:
        label = f"{self.size[0]}x{self.size[1]}"
        if self.cavity is not None:
            label += f".{self.cavity[0]}x{self.cavity[1]}"
        if self.direction is not None:
            label += f"-{self.direction}"
        return f"{self.base}:{label}"

    def char(self) -> str:
        hn = G.parse_han_name(self.base)
        return chr(hn.cp) if hn is not None else "?"


class Drawn:
    """Every variant label the source reaches, by base name."""

    def __init__(self, inv: G.Inventory):
        self.labels: dict[str, list[tuple]] = defaultdict(list)
        for name in set(inv.drawings) | set(inv.aliases):
            if inv.resolve(name) is None:
                continue
            got = label_of(name)
            if got is not None:
                base, size, cavity, direction = got
                self.labels[base].append((size, cavity, direction))
        self.added: set[Demand] = set()

    def has(self, d: Demand) -> bool:
        if d in self.added:
            return True
        for size, cavity, direction in self.labels.get(d.base, ()):
            if size != d.size or direction not in (None, d.direction):
                continue
            if d.cavity is None or (cavity is not None and cavity == d.cavity):
                return True
        return False


# --------------------------------------------------------------------------
# plans
# --------------------------------------------------------------------------

# A way to get something: the drawings it still needs, and how many composites
# it writes on the way (only reported, never counted against it).
Way = tuple[frozenset, int]


def minimal(ways: list[Way]) -> list[Way]:
    ways = sorted(set(ways), key=lambda w: (len(w[0]), w[1], sorted(map(str, w[0]))))
    kept: list[Way] = []
    for w in ways:
        if any(k[0] <= w[0] and k[1] <= w[1] for k in kept):
            continue
        kept.append(w)
        if len(kept) >= MAX_WAYS:
            break
    return kept


def product(options: list[list[Way]]) -> list[Way]:
    ways: list[Way] = [(frozenset(), 0)]
    for opts in options:
        ways = minimal([(a | b, n + m) for (a, n), (b, m) in itertools.product(ways, opts)])
        if not ways:
            break
    return ways


class Planner:
    def __init__(self, drawn: Drawn, composites: dict, band: tuple[int, int], ratio: float, depth: int,
                 max_step: int):
        self.drawn = drawn
        # glyph name -> {axis: Line}, splits only: a composite part is resized
        # through its own line only when that line is a split
        self.composites = composites
        self.band = band
        self.ratio = ratio
        self.depth = depth
        self.max_step = max_step
        self.memo: dict = {}

    def bounds(self, length: int, limit: int) -> tuple[int, int]:
        lo = max(1, math.ceil(length * self.ratio))
        hi = min(limit, math.floor(length / self.ratio))
        return lo, hi

    def line_ways(self, line: Line, need_lo: int, need_hi: int, depth: int, extent: tuple[int, int]) -> list[Way]:
        """Plans that move `line`'s total by `need_lo..need_hi` (signed)."""
        if need_lo <= 0 <= need_hi:
            return [(frozenset(), 0)]
        sign = 1 if need_lo > 0 else -1  # +1: the parts have to shrink
        k_lo, k_hi = sorted((abs(need_lo), abs(need_hi)))
        if need_lo < 0 < need_hi:
            k_lo = 0
        # per part: (name, the most it may change by, the Demand for a change)
        movers = []
        ax = 0 if line.axis == "x" else 1
        for i, name in enumerate(line.parts):
            got = label_of(name)
            if got is None:
                continue
            base, size, cavity, direction = got
            if line.enclosing and i == 0:
                if cavity is None:
                    continue  # an outer part promising no cavity: nothing to resize
                # the outer part: more room inside it is a larger cavity
                length = cavity[ax]
                lo, hi = self.bounds(length, extent[ax])
                room = (hi - length) if sign > 0 else (length - lo)
                movers.append((name, min(room, self.max_step), i))
            else:
                length = size[ax]
                lo, hi = self.bounds(length, extent[ax])
                room = (length - lo) if sign > 0 else (hi - length)
                movers.append((name, min(room, self.max_step), i))
        if not movers:
            return []
        ways: list[Way] = []
        for k in range(max(k_lo, 1), k_hi + 1):
            for split in distributions(k, [m[1] for m in movers]):
                options = []
                for (name, _, slot), d in zip(movers, split):
                    if d == 0:
                        continue
                    slot_dir = None if line.enclosing else slot_direction(line, slot)
                    options.append(self.resize(name, ax, sign * d, slot_dir, depth,
                                               outer=line.enclosing and slot == 0))
                ways.extend(product(options))
        return minimal(ways)

    def resize(self, name: str, ax: int, shrink: int, slot_dir: str | None, depth: int,
               outer: bool = False) -> list[Way]:
        """Ways to get `name` `shrink` cells shorter along axis `ax` (0 is x).

        For an enclosure's outer part it is the cavity that grows instead. A
        name drawn for no slot in particular is asked for as the slot's own
        direction, which is what the fixer would rank first.
        """
        base, size, cavity, direction = label_of(name)
        if outer:
            cav = list(cavity)
            cav[ax] += shrink
            want = Demand(base, size, tuple(cav), direction)
        else:
            new = list(size)
            new[ax] -= shrink
            want = Demand(base, tuple(new), None, direction or slot_dir)
        return self.obtain(name, want, depth)

    def obtain(self, current: str, want: Demand, depth: int) -> list[Way]:
        key = (current, want, depth)
        if key in self.memo:
            return self.memo[key]
        self.memo[key] = []  # a cycle through the composites gets nothing
        if self.drawn.has(want):
            ways = [(frozenset(), 0)]
        else:
            ways = [(frozenset([want]), 0)]
            if depth < self.depth and want.cavity is None:
                ways += self.compose(current, want, depth + 1)
        ways = minimal(ways)
        self.memo[key] = ways
        return ways

    def compose(self, current: str, want: Demand, depth: int) -> list[Way]:
        """Ways to write `want` as the IDC line `current` is written as."""
        axes = self.composites.get(current)
        if not axes:
            return []
        (line,) = axes.values()
        _, size, _, _ = label_of(current)
        dx, dy = want.size[0] - size[0], want.size[1] - size[1]
        along = dx if line.axis == "x" else dy
        across = dy if line.axis == "x" else dx
        lo, hi = self.band
        ways: list[Way] = [(frozenset(), 0)]
        if across:
            if not lo <= line.total <= hi:
                return []  # the composite has a chore of its own already
            cross = 1 if line.axis == "x" else 0
            options = []
            for slot, name in enumerate(line.parts):
                if label_of(name) is None:
                    return []  # a nested split does not change size
                options.append(self.resize(name, cross, -across, slot_direction(line, slot), depth))
            ways = product(options)
        if along:
            t = line.total + along
            ways = product([ways, self.line_ways(line, lo - t, hi - t, depth, want.size)])
        return [(w, n + 1) for w, n in ways]


def slot_direction(line: Line, slot: int) -> str | None:
    if slot == 0:
        return "l" if line.axis == "x" else "u"
    if slot == len(line.parts) - 1:
        return "r" if line.axis == "x" else "d"
    return None


def distributions(k: int, rooms: list[int]):
    """Every way to write `k` as a sum of `len(rooms)` terms, term i in 0..rooms[i]."""
    if not rooms:
        if k == 0:
            yield ()
        return
    for first in range(min(k, rooms[0]), -1, -1):
        for rest in distributions(k - first, rooms[1:]):
            yield (first,) + rest


# --------------------------------------------------------------------------
# the greedy pick
# --------------------------------------------------------------------------

@dataclass
class Chore:
    line: Line
    lo: int
    hi: int
    chores: int
    ways: list[Way] = field(default_factory=list)

    @property
    def overrun(self) -> bool:
        return self.line.total < self.lo


def greedy(chores: list[Chore], limit: int):
    open_ = [c for c in chores if c.ways and all(w[0] for w in c.ways)]
    picks = []
    while open_ and len(picks) < limit:
        credit: dict[Demand, float] = defaultdict(float)
        touched: dict[Demand, list[Chore]] = defaultdict(list)
        for c in open_:
            best: dict[Demand, float] = {}
            for need, _ in c.ways:
                share = 1 / len(need)
                for d in need:
                    if share > best.get(d, 0):
                        best[d] = share
            for d, share in best.items():
                credit[d] += share
                touched[d].append(c)
        if not credit:
            break
        pick = max(credit, key=lambda d: (credit[d], str(d)))
        still = []
        cleared = []
        for c in open_:
            c.ways = minimal([(need - {pick}, n) for need, n in c.ways])
            (cleared if any(not need for need, _ in c.ways) else still).append(c)
        open_ = still
        picks.append((pick, credit[pick], cleared, touched[pick]))
    return picks, open_


# --------------------------------------------------------------------------

def glyph_char(glyph: str) -> str:
    hn = G.parse_han_name(glyph)
    return chr(hn.cp) if hn is not None else "?"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("-i", "--font-dir", default="font")
    ap.add_argument("--uniform", help="the uniform binary (default target/release/uniform)")
    ap.add_argument("-n", type=int, default=60, help="picks to list")
    ap.add_argument("--ratio", type=float, default=0.6,
                    help="the least a part may shrink to, and the inverse of the most it may grow to, "
                         "as a fraction of its current length (default 0.6)")
    ap.add_argument("--max-step", type=int, default=1,
                    help="the most cells one part may change by in one plan (default 1)")
    ap.add_argument("--depth", type=int, default=2,
                    help="how many levels of composite parts to recompose through (default 2)")
    ap.add_argument("--list-free", type=int, default=30,
                    help="lines solvable with no new drawing, to list (default 30)")
    ap.add_argument("--list-unsolved", type=int, default=30,
                    help="lines no plan within --ratio solves, to list (default 30)")
    ap.add_argument("-v", "--verbose", action="store_true", help="list each pick's lines")
    args = ap.parse_args()

    cmd = uniform_cmd(args)
    real = run_test(cmd, args.font_dir)
    with tempfile.TemporaryDirectory() as tmp:
        forced = run_test(cmd, forced_copy(args.font_dir, tmp))

    band = None
    for fname in sorted(os.listdir(args.font_dir)):
        if fname.endswith(".unf"):
            with open(os.path.join(args.font_dir, fname), encoding="utf-8") as f:
                for text in f:
                    m = AUDIT_RE.match(text)
                    if m:
                        band = (int(m[2]), int(m[3]))
    if band is None:
        sys.exit("no `audit ideal-clearance` in the source")

    composites: dict[str, dict[str, Line]] = defaultdict(dict)
    for line, _, _ in parse_totals(forced):
        if not line.enclosing:
            composites[line.glyph][line.axis] = line

    per_line: dict[tuple, Chore] = {}
    counts = Counter(m.groups() for t in real if (m := CHORE_RE.match(t)))
    for line, lo, hi in parse_totals(real):
        per_line[line.key()] = Chore(line, lo, hi, 0)
    for c in per_line.values():
        # an enclosure's two axes share the glyph's count; give it to one
        c.chores = counts.pop((c.line.where, c.line.glyph), 0)

    parts_dir = G.load_name_parts(args.font_dir)
    inv = G.load_inventory(args.font_dir, parts_dir)
    drawn = Drawn(inv)
    planner = Planner(drawn, composites, band, args.ratio, args.depth, args.max_step)

    chores = list(per_line.values())
    for c in chores:
        _, size, _, _ = label_of(c.line.glyph) or (None, (G.BOX_W, G.BOX_H), None, None)
        c.ways = planner.line_ways(c.line, c.lo - c.line.total, c.hi - c.line.total, 0, size)

    free = [c for c in chores if any(not need for need, _ in c.ways)]
    unsolved = [c for c in chores if not c.ways]
    n_over = sum(c.overrun for c in chores)
    print(f"{len(chores)} chore lines ({sum(c.chores for c in chores)} chores): "
          f"{n_over} overrun (a part has to shrink), {len(chores) - n_over} canyon (a part has to grow)")
    print(f"  {len(free)} solvable with no new drawing (a drawn variant, or a part recomposed from them)")
    print(f"  {len(unsolved)} no plan within --ratio {args.ratio} --max-step {args.max_step} solves")

    if args.list_free and free:
        # A plan writing no composite is a swap `fix --optimize-clearance`
        # should have found itself, so it is worth telling apart.
        print(f"\n== solvable with no new drawing\n")
        for c in free[: args.list_free]:
            writes = min(n for need, n in c.ways if not need)
            how = f"recompose {writes} part(s)" if writes else "swap drawn variants (a fixer miss?)"
            print(f"  {c.line.where}  {c.line.glyph} {glyph_char(c.line.glyph)} {c.line.op} "
                  f"total {c.line.total:+d}  {' '.join(c.line.parts)}  -- {how}")

    picks, left = greedy(chores, args.n)
    done = len(free)
    print(f"\n== the drawings, in the order that clears the most lines\n")
    print(f"{'#':>4} {'credit':>7} {'done':>5}  variant")
    for rank, (d, credit, cleared, touched) in enumerate(picks, 1):
        done += len(cleared)
        froms = Counter()
        for c in touched:
            for name in c.line.parts:
                got = label_of(name)
                if got and got[0] == d.base:
                    froms[f"{got[1][0]}x{got[1][1]}"] += 1
        was = ",".join(s for s, _ in froms.most_common(3))
        chars = "".join(dict.fromkeys(glyph_char(c.line.glyph) for c in touched))
        print(f"{rank:4d} {credit:7.2f} {done:5d}  {d} {d.char()}"
              f"{f'  (from {was})' if was else ''}  {chars[:24]}")
        if args.verbose:
            for c in cleared:
                print(f"            cleared {c.line.where} {c.line.glyph} {glyph_char(c.line.glyph)}")
    print(f"\n{len(left)} lines still open after {len(picks)} picks")

    if args.list_unsolved and unsolved:
        print(f"\n== no plan within --ratio {args.ratio} --max-step {args.max_step}, worst first\n")
        unsolved.sort(key=lambda c: -abs(c.line.total - (c.lo if c.overrun else c.hi)))
        for c in unsolved[: args.list_unsolved]:
            print(f"  {c.line.where}  {c.line.glyph} {glyph_char(c.line.glyph)} {c.line.op} "
                  f"total {c.line.total:+d}  {' '.join(c.line.parts)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
