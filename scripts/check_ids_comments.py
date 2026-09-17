#!/usr/bin/env python3
"""Check (and fix) the character comments `font/han-*.unf` writes beside names.

Two kinds of line in the han sources carry a comment that is not prose but a
*restatement* of what the line already says in glyph names:

    glyph han-7247:8x16 8 16 // 片
    ⿰ han-7247:8x16 1 han-5351:6x16 // ⿰片卑

The comment is what makes those files readable -- `han-5351` is a codepoint and
卑 is a character -- and it is also the one thing in them nothing checks. A name
edited without its comment, or a line copied and renamed, leaves a comment that
now names the wrong character, and the source still builds: `uniform` never
reads a comment. This script is that check.

What it holds a line to:

  * a `glyph han-XXXX…` header (commented out or not, alias or not) whose
    comment begins with han characters: they have to be exactly `chr(0xXXXX)`;
  * an IDC line (`⿰⿱⿲⿳` and the nine enclosures, `assume`d or not) whose components are all han
    names: the comment has to begin with the operator followed by each
    component's own character, in written order -- `⿰片卑` for the line above.

Anything after that leading run is a hand's note and is left alone, which is
also what makes the comments `gen_ids_composites.py` writes pass unchanged: an
inlined line's `<- ⿰⿰XYC` note and the `-- no-inline` mark are tails, not part
of the sequence, and the sequence they trail is the line's own parts either way.
A comment that begins with no han character at all (`// not distinguished`) is
prose and is not a claim about the line, so it is left alone too, and so is a
line that names something which is not a han glyph at all (`glyph radical-1`,
a component that is a `dia-` mark): there is no character to write for it.

A line with no comment whatsoever is reported only under `--missing`, and an
alias (`glyph han-XXXX.0:15x16 = han-XXXX:15x16`) not even then -- it states the
character twice in names already and conventionally carries no comment.

A few names draw a character other than the one their own codepoint names,
because Unicode disunified the shape after the source had drawn it: `han-5f50-g`
draws 𫜹, not 彐. A comment on a line naming one of those may write either
character, and `ALSO_WRITTEN` below is the list of them -- one line per name,
which is where a newly noticed pair goes.

`--fix` rewrites the offending run in place, note and spacing untouched, and
adds the comment a `--missing` line lacks. Nothing else on the line moves, and
a position the comment already had right keeps the character it had, so that
correcting one part of a line does not restate the others (a deliberate 𫜹 stays
𫜹).

A mismatch says only that the two halves of a line disagree, never which of them
is wrong, and the comment is quite often the right one: `⿺ han-8fb6 han-4ed82 //
⿺辶付` is a mistyped *name* (`han-4ed8`, 付) that nothing but its comment
reveals, since a name is only ever read as a name. Where the name is not a han
character at all, as there, the line is reported and `--fix` leaves it alone
rather than pasting an unassigned codepoint into the one place that still says
what the line meant. Everywhere else `--fix` believes the names, so read the
report before running it.

Usage:
    python3 scripts/check_ids_comments.py [-i font] [--fix] [--missing] [FILE...]
"""

from __future__ import annotations

import argparse
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import gen_ids_composites as G

NUM_RE = re.compile(r"^[+-]?\d+$")

# The characters an IDS may be written with: the ideographs themselves (URO,
# every extension, and the compatibility blocks, since a source may name one),
# the radicals and strokes that stand in for them, and the IDCs that join them.
# The leading run of these in a comment is the part this script owns; the first
# character outside them begins the hand's own note.
IDS_RANGES = [
    (0x2E80, 0x2EFF),    # CJK radicals supplement
    (0x2F00, 0x2FDF),    # Kangxi radicals
    (0x2FF0, 0x2FFF),    # ideographic description characters
    (0x31C0, 0x31EF),    # CJK strokes
    (0x3400, 0x4DBF),    # extension A
    (0x4E00, 0x9FFF),    # URO
    (0xF900, 0xFAFF),    # compatibility ideographs
    (0x20000, 0x323AF),  # extensions B..J, compatibility supplement
    (0x323B0, 0x3347F),  # extension J
]


def is_ids_char(ch: str) -> bool:
    cp = ord(ch)
    return any(lo <= cp <= hi for lo, hi in IDS_RANGES)


