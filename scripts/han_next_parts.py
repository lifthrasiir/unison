#!/usr/bin/env python3
"""Rank the parts that unlock the most han characters, by what is blocking them.

`gen_ids_composites.py` writes an IDC line for every character whose parts the
source already draws; this asks the opposite question -- which part, once drawn,
would let it write the most new lines. Two kinds of blockage are counted apart,
because the work they ask for is not the same size:

  A. the part is not drawn at all, and
  B. the part is drawn, but at no size that could sit in the slot the line needs
     (`compose::fits_slot` / `compose::fits_enclosure_slot`, mirrored in
     `gen_ids_composites.feasible`), so all that is wanted is one more variant
     of a drawing that already exists.

An enclosure counts under B too, and its two slots ask opposite things: the
outer part wants a 15x16 drawing that promises a cavity (`:15x16.NxM`) and the
inner one wants a drawing small enough to sit in some such cavity.

A character is asked about every line the generator could write for it, not just
one: every usable sequence and every inlined form of one, read through the very
functions the generator reads them with (`sequence_trees`, `candidates`), under
the same `--allow-ivi`/`--no-inline`/`--inline` flags. So a part counts for a
character when *some* such line is one missing part away, and a character with a
line that is only a size away counts under B and nowhere else -- a new variant is
the smaller piece of work, and the generator itself prefers the line whose parts
exist. A character some line already fits is not blocked at all: it is counted
apart, as the lines a run of the generator would write today.

Only a line *one* missing part stands between is counted for that part, so a
count is what drawing it buys on its own. A character several lines are one part
away from counts once for each such part, which is why a table's total counts
characters rather than adding its rows up. `--greedy N` instead picks N parts one
after another, each time counting what is left, which is the batch to draw.

What is left over is a line whose every part sits in its own slot but not all of
them together -- a split that overruns the box, an enclosure whose drawn cavity
is too small for the drawn inner part -- which names no single part to redraw and
so is only counted.

Usage: python3 scripts/han_next_parts.py [-n 40] [--greedy 30]
"""

from __future__ import annotations

import argparse
import collections
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_ids_composites as G  # noqa: E402


