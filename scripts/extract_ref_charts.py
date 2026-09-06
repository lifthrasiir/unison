#!/usr/bin/env python3
"""Build `data/ref/<prefix>/<codepoint>.png` reference strips out of the PDF charts.

For every han code point the charts cover, this writes one PNG that puts side by
side every drawing the sources show for it:

  * the per-source glyphs of the Unicode code charts — the G/H/T/J/K/KP/V/M/S/U
    columns of the IRG blocks (URO, every extension, both compatibility blocks),
    each cell labelled with the source reference the chart carries (`G0-4752`,
    `T1-4562`, `KP1-3413`, `JMJ-030338`, …), and
  * the representative glyphs of the six IVD collections, each cell labelled
    with its variation selector (`VS17`, i.e. `E0100`) and the IVD sequence
    identifier (`AJ1 CID+13698`, `HD IA0102`, `MJ MJ000008`, …).

Two collections registering the same identifier for one sequence does **not**
fold into one cell: UTS #37 does not guarantee that two collections show the
same representative glyph, so Hanyo-Denshi and Moji_Joho — which overlap almost
entirely — each keep their own cell.

The charts themselves are downloaded on first run (about 270 MB) into
`data/ref/` and checked against the checksums in `MANIFEST` below; nothing is
read until its checksum matches.  Only PyMuPDF and Pillow are needed — no
Ghostscript, no platform-specific tooling:

    python3 -m venv .venv
    .venv/bin/pip install pymupdf pillow
    .venv/bin/python scripts/extract_ref_charts.py

The charts and everything derived from them are copyrighted, so they stay out of
the repository: `data/ref/` is ignored in full and is regenerated locally.

# Updating to a new release

`MANIFEST` is the only thing to edit; it pins one archival code chart and one
dated IVD release, and each entry carries the SHA-256 the download must have.

  1. Point the entry's URL at the new release — the code charts move with the
     Unicode version (`.../Public/<version>/charts/CodeCharts.pdf`), the IVD
     files with the IVD date (`.../ivd/data/<date>/IVD_Charts_<collection>.pdf`).
  2. Run `--checksums`, which downloads whatever is missing *without* checking
     (it prints a warning), hashes every file and writes the `MANIFEST` rows on
     stdout.  Paste them in.  Do this only when the release really did change:
     the checksum is what makes a run reproducible, so never quiet a mismatch by
     copying the hash of a file you have not accounted for.
  3. Run with `--rescan --force`: the cached chart index is keyed by file
     identity, but the images already written are not.

A **new IVD collection** takes two lines: its `MANIFEST` entry and its short
label in `COLLECTION_ABBREV`, which is what the cells are labelled with.  A
collection with no entry there is labelled by its full name, which only makes
the strip wider.

# How a chart is read

Both chart families lay a row out the same way, and both are read by the same
two rules:

  * A **row** starts at a code point label — `Arial` 10 pt in the code charts,
    `LucidaSansTypewriterStd` 9.5 pt in the IVD ones — and owns every label
    below it down to the next one.  A row that wraps (a code point with more
    sources or more variation sequences than fit one line) needs no special
    case: the extra line carries no code point label of its own, so its cells
    fall to the same owner.  A row wrapping across a *page* break is the one
    case that does: the continuation's page opens with no owner above it, so the
    scan carries the previous page's last row over.
  * A **cell** is found from its *label*, not from the drawing: the source
    reference (`UnihanInfo` 6 pt) or the variation selector
    (`LucidaSansTypewriterStd` 5 pt) is matched to the drawing directly above
    it.  The drawings themselves carry no usable text — the chart fonts are
    subset with private mappings — but their boxes are all that is wanted.

Only pages carrying source references are read, which is what makes scanning one
3156-page archival chart affordable: the ordinary chart layout (a code point
grid and a name list, which is what every block set that way — Kangxi Radicals,
Hangul, everything non-ideographic — uses) typesets no `UnihanInfo` at all, so
those pages are skipped whole and contribute nothing.  What is read is therefore
decided by the *layout*, not by a list of blocks: besides the IRG blocks that
answers the Tangut and Nushu charts too, which are set the same way and come out
right for free (`L2008-1940`, `N1966-039-096`).

The two families differ only in what a drawing *is*: the code charts typeset one
at 20–21 pt from an embedded font, so the em box is cut out of a rendering of
the page (the box is derived from the baseline, which is what makes cells from
different chart fonts line up); the IVD charts place a 128x128 PNG per glyph,
which is taken out of the file as it is.

Columns: the code charts put two to four columns of rows on a page.  The column
boundaries are not assumed — the code point labels' x positions are clustered
per page, and each label falls to the rightmost column that starts at or left of
it.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import os
import pickle
import re
import sys
import urllib.request
from collections import defaultdict
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path

try:
    import pymupdf
except ImportError:  # PyMuPDF < 1.24 only ships the `fitz` name
    import fitz as pymupdf

from PIL import Image, ImageDraw, ImageFont

# ---------------------------------------------------------------------------
# the charts


@dataclass(frozen=True)
class ChartSource:
    name: str  # the file name under the output root
    url: str
    sha256: str
    kind: str  # "unicode" | "ivd"
    collection: str = ""  # IVD only


UNICODE_VERSION = "17.0.0"
IVD_VERSION = "2026-08-03"
_CHARTS = f"https://www.unicode.org/Public/{UNICODE_VERSION}/charts"
_IVD = f"https://www.unicode.org/ivd/data/{IVD_VERSION}"

# Edit this to move to a new release; `--checksums` prints these rows.
MANIFEST = [
    ChartSource(
        "CodeCharts.pdf",
        f"{_CHARTS}/CodeCharts.pdf",
        "51cf23dc65420012bd090003b3ad4d7f89b7c3ab9c34e510969055639c53d727",
        "unicode",
    ),
    ChartSource(
        "IVD_Charts_Adobe-Japan1.pdf",
        f"{_IVD}/IVD_Charts_Adobe-Japan1.pdf",
        "7713e272075b0d7e377fd71017dbe3df8adc9f835498f635f62fd6e21c44e17a",
        "ivd",
        "Adobe-Japan1",
    ),
    ChartSource(
        "IVD_Charts_CAAPH.pdf",
        f"{_IVD}/IVD_Charts_CAAPH.pdf",
        "77ae3cf3724dda5a8d0cac89d21094e4a29a8bbef7b1d6f886c0f639190e0a4c",
        "ivd",
        "CAAPH",
    ),
    ChartSource(
        "IVD_Charts_Hanyo-Denshi.pdf",
        f"{_IVD}/IVD_Charts_Hanyo-Denshi.pdf",
        "b2bcc48560b67cf7ea861cf6273208fbf7331c32a91192deb688b673d7b0eb33",
        "ivd",
        "Hanyo-Denshi",
    ),
    ChartSource(
        "IVD_Charts_KRName.pdf",
        f"{_IVD}/IVD_Charts_KRName.pdf",
        "e064dc87599cf3f84d94a30e5677a45e1e60cf35c6938c7f1482f68ed53f2f1f",
        "ivd",
        "KRName",
    ),
    ChartSource(
        "IVD_Charts_MSARG.pdf",
        f"{_IVD}/IVD_Charts_MSARG.pdf",
        "016444ab81317e84427fb24bad606adb3ad8b103f6da73b7371a94ac1702033b",
        "ivd",
        "MSARG",
    ),
    ChartSource(
        "IVD_Charts_Moji_Joho.pdf",
        f"{_IVD}/IVD_Charts_Moji_Joho.pdf",
        "d7a068800ac72eb48f768287267d85ad6830a4676355edfaeb143ec01816ad6c",
        "ivd",
        "Moji_Joho",
    ),
]

# What a cell says a collection is.  A collection missing here is labelled by
# its full name.
COLLECTION_ABBREV = {
    "Adobe-Japan1": "AJ1",
    "CAAPH": "CA",
    "Hanyo-Denshi": "HD",
    "KRName": "KR",
    "MSARG": "MS",
    "Moji_Joho": "MJ",
}

# ---------------------------------------------------------------------------
# chart geometry

# The label fonts.  Everything the charts say about a cell is typeset in one of
# these, and nothing else in the charts is.
UNIHAN_LABEL_FONT = "UnihanInfo"
IVD_LABEL_FONT = "LucidaSansTypewriterStd"

CODEPOINT_RE = re.compile(r"^[0-9A-F]{4,6}$")
# `G0-4752`, `GKX-0078.15`, `HB1-A542`, `J14-2130`, `JMJ-056827`, `MD-4E17`,
# `GIDC23-221` (Extension I), and the two-dash references the Tangut and Nushu
# charts use (`L2008-0042-4537`, `N1966-039-096`, `H2004-B-0284`).  The leading
# letter is what keeps a radical-stroke annotation out: those are digits, and a
# negative stroke count (`47.-1`) has a dash of its own.
SOURCE_ID_RE = re.compile(r"^[A-Z][0-9A-Z]{0,7}-[0-9A-Za-z.-]+$")
VARIATION_SELECTOR_RE = re.compile(r"^E01[0-9A-F]{2}$")

# A drawn glyph in the code charts is set at 20–21 pt; the radical-stroke
# annotations beside it are at 6 pt.
MIN_GLYPH_PT = 14.0
# A span whose box is much narrower than its point size is the side bearing the
# charts typeset as a run of its own, not a glyph.
MIN_GLYPH_WIDTH_RATIO = 0.7
# The em box of a chart glyph, relative to its baseline and point size.  The CJK
# fonts the charts embed all sit on a 0.88/0.12 em, so cutting the box out this
# way (rather than by the span box, which is the font's own ascent and descent)
# is what makes two cells from two different chart fonts line up.
EM_ASCENT = 0.88
EM_DESCENT = 0.12
# A row's cells are the labels within this many points below the drawing.
LABEL_GAP_PT = 9.0
# … and no further than this from its horizontal centre.
LABEL_CENTRE_SLACK_PT = 14.0
# Column clustering: two code point labels closer than this start the same
# column.  The charts' columns are >100 pt apart and a label is ~30 pt wide.
COLUMN_SLACK_PT = 40.0
# Everything above this is the running head, not a row.
BODY_TOP_PT = 60.0
# How far back a page chunk looks for the row a wrapped line continues.
CARRY_LOOKBACK_PAGES = 3
# Pages per scanning task.
SCAN_CHUNK_PAGES = 64

# ---------------------------------------------------------------------------
# output geometry

CELL_PAD = 3
LABEL_LEADING = 1
FRAME_GRAY = 210
SEPARATOR_GRAY = 160
# Chart pages are rendered this much larger than the cell and downsampled, so a
# 20 pt glyph landing in a 64 px cell is still clean.
SUPERSAMPLE = 2.0
# Code points per rendering task.
RENDER_CHUNK = 256


@dataclass(slots=True)
class Cell:
    """One drawing to put in the strip, as the index remembers it.

    `key` is a page-relative box: the em box to cut out of a rendered chart
    page, or the box of the embedded image in an IVD chart.
    """

    source: int  # index into the source list, i.e. which PDF
    page: int
    key: tuple[float, float, float, float]
    labels: tuple[str, ...]
    order: tuple  # sort key within one code point


@dataclass
class Index:
    stamp: str
    cells: dict[int, list[Cell]] = field(default_factory=dict)


# ---------------------------------------------------------------------------
# downloading


def sha256_of(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def fetch(src: ChartSource, root: Path, verify: bool = True) -> Path:
    """The chart file, downloaded if need be and checked against `MANIFEST`.

    A download lands under a `.download` name and is renamed only once its
    checksum matches, so an interrupted or corrupted transfer can never be
    mistaken for a chart.
    """
    path = root / src.name
    if not path.exists():
        tmp = path.with_suffix(path.suffix + ".download")
        print(f"downloading {src.name} from {src.url}", file=sys.stderr)
        request = urllib.request.Request(
            src.url, headers={"User-Agent": "unison-extract-ref-charts/1"}
        )
        with urllib.request.urlopen(request) as response, open(tmp, "wb") as out:
            while block := response.read(1 << 20):
                out.write(block)
        got = sha256_of(tmp)
        if verify and got != src.sha256:
            tmp.unlink()
            raise SystemExit(
                f"{src.name}: checksum mismatch\n"
                f"  expected {src.sha256}\n  got      {got}\n"
                f"The release may have been updated; see 'Updating to a new "
                f"release' in this script's docstring."
            )
        tmp.replace(path)
    elif verify:
        got = sha256_of(path)
        if got != src.sha256:
            raise SystemExit(
                f"{path}: checksum mismatch\n"
                f"  expected {src.sha256}\n  got      {got}\n"
                f"Delete the file to fetch it again, or update MANIFEST."
            )
    return path


# ---------------------------------------------------------------------------
# scanning


def _spans(page):
    for block in page.get_text("dict")["blocks"]:
        if block["type"] != 0:
            continue
        for line in block["lines"]:
            for span in line["spans"]:
                yield span


def _image_blocks(page):
    for block in page.get_text("dict")["blocks"]:
        if block["type"] == 1:
            yield block


def _join_wrapped(labels):
    """Put a source reference the chart broke over two lines back together.

    A long reference (`GKX-0078.14`) is set as `GKX-` and `0078.14` on two
    lines; the first line's position is what places the cell, so the second is
    folded into it and dropped.
    """
    labels = sorted(labels, key=lambda lb: (lb[1], lb[0]))
    out, used = [], set()
    for i, (centre, y0, text, x0) in enumerate(labels):
        if i in used:
            continue
        while text.endswith("-"):
            for j in range(i + 1, len(labels)):
                nc, ny0, ntext, _ = labels[j]
                if ny0 <= y0 or ny0 - y0 > 10.0 or j in used:
                    continue
                if abs(nc - centre) <= LABEL_CENTRE_SLACK_PT:
                    text += ntext
                    used.add(j)
                    break
            else:
                break
        out.append((centre, y0, text, x0))
    return out


def _traced_glyphs(page):
    """Every drawn chart glyph on the page, as (x0, baseline, point size).

    Read off the show-text operations rather than the extracted text: a chart
    font maps most of its glyphs to nothing (or to U+FFFD), and a run of those
    is dropped from the text layer entirely — `20244` and 225 other Extension B
    code points would otherwise have no drawing at all.
    """
    glyphs = []
    for run in page.get_texttrace():
        size = run["size"]
        if size < MIN_GLYPH_PT or run["type"] != 0:
            continue
        if run["font"].startswith((UNIHAN_LABEL_FONT, "Arial", "MyriadPro")):
            continue
        for _code, _gid, origin, bbox in run["chars"]:
            if bbox[1] < BODY_TOP_PT:
                continue
            if (bbox[2] - bbox[0]) < size * MIN_GLYPH_WIDTH_RATIO:
                continue  # the side bearing the charts set as a run of its own
            glyphs.append((origin[0], origin[1], size))
    return glyphs


def _columns(labels):
    """Cluster code point labels into columns, left to right."""
    starts: list[float] = []
    for x0 in sorted(x for x, _, _ in labels):
        if not starts or x0 - starts[-1] > COLUMN_SLACK_PT:
            starts.append(x0)
    return starts


def _column_of(starts, x0):
    which = 0
    for i, start in enumerate(starts):
        if x0 >= start - COLUMN_SLACK_PT / 2:
            which = i
    return which


def _owner(rows, column, y, carried):
    """The code point whose row a label at `y` in `column` belongs to.

    `carried` is the last code point of the previous page, which is the answer
    when a row wraps across a page break: the continuation carries no code point
    label of its own, so there is nothing above the label on its own page.
    """
    best = None
    for x0, y0, cp in rows:
        if x0 != column or y0 > y:
            continue
        if best is None or y0 > best[0]:
            best = (y0, cp)
    if best is not None:
        return best[1]
    for x0, y0, cp in sorted(rows, key=lambda r: (r[0], r[1])):
        if x0 < column:
            best = (y0, cp)  # the column to the left of a wrapped first column
    return best[1] if best is not None else carried


def _last_row(rows, carried):
    """The code point the page ends on, in reading order."""
    if not rows:
        return carried
    return max(rows, key=lambda r: (r[0], r[1]))[2]


def _rows_and_labels(page, font_prefix, label_ok, size_min=0.0):
    """The page's code point labels and its cell labels, unplaced."""
    rows, labels = [], []
    for span in _spans(page):
        x0, y0, x1, y1 = span["bbox"]
        if y0 < BODY_TOP_PT:
            continue
        text = span["text"].strip()
        if not text:
            continue
        if span["font"].startswith(font_prefix) and span["size"] > size_min:
            if CODEPOINT_RE.match(text):
                rows.append((x0, y0, int(text, 16)))
                continue
        if label_ok(span, text):
            labels.append(((x0 + x1) / 2, y0, text, x0))
    return rows, labels