# --------------------------------------------------------------------------
# the exceptions
# --------------------------------------------------------------------------

# A name whose *drawing* is another character, so that a comment naming that
# character is right and the codepoint in the name would be the wrong thing to
# write. Unicode disunifying a shape is what makes such a pair: the source keeps
# drawing the old character, since a `.S` shape or a region's own drawing is
# exactly what the disunified character is, and a hand writing an IDC line names
# what is on the page.
#
# Each entry is one name -- matched against the name with its `:LABEL` dropped,
# in full and without regard to what the label is -- and the characters a
# comment may use for it *besides* `chr(cp)`, which is always accepted and is
# the one `--fix` writes. Add a line here rather than teaching the check a rule:
# every one of these is a judgement about a drawing, and the list is meant to be
# read as such.
ALSO_WRITTEN = {
    "han-5f50.1": "𫜹", # shape-based substitutes
    "han-5f50.3": "𫜹",
    "han-5f50-g": "𫜹",
    "han-5f50-p": "𫜹",
}


def chars_for(tok: str) -> list[str] | None:
    """The characters a component or header may be commented as, canonical first.

    `None` for a token that names no han glyph at all, which is what stops the
    line it is on from being checked.
    """
    hn = G.parse_han_name(tok)
    if hn is None:
        return None
    out = [chr(hn.cp)]
    also = ALSO_WRITTEN.get(hn.family)
    if also is not None and also not in out:
        out.append(also)
    return out


def comment_at(line: str) -> int | None:
    """Where the line's own comment starts, past any `//` commenting it out.

    A commented-out block line carries two markers -- `// ⿰ a b // ⿰片卑` --
    and it is the second that introduces the comment this script reads.
    """
    body = len(line) - len(line.lstrip())
    if line[body:body + 2] == "//":
        body += 2
    at = line.find("//", body)
    return None if at < 0 else at


def body_of(line: str) -> str:
    """The directive the line states, without its comment or its `//` marker."""
    s = line.strip()
    if s.startswith("//"):
        s = s[2:]
    return s.partition("//")[0].strip()


def leading_ids(text: str) -> tuple[int, int]:
    """The `(start, end)` of the run of IDS characters the comment opens with."""
    start = len(text) - len(text.lstrip())
    end = start
    while end < len(text) and is_ids_char(text[end]):
        end += 1
    return start, end


def expected(body: str) -> list[list[str]] | None:
    """The comment's sequence, position by position, or `None` for a line that
    makes no claim at all.

    One list per character the comment has to carry -- the operator, then each
    component in written order -- holding what that position may be, canonical
    first. A position has more than one entry only where the name is in
    `ALSO_WRITTEN`, and the positions are independent of each other: a line
    naming two such parts may write either character for either of them.

    `None` covers everything this script does not read: a directive that is
    neither a `glyph` header nor an IDC line, and a line of either kind naming
    something that is not a han glyph.
    """
    toks = body.split()
    if not toks:
        return None
    if toks[0] == "glyph":
        if len(toks) < 2:
            return None
        chars = chars_for(toks[1])
        return None if chars is None else [chars]
    idc = G.idc_tokens(toks)
    if idc is not None:
        toks, _ = idc
        parts = [[toks[0]]]
        for tok in toks[1:]:
            if NUM_RE.match(tok):  # a gap, or an enclosure's offset
                continue
            chars = chars_for(tok)
            if chars is None:
                return None
            parts.append(chars)
        if len(parts) - 1 != G.IDC_ARITY[toks[0]]:
            return None
        return parts
    return None


def canonical(parts: list[list[str]]) -> str:
    """The sequence the names spell codepoint for codepoint."""
    return "".join(p[0] for p in parts)


def repair(found: str, parts: list[list[str]]) -> str:
    """The sequence to write in place of `found`, keeping what it got right.

    A position `found` has an accepted character in keeps it, so that fixing one
    part of a line does not quietly restate the others: a hand that deliberately
    wrote 𫜹 for `han-5f50-g` keeps its 𫜹 when the part beside it is corrected.
    """
    if len(found) != len(parts):
        return canonical(parts)
    return "".join(c if c in p else p[0] for c, p in zip(found, parts))


