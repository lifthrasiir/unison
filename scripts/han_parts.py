#!/usr/bin/env python3
"""Extract common components (radicals etc.) from han-*.unf bitmap glyphs.

Uses BabelStone IDS.TXT to learn each character's top-level composition
(left-right or top-bottom), then mines pixel-identical regions shared by many
glyphs with the same IDS component on the same side. Shared regions become
part glyphs in font/han-parts.unf and the original glyphs are rewritten as a
residual pixel grid plus `ref` lines.

A part is primarily a rectangular slice (left/right columns or top/bottom
rows), optionally extended by small "fragments" just beyond the cut line —
stroke tips of the radical that stick out past the best shared rectangle
(e.g. the tip of 亻's falling stroke). Fragment absorption keeps the whole
radical in one part instead of leaving orphan pixels in the residual grid.

Usage:
  python3 scripts/han_parts.py stats   [--min-uses N] [--no-ragged]
  python3 scripts/han_parts.py apply   [--min-uses N] [--no-ragged]
  python3 scripts/han_parts.py audit             # report leftover fragments
  python3 scripts/han_parts.py flatten           # undo: refs -> plain grids

All modes accept --font-dir to work on a copy of the font directory.
`apply` verifies in-memory that every rewritten glyph composites back to the
exact original bitmap before writing anything. Already-refactored glyphs
(grid followed by `ref` lines) are left alone by stats/apply, so `apply` is
safe to re-run after adding new glyphs; use `flatten` first to redo
everything from scratch.
"""

import argparse
import math
import os
import re
from collections import Counter, defaultdict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IDS_PATH = os.path.join(ROOT, "scripts", "IDS.TXT")

W = H = 16
BLANK = ".."
FRAG_MAX_CELLS = 8  # audit: fragments larger than this are body strokes
FRAG_MAX_DEPTH = 3  # how far past the cut line a fragment may reach

# ---------------------------------------------------------------- IDS parsing

# arity of IDS operators
OPS = {
    "⿰": 2, "⿱": 2, "⿲": 3, "⿳": 3, "⿴": 2, "⿵": 2, "⿶": 2, "⿷": 2,
    "⿸": 2, "⿹": 2, "⿺": 2, "⿻": 2, "⿼": 2, "⿽": 2, "⿾": 1, "⿿": 1,
}


def tokenize_ids(s):
    """Split an IDS into tokens: operators, chars, {nn} refs. Drops 〾/VS."""
    toks = []
    i = 0
    while i < len(s):
        ch = s[i]
        if ch == "{":
            j = s.index("}", i)
            toks.append(s[i : j + 1])
            i = j + 1
        elif ch == "〾" or 0xFE00 <= ord(ch) <= 0xFE0F or 0xE0100 <= ord(ch) <= 0xE01EF:
            i += 1
        else:
            toks.append(ch)
            i += 1
    return toks


def subtree(toks, i):
    """Return (subtree-token-list, next-index) starting at toks[i]."""
    t = toks[i]
    if t in OPS:
        out = [t]
        j = i + 1
        for _ in range(OPS[t]):
            if j >= len(toks):
                return out, j  # malformed; bail
            sub, j = subtree(toks, j)
            out.extend(sub)
        return out, j
    return [t], i + 1


def parse_ids_file(path):
    """codepoint -> (op, [component subtree strings]) for the preferred sequence."""
    entries = {}
    with open(path, encoding="utf-8-sig") as f:
        for line in f:
            if not line.startswith("U+"):
                continue
            fields = line.rstrip("\r\n").split("\t")
            if len(fields) < 3:
                continue
            cp = int(fields[0][2:], 16)
            # prefer G-source sequence, then T, then the first one
            best, best_rank = None, 99
            for seq in fields[2:]:
                m = re.match(r"\^(.+)\$\(([A-Z]+[^)]*)\)", seq)
                if not m:
                    m = re.match(r"\^(.+)\$", seq)
                    if not m:
                        continue
                    srcs = ""
                else:
                    srcs = m.group(2)
                rank = 0 if "G" in srcs else 1 if "T" in srcs else 2
                if rank < best_rank:
                    best, best_rank = m.group(1), rank
            if best is None:
                continue
            toks = tokenize_ids(best)
            if not toks or toks[0] not in OPS:
                continue
            op = toks[0]
            comps = []
            j = 1
            for _ in range(OPS[op]):
                if j >= len(toks):
                    break
                sub, j = subtree(toks, j)
                comps.append("".join(sub))
            if len(comps) == OPS[op]:
                entries[cp] = (op, comps)
    return entries


