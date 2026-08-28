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

Only a character *one* missing part stands between is counted for that part, so
a count is what drawing it buys on its own. `--greedy N` instead picks N parts
one after another, each time counting what is left, which is the batch to draw.

Usage: python3 scripts/han_next_parts.py [-n 40] [--greedy 30]
"""

from __future__ import annotations

import argparse
import collections
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_ids_composites as G  # noqa: E402


def best_tree(entry):
    """The tree of the character's best-attested plain sequence, if any."""
    for seq, tags in sorted(entry.seqs, key=lambda s: -G.tag_score(s[1])):
        if "？" in seq or "{" in seq:
            continue
        seq = seq.replace("〾", "")
        tree, used = G.parse_ids(seq)
        if tree is not None and used == len(seq):
            return tree
    return None


def collect(inv, ids):
    """`[(cp, op, [components])]` for every undrawn character a line could hold."""
    out = []
    for cp, entry in ids.items():
        if cp in inv.covered or G.block_of(cp) is None:
            continue
        tree = best_tree(entry)
        if tree is None or tree.op not in G.IDC_ARITY:
            continue
        if not all(kid.char is not None for kid in tree.kids):
            continue
        comps = [ord(G.normalize_component(kid.char)) for kid in tree.kids]
        if cp not in comps:
            out.append((cp, tree.op, comps))
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("-n", type=int, default=40, help="rows per table")
    ap.add_argument("--greedy", type=int, default=30, help="batch size, 0 to skip")
    args = ap.parse_args()

    parts = G.load_name_parts(G.__dict__.get("FONT_DIR", "font"))
    inv = G.load_inventory("font", parts)
    ids = G.load_ids(G.IDS_PATH)
    todo = collect(inv, ids)

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

    undrawn = collections.Counter()
    unsized = collections.Counter()
    orient: dict[int, collections.Counter] = collections.defaultdict(collections.Counter)
    for cp, op, comps in todo:
        if op in G.ENCLOSING:
            tag = "15x16.NxM"
        else:
            tag = "Nx16" if op in G.HORIZONTAL else "15xN"
        missing = {c for c in comps if not drawn(c)}
        if len(missing) == 1:
            part = next(iter(missing))
            undrawn[part] += 1
            orient[part][tag] += 1
        elif not missing:
            # By slot rather than by part: an enclosure's two slots ask opposite
            # things of a drawing, so the same character can fit one and not the
            # other.
            bad = {c for slot, c in enumerate(comps) if not fits(c, op, slot)}
            if len(bad) == 1:
                unsized[next(iter(bad))] += 1
                orient[next(iter(bad))][tag] += 1

    def table(title, counter):
        total = sum(counter.values())
        print(f"\n== {title}: {len(counter)} parts blocking {total} characters")
        for cp, n in counter.most_common(args.n):
            sizes = "  ".join(f"{t}:{k}" for t, k in orient[cp].most_common())
            print(f"   {chr(cp)} U+{cp:05X}  {n:5d}  [{sizes}]")

    print(f"drawn: {len(inv.covered_full)} at the full box, {len(inv.covered)} at any size")
    table("A. not drawn at all", undrawn)
    table("B. drawn, but at no size the slot takes", unsized)

    if args.greedy:
        print(f"\n== batch of {args.greedy}, each pick counting what the last one left")
        missing_of = [
            (cp, frozenset(c for c in comps if not drawn(c))) for cp, _, comps in todo
        ]
        picked: set[int] = set()
        total = 0
        for step in range(1, args.greedy + 1):
            gain: collections.Counter = collections.Counter()
            for cp, miss in missing_of:
                rest = miss - picked
                if len(rest) == 1:
                    gain[next(iter(rest))] += 1
            if not gain:
                break
            part, n = gain.most_common(1)[0]
            picked.add(part)
            total += n
            print(f"   {step:2d}. {chr(part)} U+{part:05X}  +{n:4d}  running total {total:5d}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