def collect(inv, ids, allow_ivi, inline, splits):
    """`[(cp, [Candidate])]` for every undrawn character some line could hold.

    The candidates are in the generator's own order, best-attested sequence
    first, which is what decides the size tag a part is reported under; and a
    fallback sequence's only count where no attested one gives any, since that
    is the only case in which the generator would write one.
    """
    out = []
    for cp, entry in ids.items():
        if cp in inv.covered or G.block_of(cp) is None:
            continue
        attested, fallback = [], []
        for tree, _, is_fallback in G.sequence_trees(entry, allow_ivi):
            if tree is None or tree.op not in G.IDC_ARITY:
                continue
            (fallback if is_fallback else attested).extend(
                cand
                for cand in G.candidates(tree, inline, splits, inv.pixel_drawn)
                if cp not in cand.comps
            )
        cands = attested or fallback
        if cands:
            out.append((cp, cands))
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("-i", "--font-dir", default="font")
    ap.add_argument("--ids", action="append", help=G.IDS_HELP)
    ap.add_argument("-n", type=int, default=40, help="rows per table")
    ap.add_argument("--greedy", type=int, default=30, help="batch size, 0 to skip")
    ap.add_argument("--allow-ivi", action="store_true",
                    help="also use sequences marked 〾, as the generator's flag does")
    ap.add_argument("--no-inline", action="store_true",
                    help="do not count a same-axis nested operand inlined into ⿲/⿳")
    ap.add_argument("--inline", action="store_true",
                    help="also count an operand named by a character whose own IDS "
                         "splits the same way, as the generator's flag does")
    args = ap.parse_args()

    parts = G.load_name_parts(args.font_dir)
    inv = G.load_inventory(args.font_dir, parts)
    ids = G.load_ids(args.ids or G.IDS_PATHS)
    inline = not args.no_inline
    splits = G.build_split_index(ids, args.allow_ivi) if inline and args.inline else None
    todo = collect(inv, ids, args.allow_ivi, inline, splits)

    # What a line may name a character as: for one drawn per region that is the
    # labels every region draws, which is the same question `feasible` asks and
    # so the same answer a generated line would be held to.
    def variants(cp):
        family = inv.families.get(cp)
        return family.shared if family is not None else []

    def drawn(cp):
        return bool(variants(cp))

    # Every cavity the source promises anywhere, which is what an enclosure's
    # inner slot is measured against. Collected once: it does not depend on the
    # character being asked about.
    cavities = [
        v.cavity
        for family in inv.families.values()
        for v in family.shared
        if (v.w, v.h) == (G.BOX_W, G.BOX_H) and v.cavity is not None
    ]

    def fits(cp, op, slot):
        """Whether some drawing of `cp` could sit in slot `slot` of an `op` line."""
        drawings = variants(cp)
        if op in G.ENCLOSING:
            # The outer slot wants the glyph exactly, with a cavity; the inner
            # one wants anything without a cavity that some drawn cavity holds.
            if slot == 0:
                return any(
                    (v.w, v.h) == (G.BOX_W, G.BOX_H) and v.cavity is not None
                    for v in drawings
                )
            return any(
                v.cavity is None and v.w <= n and v.h <= m
                for v in drawings
                for n, m in cavities
            )
        horizontal = op in G.HORIZONTAL
        axis, cross = (G.BOX_W, G.BOX_H) if horizontal else (G.BOX_H, G.BOX_W)
        return any(
            (v.h if horizontal else v.w) == cross and (v.w if horizontal else v.h) < axis
            for v in drawings
        )

    def size_tag(op):
        if op in G.ENCLOSING:
            return "15x16.NxM"
        return "Nx16" if op in G.HORIZONTAL else "15xN"

    ready = 0
    overrun = 0
    # part -> the characters it is the one blocker of, per category
    undrawn: dict[int, set[int]] = collections.defaultdict(set)
    unsized: dict[int, set[int]] = collections.defaultdict(set)
    orient: dict[int, collections.Counter] = collections.defaultdict(collections.Counter)
    # cp -> the missing parts of each of its lines, for the greedy batch
    missing_of: list[tuple[int, list[frozenset[int]]]] = []
    for cp, cands in todo:
        if any(G.feasible(inv, c.op, c.comps, c.inlined).fits for c in cands):
            ready += 1
            continue
        missing = [frozenset(c for c in cand.comps if not drawn(c)) for cand in cands]
        if not all(missing):
            # Some line is only a size away, so that is the work to count; by
            # slot rather than by part, since an enclosure's two slots ask
            # opposite things of a drawing and the same character can fit one
            # and not the other.
            blockers = {}
            together = False
            for cand, miss in zip(cands, missing):
                if miss:
                    continue
                bad = {c for slot, c in enumerate(cand.comps) if not fits(c, cand.op, slot)}
                if len(bad) == 1:
                    blockers.setdefault(next(iter(bad)), cand.op)
                together = together or not bad
            if not blockers and together:
                overrun += 1
            for part, op in blockers.items():
                unsized[part].add(cp)
                orient[part][size_tag(op)] += 1
            continue
        missing_of.append((cp, missing))
        blockers = {}
        for cand, miss in zip(cands, missing):
            if len(miss) == 1:
                blockers.setdefault(next(iter(miss)), cand.op)
        for part, op in blockers.items():
            undrawn[part].add(cp)
            orient[part][size_tag(op)] += 1

    def table(title, blocked):
        chars = set().union(*blocked.values()) if blocked else set()
        print(f"\n== {title}: {len(blocked)} parts blocking {len(chars)} characters")
        rows = sorted(blocked.items(), key=lambda kv: (-len(kv[1]), kv[0]))
        for cp, of in rows[: args.n]:
            sizes = "  ".join(f"{t}:{k}" for t, k in orient[cp].most_common())
            print(f"   {chr(cp)} U+{cp:05X}  {len(of):5d}  [{sizes}]")

    print(f"drawn: {len(inv.covered_full)} at the full box, {len(inv.covered)} at any size")
    print(f"ready: {ready} characters some line already fits "
          f"(gen_ids_composites.py writes them)")
    table("A. not drawn at all", undrawn)
    table("B. drawn, but at no size the slot takes", unsized)
    print(f"\n== C. every part fits its own slot but not together: {overrun} characters")

    if args.greedy:
        print(f"\n== batch of {args.greedy}, each pick counting what the last one left")
        picked: set[int] = set()
        pending = missing_of
        total = 0
        for step in range(1, args.greedy + 1):
            gain: collections.Counter = collections.Counter()
            for _, options in pending:
                for part in {next(iter(m - picked)) for m in options if len(m - picked) == 1}:
                    gain[part] += 1
            if not gain:
                break
            part, n = min(gain.items(), key=lambda kv: (-kv[1], kv[0]))
            picked.add(part)
            total += n
            # a character a pick unlocked is done, whichever of its lines did it
            pending = [(cp, o) for cp, o in pending if not any(m <= picked for m in o)]
            print(f"   {step:2d}. {chr(part)} U+{part:05X}  +{n:4d}  running total {total:5d}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