# ---------------------------------------------------------------- unf parsing

GLYPH_RE = re.compile(r"^glyph (han-([0-9a-f]{4,5})) 16 16$")


class Glyph:
    __slots__ = ("name", "cp", "file", "header_idx", "rows", "refs")

    def __init__(self, name, cp, file, header_idx, rows, refs):
        self.name = name
        self.cp = cp
        self.file = file
        self.header_idx = header_idx  # line index of the header in file lines
        self.rows = rows  # list of 16 strings, 32 chars each
        self.refs = refs  # existing ref lines following the grid (str list)


def han_file_paths(font_dir):
    return [
        os.path.join(font_dir, fn)
        for fn in sorted(os.listdir(font_dir))
        if re.fullmatch(r"han-[0-9a-f]{4,5}\.unf", fn)
    ]


def load_han_files(font_dir):
    """Return (files: {path: [lines]}, glyphs: [Glyph], incl. refactored)."""
    files, glyphs = {}, []
    for path in han_file_paths(font_dir):
        with open(path, encoding="utf-8") as f:
            lines = f.read().split("\n")
        files[path] = lines
        i = 0
        while i < len(lines):
            m = GLYPH_RE.match(lines[i])
            if m:
                rows = lines[i + 1 : i + 1 + H]
                assert len(rows) == H and all(
                    len(r) == 2 * W for r in rows
                ), f"{path}: bad grid at line {i + 1}"
                i += 1 + H
                refs = []
                while i < len(lines) and lines[i].startswith("ref "):
                    refs.append(lines[i])
                    i += 1
                glyphs.append(Glyph(m.group(1), int(m.group(2), 16), path, i - 1 - H - len(refs), rows, refs))
            else:
                i += 1
    return files, glyphs


def load_parts(parts_path):
    """parts file -> {name: (w, h, rows)}."""
    parts = {}
    if not os.path.exists(parts_path):
        return parts
    with open(parts_path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    i = 0
    while i < len(lines):
        m = re.match(r"^glyph (\S+) (\d+) (\d+)$", lines[i])
        if m:
            pw, ph = int(m.group(2)), int(m.group(3))
            parts[m.group(1)] = (pw, ph, lines[i + 1 : i + 1 + ph])
            i += 1 + ph
        else:
            i += 1
    return parts


def cell(rows, r, c):
    return rows[r][2 * c : 2 * c + 2]


def ink_cells(rows, w=W, h=H):
    return {
        (r, c) for r in range(h) for c in range(w) if cell(rows, r, c) != BLANK
    }


# ---------------------------------------------------------------- geometry

# side -> (op list, component index)
SIDE_SOURCES = {
    "l": [("⿰", 0), ("⿲", 0)],
    "r": [("⿰", 1), ("⿲", 2)],
    "t": [("⿱", 0), ("⿳", 0)],
    "b": [("⿱", 1), ("⿳", 2)],
}


def side_component(op, comps, side):
    for sop, idx in SIDE_SOURCES[side]:
        if op == sop:
            return comps[idx]
    return None


def rect_cells(side, cut):
    """The rectangular region of a cut as a set of (r, c)."""
    if side == "l":
        return {(r, c) for r in range(H) for c in range(cut)}
    if side == "r":
        return {(r, c) for r in range(H) for c in range(W - cut, W)}
    if side == "t":
        return {(r, c) for r in range(cut) for c in range(W)}
    return {(r, c) for r in range(H - cut, H) for c in range(W)}


def beyond_depth(side, cut, r, c):
    """Distance (1-based) of a cell past the cut line; <=0 means inside."""
    if side == "l":
        return c - cut + 1
    if side == "r":
        return (W - cut - 1) - c + 1
    if side == "t":
        return r - cut + 1
    return (H - cut - 1) - r + 1


NEIGHBORS8 = [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1)]