def check_line(line: str) -> tuple[str, str | None, str] | None:
    """`(expected, found, kind)` where the line's comment is not what it says.

    `found` is `None` where the line carries no comment at all (`kind` is then
    `"missing"`, which only `--missing` reports); `kind` is `"wrong"` where the
    comment opens with han characters that are not the ones the line names.
    A line whose comment opens with prose states nothing and is not reported.

    `"alien"` is the third kind and the one `--fix` will not touch: a name whose
    codepoint is no han character at all, which is what a mistyped name looks
    like (`han-4ed82` for `han-4ed8`). Writing that codepoint into the comment
    would only spread the typo and leave a comment no later run can even read
    back, so the line is reported and left for a hand.
    """
    body = body_of(line)
    parts = expected(body)
    if parts is None:
        return None
    want = canonical(parts)
    if not all(is_ids_char(c) for c in want):
        return (want, None, "alien")
    at = comment_at(line)
    if at is None:
        # An alias (`glyph han-XXXX.0:15x16 = han-XXXX:15x16`) states the
        # character twice in names already and conventionally carries no
        # comment at all, so its not having one is not something to report.
        return None if "=" in body else (want, None, "missing")
    start, end = leading_ids(line[at + 2:])
    found = line[at + 2 + start:at + 2 + end]
    if len(found) == len(parts) and all(c in p for c, p in zip(found, parts)):
        return None
    if not found:
        return None  # prose; the line makes no claim to check
    return (repair(found, parts), found, "wrong")


def fixed_line(line: str, want: str) -> str:
    """The line with its comment's IDS run replaced, or the comment added.

    Only ever called on a line `check_line` reported, so a line that has a
    comment has a run to replace and everything around it is a hand's -- the
    spacing it chose and the note it left both survive untouched.
    """
    at = comment_at(line)
    if at is None:
        return line.rstrip() + f" // {want}"
    start, end = leading_ids(line[at + 2:])
    return line[:at + 2 + start] + want + line[at + 2 + end:]


def unf_files(font_dir: str, given: list[str]) -> list[str]:
    if given:
        return given
    return [
        os.path.join(font_dir, name)
        for name in sorted(os.listdir(font_dir))
        if name.startswith("han-") and name.endswith(".unf")
    ]


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-i", "--font-dir", default="font")
    ap.add_argument("--fix", action="store_true",
                    help="rewrite the comments in place")
    ap.add_argument("--missing", action="store_true",
                    help="also report (and with --fix, add) an absent comment")
    ap.add_argument("--dry-run", action="store_true",
                    help="with --fix, report what would change and write nothing")
    ap.add_argument("files", nargs="*", help="the files to read (default: han-*.unf)")
    args = ap.parse_args()

    counts = {"wrong": 0, "missing": 0, "alien": 0}
    changed = files_changed = 0
    for path in unf_files(args.font_dir, args.files):
        with open(path, encoding="utf-8") as f:
            lines = f.read().split("\n")
        touched = False
        for i, line in enumerate(lines):
            got = check_line(line)
            if got is None:
                continue
            want, found, kind = got
            if kind == "missing" and not args.missing:
                continue
            counts[kind] += 1
            where = f"{path}:{i + 1}:"
            if kind == "missing":
                print(f"{where} no comment; the line draws {want}")
            elif kind == "alien":
                print(f"{where} the name is no han character "
                      f"(U+{ord(want[-1]):04X}); a mistyped name?")
            else:
                print(f"{where} comment says {found}, the line draws {want}")
            if args.fix and kind != "alien":
                lines[i] = fixed_line(line, want)
                changed += 1
                touched = True
        if touched and not args.dry_run:
            with open(path, "w", encoding="utf-8") as f:
                f.write("\n".join(lines))
            files_changed += 1

    if not sum(counts.values()):
        print("all comments match")
        return 0
    alien = f", {counts['alien']} left alone" if counts["alien"] else ""
    if args.fix:
        print(f"{changed} comment(s) {'would be ' if args.dry_run else ''}fixed"
              + ("" if args.dry_run else f" in {files_changed} file(s)") + alien)
        return 1 if counts["alien"] else 0
    missing = f", {counts['missing']} missing" if counts["missing"] else ""
    print(f"{counts['wrong']} wrong comment(s){missing}{alien}")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