def _placed(rows):
    starts = _columns(rows) or [0.0]
    return starts, [(_column_of(starts, x0), y0, cp) for x0, y0, cp in rows]


def _carried_into(doc, first, scan):
    """The code point a page chunk starting at `first` may be continuing."""
    for pno in range(first - 1, max(-1, first - 1 - CARRY_LOOKBACK_PAGES), -1):
        rows, labels = scan(doc[pno])
        if not labels:
            continue  # a page with no cell labels cannot be continued from
        _, placed = _placed(rows)
        return _last_row(placed, None)
    return None


def _unicode_page(page):
    return _rows_and_labels(
        page,
        "Arial",
        lambda span, text: span["font"].startswith(UNIHAN_LABEL_FONT),
    )


def scan_unicode_chart(path: Path, source: int, first: int, last: int):
    """Every per-source glyph the code chart draws on pages [`first`, `last`)."""
    cells: dict[int, list[Cell]] = defaultdict(list)
    with pymupdf.open(path) as doc:
        carried = _carried_into(doc, first, _unicode_page)
        for pno in range(first, min(last, doc.page_count)):
            page = doc[pno]
            raw_rows, raw_labels = _unicode_page(page)
            labels = [
                lb for lb in _join_wrapped(raw_labels) if SOURCE_ID_RE.match(lb[2])
            ]
            if not labels:
                continue  # an ordinary chart page: a grid and a name list
            starts, rows = _placed(raw_rows)
            glyphs = _traced_glyphs(page)

            for centre, y0, text, lx0 in labels:
                best, best_gap = None, None
                for gx0, baseline, size in glyphs:
                    top = baseline - EM_ASCENT * size
                    bottom = baseline + EM_DESCENT * size
                    gap = y0 - bottom
                    if not (-3.0 <= gap <= LABEL_GAP_PT):
                        continue
                    if abs((gx0 + size / 2) - centre) > LABEL_CENTRE_SLACK_PT:
                        continue
                    if best_gap is None or gap < best_gap:
                        best, best_gap = (gx0, top, size, bottom), gap
                if best is None:
                    continue
                gx0, top, size, bottom = best
                cp = _owner(rows, _column_of(starts, lx0), y0, carried)
                if cp is None:
                    continue
                cells[cp].append(
                    Cell(
                        source=source,
                        page=pno,
                        key=(gx0, top, gx0 + size, bottom),
                        labels=(text,),
                        order=(pno, round(y0, 1), round(gx0, 1)),
                    )
                )
            carried = _last_row(rows, carried)
    return cells