def band_cells(side, cut):
    """Cells 1..FRAG_MAX_DEPTH past the cut line."""
    return {
        (r, c)
        for r in range(H)
        for c in range(W)
        if 1 <= beyond_depth(side, cut, r, c) <= FRAG_MAX_DEPTH
    }


def region_key(rows, side, cut):
    if side == "l":
        return "|".join(rows[r][: 2 * cut] for r in range(H))
    if side == "r":
        return "|".join(rows[r][2 * (W - cut) :] for r in range(H))
    if side == "t":
        return "|".join(rows[:cut])
    return "|".join(rows[H - cut :])


def candidates_for(g, side):
    """Yield (cut, key) for plausible rectangular cuts on this side.

    The rect must end with an inked line and both it and the remainder must
    contain ink.
    """
    ink = ink_cells(g.rows)
    if not ink:
        return []
    out = []
    limit = W if side in "lr" else H
    for cut in range(2, limit - 1):
        region = rect_cells(side, cut)
        region_ink = ink & region
        rest_ink = ink - region
        if not region_ink or not rest_ink:
            continue
        # the rect's last line must carry ink (otherwise same as smaller cut)
        if not any(beyond_depth(side, cut, r, c) == 0 for (r, c) in region_ink):
            continue
        out.append((cut, region_key(g.rows, side, cut)))
    return out


FRAG_AGREE = 0.8  # fraction of a group that must agree on an absorbed cell


def group_fragment(members, side, cut):
    """Fragment absorbed by a rect group: band cells past the cut whose
    2-char code is identical (and inked) in >= FRAG_AGREE of the member
    glyphs, and which connect back to the shared rect's ink.

    Radical stroke tips that overhang the cut line satisfy this (the radical
    is drawn identically in every member while the varying bodies rarely
    agree); returns a sorted (r, c, cell) tuple. Members that don't carry
    the exact fragment must fall back to the plain rect part.
    """
    need = max(2, math.ceil(len(members) * FRAG_AGREE))
    shared = {}
    for r, c in band_cells(side, cut):
        codes = Counter(cell(g.rows, r, c) for g in members)
        code, n = codes.most_common(1)[0]
        if code != BLANK and n >= need:
            shared[(r, c)] = code
    if not shared:
        return ()
    # keep only cells 8-connected (transitively) to the rect region's ink
    region_ink = ink_cells(members[0].rows) & rect_cells(side, cut)
    keep = set()
    frontier = region_ink
    while frontier:
        frontier = {
            (r + dr, c + dc)
            for r, c in frontier
            for dr, dc in NEIGHBORS8
            if (r + dr, c + dc) in shared and (r + dr, c + dc) not in keep
        }
        keep |= frontier
    return tuple(sorted((r, c, shared[(r, c)]) for r, c in keep))


def claimed_cells(side, cut, frag):
    return rect_cells(side, cut) | {(r, c) for (r, c, _) in frag}


def part_bbox(side, cut, frag):
    """(c0, r0, w, h) of the part glyph covering rect + fragments."""
    depth = max((beyond_depth(side, cut, r, c) for (r, c, _) in frag), default=0)
    if side == "l":
        return 0, 0, cut + depth, H
    if side == "r":
        return W - cut - depth, 0, cut + depth, H
    if side == "t":
        return 0, 0, W, cut + depth
    return 0, H - cut - depth, W, cut + depth


# ---------------------------------------------------------------- mining

def mine(glyphs, ids_map, min_uses, ragged):
    """Choose part extractions.

    Returns (final_uses: Counter[part_id], chosen: {glyph name: [(side, part_id)]}).
    part_id = (side, comp, cut, region_key, frag).
    """
    groups = defaultdict(list)  # (side, comp, cut, key) -> [glyph]
    glyph_sides = []  # (glyph, side, comp, [(cut, key)])
    for g in glyphs:
        ent = ids_map.get(g.cp)
        if not ent:
            continue
        op, comps = ent
        for side in ("l", "r", "t", "b"):
            comp = side_component(op, comps, side)
            if comp is None:
                continue
            cands = candidates_for(g, side)
            if cands:
                glyph_sides.append((g, side, comp, cands))
                for cut, key in cands:
                    groups[(side, comp, cut, key)].append(g)

    # For every well-supported rect group, absorb the group-consensus
    # fragment (radical stroke tips overhanging the cut) into the part.
    # Members whose pixels don't match the consensus exactly fall back to
    # the plain rect variant.
    pid_n = {}  # part_id -> support count
    glyph_pids = defaultdict(list)  # (glyph name, side) -> [part_id]
    for gk, members in groups.items():
        if len(members) < min_uses:
            continue
        side, comp, cut, key = gk
        frag = group_fragment(members, side, cut) if ragged else ()
        if frag:
            matching = [
                g for g in members
                if all(cell(g.rows, r, c) == code for r, c, code in frag)
            ]
        else:
            matching = members
        matching_ids = {id(g) for g in matching}
        rest = [g for g in members if id(g) not in matching_ids] if frag else []
        for sub, subfrag in ((matching, frag), (rest, ())):
            if len(sub) >= min_uses:
                pid = (side, comp, cut, key, subfrag)
                pid_n[pid] = len(sub)
                for g in sub:
                    glyph_pids[(g.name, side)].append(pid)

    # Per glyph+side pick the best-supported extraction; cells * sqrt(n)
    # balances dedup volume against part fragmentation.
    chosen = defaultdict(list)
    uses = Counter()
    for g, side, comp, cands in glyph_sides:
        best = None
        for pid in glyph_pids.get((g.name, side), ()):
            _, _, cut, _, frag = pid
            cells = cut * (H if side in "lr" else W) + len(frag)
            score = (cells * math.sqrt(pid_n[pid]), cells)
            if best is None or score > best[0]:
                best = (score, pid)
        if best:
            chosen[g.name].append((side, best[1]))
            uses[best[1]] += 1

    # drop parts that ended up under-used after per-glyph choice
    ok = {pid for pid, n in uses.items() if n >= min_uses}
    for name in list(chosen):
        chosen[name] = [x for x in chosen[name] if x[1] in ok]
        if not chosen[name]:
            del chosen[name]

    # resolve overlaps between the two sides: drop the smaller extraction
    for name in list(chosen):
        picks = chosen[name]
        if len(picks) == 2:
            (s1, p1), (s2, p2) = picks
            m1 = claimed_cells(s1, p1[2], p1[4])
            m2 = claimed_cells(s2, p2[2], p2[4])
            if m1 & m2:
                chosen[name] = [picks[0] if len(m1) >= len(m2) else picks[1]]

    # recount and refilter once (dropping overlaps may sink a part below min)
    final_uses = Counter()
    for picks in chosen.values():
        for _, pid in picks:
            final_uses[pid] += 1
    ok = {pid for pid, n in final_uses.items() if n >= min_uses}
    final_uses = Counter()
    for name in list(chosen):
        chosen[name] = [x for x in chosen[name] if x[1] in ok]
        if not chosen[name]:
            del chosen[name]
        else:
            for _, pid in chosen[name]:
                final_uses[pid] += 1

    return final_uses, chosen


# ---------------------------------------------------------------- naming

def comp_slug_str(comp):
    """ASCII slug for an IDS component subtree (may contain {nn} tokens)."""
    out = []
    for tok in tokenize_ids(comp):
        if tok.startswith("{"):
            out.append("b" + tok.strip("{}"))  # BabelStone private component
        else:
            out.append(f"{ord(tok):x}")
    return "-".join(out)