def _ivd_page(page):
    return _rows_and_labels(
        page,
        IVD_LABEL_FONT,
        lambda span, text: span["font"].startswith(IVD_LABEL_FONT)
        and span["size"] < 7.0,
        size_min=7.0,
    )


def scan_ivd_chart(path: Path, source: int, collection: str, first: int, last: int):
    """Every representative glyph the IVD chart shows on pages [`first`, `last`)."""
    abbrev = COLLECTION_ABBREV.get(collection, collection)
    cells: dict[int, list[Cell]] = defaultdict(list)
    with pymupdf.open(path) as doc:
        carried = _carried_into(doc, first, _ivd_page)
        for pno in range(first, min(last, doc.page_count)):
            page = doc[pno]
            raw_rows, raw_labels = _ivd_page(page)
            selectors = [lb for lb in raw_labels if VARIATION_SELECTOR_RE.match(lb[2])]
            idents = [lb for lb in raw_labels if lb[2] != collection]
            if not selectors:
                continue
            starts, rows = _placed(raw_rows)
            images = [b["bbox"] for b in _image_blocks(page)]

            for centre, y0, vs, lx0 in selectors:
                box = None
                for bbox in images:
                    if abs((bbox[0] + bbox[2]) / 2 - centre) > LABEL_CENTRE_SLACK_PT:
                        continue
                    if not (-LABEL_GAP_PT <= y0 - bbox[3] <= 3 * LABEL_GAP_PT):
                        continue
                    box = bbox
                    break
                if box is None:
                    continue
                cp = _owner(rows, _column_of(starts, lx0), y0, carried)
                if cp is None:
                    continue
                # The identifier is the label two lines under the selector; the
                # collection name in between is the same for the whole file.
                ident, ident_y = "", 0.0
                for icentre, iy0, text, _ in idents:
                    if abs(icentre - centre) > LABEL_CENTRE_SLACK_PT or iy0 <= y0:
                        continue
                    if not ident or iy0 < ident_y:
                        ident, ident_y = text, iy0
                vs_num = int(vs, 16) - 0xE0100 + 17
                cells[cp].append(
                    Cell(
                        source=source,
                        page=pno,
                        key=tuple(box),
                        labels=(f"VS{vs_num} {vs}", f"{abbrev} {ident}".strip()),
                        order=(vs, ident),
                    )
                )
            carried = _last_row(rows, carried)
    return cells


def scan_all(sources: list[ChartSource], paths: list[Path], jobs: int, stamp: str):
    """Scan every chart, a chunk of pages at a time."""
    merged: dict[int, list[Cell]] = defaultdict(list)
    counts: dict[int, int] = defaultdict(int)
    tasks = []
    for i, (src, path) in enumerate(zip(sources, paths)):
        with pymupdf.open(path) as doc:
            pages = doc.page_count
        for first in range(0, pages, SCAN_CHUNK_PAGES):
            last = first + SCAN_CHUNK_PAGES
            if src.kind == "unicode":
                tasks.append((i, (scan_unicode_chart, (path, i, first, last))))
            else:
                tasks.append(
                    (i, (scan_ivd_chart, (path, i, src.collection, first, last)))
                )
    with ProcessPoolExecutor(max_workers=jobs) as pool:
        futures = [(i, pool.submit(fn, *args)) for i, (fn, args) in tasks]
        for i, future in futures:
            for cp, cells in future.result().items():
                merged[cp].extend(cells)
                counts[i] += len(cells)
    for i, src in enumerate(sources):
        print(f"  {src.name}: {counts[i]} glyphs", file=sys.stderr)
    kinds = [src.kind for src in sources]

    def order(cell: Cell):
        """The chart's own columns first, then the IVD cells by sequence.

        A code point's IVD cells read as the variation sequences do — by
        selector, and within one selector by collection, which is `MANIFEST`
        order — rather than one collection's whole run after another's, so the
        several collections registering the same selector stand side by side.
        """
        if kinds[cell.source] == "unicode":
            return (0, cell.source) + tuple(cell.order)
        return (1, cell.order[0], cell.source) + tuple(cell.order[1:])

    for cells in merged.values():
        cells.sort(key=order)
    return Index(stamp=stamp, cells=dict(merged))