def assign_names(final_uses, existing_parts):
    """part_id -> glyph name (han-<side>-<compslug>[-v2...]).

    Parts whose grid is identical to one in `existing_parts` reuse that name;
    fresh names never collide with existing ones.
    """
    existing_by_grid = {
        (w, h, tuple(rows)): name for name, (w, h, rows) in existing_parts.items()
    }
    taken = set(existing_parts)
    by_base = defaultdict(list)
    for pid in final_uses:
        side, comp, _, _, _ = pid
        by_base[(side, comp)].append(pid)
    names = {}
    for (side, comp), pids in by_base.items():
        # stable order: most-used first, then bigger first
        pids.sort(key=lambda p: (-final_uses[p], -p[2], -len(p[4]), p[3], p[4]))
        base = f"han-{side}-{comp_slug_str(comp)}"
        i = 0
        for pid in pids:
            grid = part_grid_from_pid(pid)
            key = (len(grid[0]) // 2, len(grid), tuple(grid))
            if key in existing_by_grid:
                names[pid] = existing_by_grid[key]
                continue
            while True:
                i += 1
                cand = base if i == 1 else f"{base}-v{i}"
                if cand not in taken:
                    break
            names[pid] = cand
            taken.add(cand)
    return names


# ---------------------------------------------------------------- rewriting

def part_grid_from_pid(pid):
    """Render the part glyph's pixel rows straight from a part id."""
    side, _, cut, key, frag = pid
    cellmap = {}
    for i, rowstr in enumerate(key.split("|")):
        if side == "l":
            base_r, base_c = i, 0
        elif side == "r":
            base_r, base_c = i, W - cut
        elif side == "t":
            base_r, base_c = i, 0
        else:
            base_r, base_c = H - cut + i, 0
        for j in range(len(rowstr) // 2):
            cellmap[(base_r, base_c + j)] = rowstr[2 * j : 2 * j + 2]
    for r, c, cl in frag:
        cellmap[(r, c)] = cl
    c0, r0, pw, ph = part_bbox(side, cut, frag)
    return [
        "".join(cellmap.get((r, c), BLANK) for c in range(c0, c0 + pw))
        for r in range(r0, r0 + ph)
    ]


def blank_cells(rows, cells_to_blank):
    out = list(rows)
    for r, c in cells_to_blank:
        out[r] = out[r][: 2 * c] + BLANK + out[r][2 * c + 2 :]
    return out


def rewrite_all(files, glyphs, chosen, names, final_uses, old_parts_text):
    """Apply extractions in-memory. Returns (new file lines, parts file text)."""
    new_files = {p: list(ls) for p, ls in files.items()}
    per_file = defaultdict(list)
    for g in glyphs:
        per_file[g.file].append(g)
    for path, gs in per_file.items():
        # bottom-up so earlier header indices stay valid
        for g in sorted(gs, key=lambda g: -g.header_idx):
            picks = chosen.get(g.name)
            if not picks:
                continue
            rows = list(g.rows)
            reflines = []
            for side, pid in sorted(picks):
                claimed = claimed_cells(side, pid[2], pid[4])
                claimed_ink = {rc for rc in claimed if cell(g.rows, *rc) != BLANK}
                rows = blank_cells(rows, claimed_ink)
                c0, r0, _, _ = part_bbox(side, pid[2], pid[4])
                reflines.append(f"ref {names[pid]} {c0} {r0}")
            lines = new_files[path]
            lines[g.header_idx : g.header_idx + 1 + H] = (
                [lines[g.header_idx]] + rows + reflines
            )

    side_word = {"l": "left", "r": "right", "t": "top", "b": "bottom"}
    if old_parts_text is None:
        out = [
            "////////////////////////////////////////",
            "// han: shared components extracted by scripts/han_parts.py",
            "// (guided by BabelStone IDS.TXT; do not map these)",
            "",
        ]
    else:
        out = old_parts_text.rstrip("\n").split("\n") + [""]
    already = set(load_parts_text(old_parts_text or ""))
    for pid in sorted(final_uses, key=lambda p: names[p]):
        if names[pid] in already:  # reused an existing part verbatim
            continue
        side, comp, _, _, frag = pid
        grid = part_grid_from_pid(pid)
        ragged_note = f", {len(frag)} fragment cells" if frag else ""
        out.append(f"// {comp} ({side_word[side]}, {final_uses[pid]} uses{ragged_note})")
        out.append(f"glyph {names[pid]} {len(grid[0]) // 2} {len(grid)}")
        out.extend(grid)
        out.append("")
    return new_files, "\n".join(out) + "\n"


# ---------------------------------------------------------------- verify/flatten

def compose(rows, refs, parts):
    """Overlay ref'd part grids onto rows. Asserts no ink overlap."""
    comp = [list(r) for r in rows]
    for refline in refs:
        _, pname, cs, rs = refline.split(" ")
        pw, ph, pgrid = parts[pname]
        c0, r0 = int(cs), int(rs)
        for pr in range(ph):
            for pc in range(pw):
                cl = pgrid[pr][2 * pc : 2 * pc + 2]
                if cl != BLANK:
                    tr, tc = r0 + pr, c0 + pc
                    under = "".join(comp[tr][2 * tc : 2 * tc + 2])
                    assert under == BLANK, f"overlap at {tc},{tr}"
                    comp[tr][2 * tc] = cl[0]
                    comp[tr][2 * tc + 1] = cl[1]
    return ["".join(r) for r in comp]


def verify(new_files, parts_text, orig_glyphs):
    """Re-parse rewritten lines, composite refs, compare with originals."""
    parts = load_parts_text(parts_text)
    orig = {g.name: g for g in orig_glyphs}
    n_checked = 0
    for path, lines in new_files.items():
        i = 0
        while i < len(lines):
            m = GLYPH_RE.match(lines[i])
            if not m:
                i += 1
                continue
            name = m.group(1)
            rows = lines[i + 1 : i + 1 + H]
            i += 1 + H
            refs = []
            while i < len(lines) and lines[i].startswith("ref "):
                refs.append(lines[i])
                i += 1
            if name not in orig:  # already refactored before this run
                continue
            got = compose(rows, refs, parts)
            if got != orig[name].rows:
                raise AssertionError(f"{name}: mismatch after recomposition")
            n_checked += 1
    return n_checked


def load_parts_text(text):
    parts = {}
    lines = text.split("\n")
    i = 0
    while i < len(lines):
        m = re.match(r"^glyph (\S+) (\d+) (\d+)$", lines[i])
        if m:
            pw, ph = int(m.group(2)), int(m.group(3))
            parts[m.group(1)] = (pw, ph, lines[i + 1 : i + 1 + ph])
            i += 1 + ph
        else:
            i += 1
    return parts


def flatten(font_dir):
    """Undo refactoring: recompose every ref'd glyph and drop the ref lines."""
    parts_path = os.path.join(font_dir, "han-parts.unf")
    parts = load_parts(parts_path)
    files, glyphs = load_han_files(font_dir)
    n = 0
    for path in files:
        lines = files[path]
        gs = [g for g in glyphs if g.file == path and g.refs]
        for g in sorted(gs, key=lambda g: -g.header_idx):
            composed = compose(g.rows, g.refs, parts)
            lines[g.header_idx : g.header_idx + 1 + H + len(g.refs)] = (
                [lines[g.header_idx]] + composed
            )
            n += 1
        with open(path, "w", encoding="utf-8") as f:
            f.write("\n".join(lines))
    if os.path.exists(parts_path):
        os.remove(parts_path)
    print(f"flattened {n} glyphs; removed {os.path.relpath(parts_path, ROOT)}")


# ---------------------------------------------------------------- audit

def audit(font_dir):
    """Report residual ink fragments hugging a part's cut line."""
    parts = load_parts(os.path.join(font_dir, "han-parts.unf"))
    _, glyphs = load_han_files(font_dir)
    total_frag_cells = 0
    flagged = []
    by_part = Counter()
    for g in glyphs:
        if not g.refs:
            continue
        ink = ink_cells(g.rows)
        if not ink:
            continue
        for refline in g.refs:
            _, pname, cs, rs = refline.split(" ")
            pw, ph, pgrid = parts[pname]
            c0, r0 = int(cs), int(rs)
            # part ink in glyph coordinates
            pink = {
                (r0 + r, c0 + c)
                for r in range(ph)
                for c in range(pw)
                if pgrid[r][2 * c : 2 * c + 2] != BLANK
            }
            # infer side + cut line from geometry
            if ph == H and pw < W:
                side = "l" if c0 == 0 else "r"
                cut = pw if side == "l" else W - c0  # bbox size incl. fragments
            elif pw == W and ph < H:
                side = "t" if r0 == 0 else "b"
                cut = ph if side == "t" else H - r0
            else:
                continue
            region = rect_cells(side, cut)
            frags = set()
            # residual components adjacent to part ink, small + shallow
            outside = ink - region
            seeds = {
                (r, c)
                for (r, c) in outside
                if beyond_depth(side, cut, r, c) <= FRAG_MAX_DEPTH
                and any((r + dr, c + dc) in pink for dr, dc in NEIGHBORS8)
            }
            visited = set()
            for seed in seeds:
                if seed in visited:
                    continue
                comp = {seed}
                queue = [seed]
                visited.add(seed)
                ok = True
                while queue:
                    r, c = queue.pop()
                    if beyond_depth(side, cut, r, c) > FRAG_MAX_DEPTH:
                        ok = False
                    for dr, dc in NEIGHBORS8:
                        nb = (r + dr, c + dc)
                        if nb in outside and nb not in visited:
                            visited.add(nb)
                            comp.add(nb)
                            queue.append(nb)
                    if len(comp) > FRAG_MAX_CELLS:
                        ok = False
                if ok:
                    frags |= comp
            if frags:
                flagged.append((g.name, pname, len(frags)))
                by_part[pname] += 1
                total_frag_cells += len(frags)
    print(f"{len(flagged)} glyph/part pairs with suspected leftover fragments "
          f"({total_frag_cells} cells) out of "
          f"{sum(1 for g in glyphs if g.refs)} refactored glyphs")
    for pname, n in by_part.most_common(20):
        print(f"  {pname:30s} {n} glyphs")
    return flagged


# ---------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["stats", "apply", "audit", "flatten"])
    ap.add_argument("--min-uses", type=int, default=4)
    ap.add_argument("--no-ragged", action="store_true",
                    help="rectangular cuts only (no fragment absorption)")
    ap.add_argument("--frag-agree", type=float, default=FRAG_AGREE)
    ap.add_argument("--frag-depth", type=int, default=FRAG_MAX_DEPTH)
    ap.add_argument("--font-dir", default=os.path.join(ROOT, "font"))
    args = ap.parse_args()
    globals()["FRAG_AGREE"] = args.frag_agree
    globals()["FRAG_MAX_DEPTH"] = args.frag_depth

    if args.mode == "flatten":
        flatten(args.font_dir)
        return
    if args.mode == "audit":
        audit(args.font_dir)
        return

    ids_map = parse_ids_file(IDS_PATH)
    files, glyphs = load_han_files(args.font_dir)
    fresh = [g for g in glyphs if not g.refs]  # skip already-refactored ones
    print(f"{len(fresh)} unrefactored glyphs (of {len(glyphs)}) in {len(files)} "
          f"files; IDS for {sum(1 for g in fresh if g.cp in ids_map)} of them")

    parts_path = os.path.join(args.font_dir, "han-parts.unf")
    old_parts_text = None
    if os.path.exists(parts_path):
        with open(parts_path, encoding="utf-8") as f:
            old_parts_text = f.read()

    final_uses, chosen = mine(fresh, ids_map, args.min_uses, not args.no_ragged)
    names = assign_names(final_uses, load_parts_text(old_parts_text or ""))

    total_cells = sum(
        pid[2] * (H if side in "lr" else W) + len(pid[4])
        for picks in chosen.values()
        for side, pid in picks
    )
    n_ragged = sum(1 for pid in final_uses if pid[4])
    print(f"parts: {len(final_uses)} ({n_ragged} ragged)  |  glyphs refactored: "
          f"{len(chosen)} ({100 * len(chosen) / max(1, len(fresh)):.1f}%)  |  "
          f"cells deduped: {total_cells}")

    if args.mode == "stats":
        top = sorted(final_uses.items(), key=lambda kv: -kv[1])[:30]
        for pid, n in top:
            side, comp, cut, _, frag = pid
            fr = f"+{len(frag)}px" if frag else ""
            print(f"  {names[pid]:30s} {comp:8s} side={side} size={cut:2d}{fr:6s} uses={n}")
        return

    new_files, parts_text = rewrite_all(files, fresh, chosen, names, final_uses,
                                        old_parts_text)
    n = verify(new_files, parts_text, fresh)
    print(f"verified {n} glyphs recomposite to original bitmaps")
    for path, lines in new_files.items():
        with open(path, "w", encoding="utf-8") as f:
            f.write("\n".join(lines))
    with open(parts_path, "w", encoding="utf-8") as f:
        f.write(parts_text)
    print(f"wrote {len(new_files)} files + {os.path.relpath(parts_path, ROOT)}")


if __name__ == "__main__":
    main()