# ---------------------------------------------------------------------------
# rendering

_STATE: dict = {}


def _page_image(source: int, pno: int, zoom: float):
    """A rendered chart page, kept for as long as the next cells may want it."""
    cache = _STATE.setdefault("pages", {})
    hit = cache.get(source)
    if hit is not None and hit[0] == pno:
        return hit[1]
    doc = _STATE["docs"][source]
    pix = doc[pno].get_pixmap(matrix=pymupdf.Matrix(zoom, zoom), colorspace="gray")
    img = Image.frombytes("L", (pix.width, pix.height), pix.samples)
    cache[source] = (pno, img)
    return img


def _cell_image(cell: Cell, size: int) -> Image.Image:
    src = _STATE["sources"][cell.source]
    if src.kind == "unicode":
        zoom = SUPERSAMPLE * size / 21.0
        page = _page_image(cell.source, cell.page, zoom)
        x0, y0, x1, y1 = (v * zoom for v in cell.key)
        crop = page.crop((round(x0), round(y0), round(x1), round(y1)))
    else:
        doc = _STATE["docs"][cell.source]
        blocks = _STATE.setdefault("ivd", {})
        hit = blocks.get(cell.source)
        if hit is None or hit[0] != cell.page:
            found = {
                tuple(round(v, 1) for v in b["bbox"]): (b["image"], b.get("mask"))
                for b in _image_blocks(doc[cell.page])
            }
            hit = (cell.page, found)
            blocks[cell.source] = hit
        raw = hit[1].get(tuple(round(v, 1) for v in cell.key))
        if raw is None:
            return Image.new("L", (size, size), 255)
        crop = Image.open(io.BytesIO(raw[0])).convert("L")
        if raw[1] is not None:
            # Moji_Joho draws every glyph as a black rectangle behind a soft
            # mask, so the drawing is in the mask and the image alone is a solid
            # black cell.  Compositing over white is what the two collections
            # that carry no mask amount to anyway.
            alpha = Image.open(io.BytesIO(raw[1])).convert("L")
            if alpha.size != crop.size:
                alpha = alpha.resize(crop.size, Image.LANCZOS)
            crop = Image.composite(crop, Image.new("L", crop.size, 255), alpha)
    return crop.resize((size, size), Image.LANCZOS)


def compose(cp: int, cells: list[Cell], size: int, font) -> Image.Image:
    line_h = font.getbbox("Ag")[3] + LABEL_LEADING
    label_rows = max((len(c.labels) for c in cells), default=1)
    height = CELL_PAD + size + CELL_PAD + label_rows * line_h + CELL_PAD

    drawn = []
    for cell in cells:
        img = _cell_image(cell, size)
        width = size
        for text in cell.labels:
            width = max(width, font.getbbox(text)[2])
        drawn.append((img, cell.labels, int(width) + 2 * CELL_PAD))

    head = f"U+{cp:04X}"
    head_w = int(font.getbbox(head)[2]) + 2 * CELL_PAD
    total = head_w + sum(w for _, _, w in drawn)
    out = Image.new("L", (total, height), 255)
    draw = ImageDraw.Draw(out)
    draw.text((CELL_PAD, (height - line_h) // 2), head, font=font, fill=0)

    x = head_w
    for img, labels, width in drawn:
        draw.line([(x, 0), (x, height - 1)], fill=SEPARATOR_GRAY)
        gx = x + (width - size) // 2
        draw.rectangle(
            [gx - 1, CELL_PAD - 1, gx + size, CELL_PAD + size], outline=FRAME_GRAY
        )
        out.paste(img, (gx, CELL_PAD))
        ty = CELL_PAD + size + CELL_PAD
        for text in labels:
            tw = font.getbbox(text)[2]
            draw.text((x + (width - tw) // 2, ty), text, font=font, fill=0)
            ty += line_h
        x += width
    return out


def out_path(root: Path, cp: int) -> Path:
    name = f"{cp:04x}"
    return root / name[:-3] / f"{name}.png"


def _init_worker(sources: list[ChartSource], paths: list[Path]):
    _STATE["sources"] = sources
    _STATE["docs"] = {i: pymupdf.open(p) for i, p in enumerate(paths)}


def _label_font():
    try:  # Pillow >= 10.1 scales its bundled default font
        return ImageFont.load_default(size=11)
    except TypeError:
        return ImageFont.load_default()


def _render_chunk(items, root: Path, size: int, force: bool) -> int:
    font = _label_font()
    made = 0
    for cp, cells in items:
        path = out_path(root, cp)
        if not force and path.exists():
            continue
        img = compose(cp, cells, size, font)
        path.parent.mkdir(parents=True, exist_ok=True)
        img.save(path, optimize=True)
        made += 1
    return made


# ---------------------------------------------------------------------------
# driver


def parse_ranges(spec: str) -> list[tuple[int, int]]:
    out = []
    for part in spec.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            lo, hi = part.split("-", 1)
            out.append((int(lo, 16), int(hi, 16)))
        else:
            out.append((int(part, 16), int(part, 16)))
    return out


def print_checksums(root: Path) -> int:
    """The `MANIFEST` rows for whatever is on disk, to paste after a release."""
    for src in MANIFEST:
        path = root / src.name
        if not path.exists():
            print(f"# {src.name} is missing; fetching it UNVERIFIED", file=sys.stderr)
            fetch(src, root, verify=False)
        got = sha256_of(path)
        mark = "" if got == src.sha256 else "  # CHANGED"
        print(f'        "{got}",{mark}  # {src.name}')
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument(
        "--root",
        type=Path,
        default=Path("data/ref"),
        help="where the charts are kept and the images are written",
    )
    ap.add_argument("--size", type=int, default=64, help="glyph cell size in pixels")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--range", dest="ranges", default=None, help="e.g. 4E00-4EFF,F900")
    ap.add_argument("--force", action="store_true", help="rewrite existing PNGs")
    ap.add_argument("--rescan", action="store_true", help="ignore the cached index")
    ap.add_argument(
        "--checksums",
        action="store_true",
        help="print the MANIFEST checksum rows for the files on disk and stop",
    )
    args = ap.parse_args()

    root = args.root
    root.mkdir(parents=True, exist_ok=True)
    if args.checksums:
        return print_checksums(root)

    paths = [fetch(src, root) for src in MANIFEST]
    for src, path in zip(MANIFEST, paths):
        print(f"chart: {path} ({src.kind} {src.collection})".rstrip(), file=sys.stderr)

    cache_dir = root / ".cache"
    cache_dir.mkdir(parents=True, exist_ok=True)
    index_path = cache_dir / "index.pkl"
    # The index is keyed by what produced it: the charts *and* this script, so
    # that a change to how a chart is read is not read out of a stale cache.
    stamp = "\n".join(
        [sha256_of(Path(__file__))] + [f"{s.name} {s.sha256}" for s in MANIFEST]
    )
    index = None
    if not args.rescan and index_path.exists():
        with open(index_path, "rb") as f:
            cached = pickle.load(f)
        if getattr(cached, "stamp", None) == stamp:
            print("using the cached chart index", file=sys.stderr)
            index = cached
    if index is None:
        print("scanning the charts…", file=sys.stderr)
        index = scan_all(MANIFEST, paths, args.jobs, stamp)
        with open(index_path, "wb") as f:
            pickle.dump(index, f, protocol=pickle.HIGHEST_PROTOCOL)

    wanted = sorted(index.cells)
    if args.ranges:
        ranges = parse_ranges(args.ranges)
        wanted = [cp for cp in wanted if any(lo <= cp <= hi for lo, hi in ranges)]
    print(f"{len(wanted)} code points to draw", file=sys.stderr)

    chunks = [wanted[i : i + RENDER_CHUNK] for i in range(0, len(wanted), RENDER_CHUNK)]
    made = 0
    with ProcessPoolExecutor(
        max_workers=args.jobs, initializer=_init_worker, initargs=(MANIFEST, paths)
    ) as pool:
        futures = [
            pool.submit(
                _render_chunk,
                [(cp, index.cells[cp]) for cp in chunk],
                root,
                args.size,
                args.force,
            )
            for chunk in chunks
        ]
        for i, future in enumerate(futures):
            made += future.result()
            if (i + 1) % 20 == 0 or i + 1 == len(futures):
                print(
                    f"  {i + 1}/{len(futures)} chunks, {made} images written",
                    file=sys.stderr,
                )
    print(f"wrote {made} images under {root}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
