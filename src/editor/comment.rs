//! Ctrl/Cmd+`/` — commenting a run of lines out, and taking the comment back off.
//!
//! # What a comment is here
//!
//! Only a *whole-line* comment: a line whose every token is inside the `// …`,
//! which is what [`crate::document::DocumentItem::Comment`] models. A trailing
//! comment on a directive (`glyph foo 4 4 // note`) is part of that directive's
//! line and is never what this toggle adds or removes — the test is `^\s*//`,
//! read at the start of the line and nowhere else.
//!
//! Commenting prepends `// ` at column 0; uncommenting strips `^\s*//\s*`
//! greedily, so the indentation goes with the marker rather than being
//! restored. The two are deliberately *not* inverses of each other on an
//! indented line: this format has no meaningful indentation to preserve, and a
//! greedy strip is what makes a commented pixel row decode again (a row is
//! `..@@..` exactly, with nothing before it).
//!
//! # Which lines are the target
//!
//! Line-wise, like every editor's version of this: the lines the selection's
//! two ends sit on, everything between them, or the caret's own line when
//! there is no selection. Two things widen that range:
//!
//! * A header and the grid under it are one block (the rule
//!   [`crate::editor::doc_input`]'s line-wise copy already follows), so
//!   including either end of the pair pulls the other in. Otherwise a
//!   commented header would leave its grid orphaned, and `reconcile` would
//!   demote the pixels the user never asked to touch.
//! * When *uncommenting*, a commented header at the end of the range pulls the
//!   commented pixel rows that follow it back in, so the block that was
//!   commented as one glyph comes back as one glyph.
//!
//! # Grids
//!
//! A [`DocLine::Grid`] is one line to the editor and many lines to the file, so
//! it cannot be commented as it stands: it is **demoted** to its pixel rows
//! first and those rows are commented, exactly as `reconcile` demotes an
//! orphan. An all-empty grid demotes to *nothing*, for the same reason it does
//! there — those rows were never in the file, and the serializer writes none.
//!
//! The way back is [`promote`], which fuses a header and the pixel rows under
//! it back into a grid. It mirrors [`crate::document_io::parse_doclines`]
//! rather than being stricter than it, so what an uncomment produces is what
//! the same text would have parsed as had it been read from disk.
//!
//! Because the result is already structurally settled, the caller sets
//! `skip_reconcile`: a partially uncommented glyph (rows still commented under
//! a live header) is a state the user is in the middle of creating, and
//! `reconcile` would answer it by inserting an empty grid.

use crate::document::{DocLine, PixelGrid};
use crate::document_io::{decode_grid_row, encode_grid_row};
use crate::editor::caret::Caret;
use crate::editor::reconcile::parse_glyph_header_dims;
use crate::editor::undo::UndoStack;

/// What commenting a line puts in front of it.
const PREFIX: &str = "// ";

/// The text of a whole-line comment, with `^\s*//\s*` taken off, or `None`
/// when the line is not one.
fn comment_body(s: &str) -> Option<&str> {
    Some(s.trim_start().strip_prefix("//")?.trim_start())
}

fn is_comment(s: &str) -> bool {
    comment_body(s).is_some()
}

/// A line that says nothing either way: it is commented along with the rest,
/// but it never makes the range "not all comments".
fn is_blank(line: &DocLine) -> bool {
    match line {
        DocLine::Text(t) => t.trim().is_empty(),
        // An empty grid writes no rows at all, so it is not a line of the file.
        DocLine::Grid(g) => g.is_all_empty(),
    }
}

/// The header dimensions a *commented* header line states, if it is one.
fn commented_header_dims(s: &str) -> Option<(u16, u16)> {
    parse_glyph_header_dims(comment_body(s)?)
}

/// Toggle whole-line comments over the selection (or the caret's line).
///
/// Returns whether anything changed. The caret and the selection are carried
/// to where their content ended up, so pressing the chord twice leaves the
/// same block selected.
pub(crate) fn toggle(
    lines: &mut Vec<DocLine>,
    undo: &mut UndoStack,
    cursor: &mut Caret,
    selection_anchor: &mut Option<Caret>,
) -> bool {
    if lines.is_empty() {
        return false;
    }

    let caret_before = *cursor;
    let (sel_lo, sel_hi) = match *selection_anchor {
        Some(anchor) if anchor != *cursor => ((*cursor).min(anchor), (*cursor).max(anchor)),
        _ => (*cursor, *cursor),
    };
    let (mut lo, mut hi) = (
        sel_lo.line.min(lines.len() - 1),
        sel_hi.line.min(lines.len() - 1),
    );

    // The header/grid pair is one block, from either end.
    if matches!(&lines[lo], DocLine::Grid(_))
        && lo > 0
        && matches!(&lines[lo - 1], DocLine::Text(t) if parse_glyph_header_dims(t).is_some())
    {
        lo -= 1;
    }
    if matches!(&lines[hi], DocLine::Text(t) if parse_glyph_header_dims(t).is_some())
        && matches!(lines.get(hi + 1), Some(DocLine::Grid(_)))
    {
        hi += 1;
    }

    // Uncomment only when every line that says anything already is a comment.
    let mut saw_content = false;
    let mut all_comments = true;
    for line in &lines[lo..=hi] {
        if is_blank(line) {
            continue;
        }
        saw_content = true;
        all_comments &= matches!(line, DocLine::Text(t) if is_comment(t));
    }
    let uncomment = saw_content && all_comments;

    // A commented header takes its commented rows with it, so the glyph comes
    // back whole. Only on the way out: on the way in those rows are already
    // commented and must not be commented twice.
    if uncomment
        && let DocLine::Text(t) = &lines[hi]
        && let Some((w, h)) = commented_header_dims(t)
    {
        let mut rows = 0usize;
        while rows < h as usize
            && matches!(lines.get(hi + 1 + rows),
                Some(DocLine::Text(t)) if comment_body(t).is_some_and(|b| decode_grid_row(b, w).is_some()))
        {
            rows += 1;
        }
        hi += rows;
    }

    // The promotion below needs the header the rows belong to, even when the
    // range starts at the rows themselves.
    let region_start = if lo > 0
        && !matches!(&lines[lo], DocLine::Grid(_))
        && matches!(&lines[lo - 1], DocLine::Text(t) if parse_glyph_header_dims(t).is_some())
    {
        lo - 1
    } else {
        lo
    };

    // Phase 1: rewrite the range, demoting the grids that have to become text.
    let mut staged: Vec<DocLine> = Vec::with_capacity(hi - region_start + 1);
    let mut map: Vec<usize> = Vec::with_capacity(hi - region_start + 1);
    let mut col_delta: Vec<i64> = Vec::with_capacity(hi - region_start + 1);
    for (i, line) in lines[region_start..=hi].iter().enumerate() {
        map.push(staged.len());
        let untouched = region_start + i < lo;
        match line {
            _ if untouched => {
                col_delta.push(0);
                staged.push(line.clone());
            }
            DocLine::Grid(g) => {
                col_delta.push(0);
                // An all-empty grid was never text in the file; demoting it
                // would write rows of blank pixel text nothing ever wrote.
                if !g.is_all_empty() {
                    for row in 0..g.height {
                        staged.push(DocLine::Text(format!(
                            "{PREFIX}{}",
                            encode_grid_row(g, row)
                        )));
                    }
                }
            }
            DocLine::Text(t) => {
                if uncomment {
                    match comment_body(t) {
                        Some(body) => {
                            col_delta.push(body.chars().count() as i64 - t.chars().count() as i64);
                            staged.push(DocLine::Text(body.to_string()));
                        }
                        None => {
                            col_delta.push(0);
                            staged.push(line.clone());
                        }
                    }
                } else {
                    // A line with nothing on it takes the bare marker: `// `
                    // would only be trailing whitespace to trim on the way out.
                    let commented = if t.is_empty() {
                        "//".to_string()
                    } else {
                        format!("{PREFIX}{t}")
                    };
                    col_delta.push(commented.chars().count() as i64 - t.chars().count() as i64);
                    staged.push(DocLine::Text(commented));
                }
            }
        }
    }

    // Phase 2: fuse every header/rows pair the rewrite exposed back into a grid.
    let (new_region, promoted) = promote(&staged);

    let old_region = lines[region_start..=hi].to_vec();
    if new_region == old_region {
        return false;
    }

    let remap = |c: Caret| -> Caret {
        if c.line < region_start {
            return c;
        }
        if c.line > hi {
            let shifted = c.line + new_region.len() - old_region.len();
            return Caret::new(shifted, c.col);
        }
        let idx = c.line - region_start;
        // A dropped line (the empty grid that demotes to nothing) has no line
        // of its own left: it maps to whatever now stands where it was.
        let staged_line = map[idx];
        let new_idx = promoted
            .get(staged_line)
            .copied()
            .unwrap_or(new_region.len());
        let line = region_start + new_idx;
        let col = match new_region.get(new_idx) {
            // The content moved into a grid, or the line was one: a grid takes
            // no column.
            Some(DocLine::Grid(_)) | None => 0,
            Some(DocLine::Text(t)) => {
                (c.col as i64 + col_delta[idx]).clamp(0, t.chars().count() as i64) as usize
            }
        };
        Caret::new(line, col)
    };

    let caret_after = remap(*cursor);
    let anchor_after = selection_anchor.map(remap);

    undo.break_coalesce();
    undo.push_lines(
        region_start,
        old_region.clone(),
        new_region.clone(),
        caret_before,
        caret_after,
    );
    undo.break_coalesce();
    lines.splice(region_start..=hi, new_region);
    *cursor = caret_after;
    *selection_anchor = anchor_after.filter(|a| *a != caret_after);
    true
}

/// Fuse each `glyph NAME W H` header with the pixel rows written under it,
/// the way [`crate::document_io::parse_doclines`] does when the same text is
/// read from a file. Returns the new lines and, for each input line, the index
/// it ended up at — the rows of one grid all land on the grid itself.
fn promote(lines: &[DocLine]) -> (Vec<DocLine>, Vec<usize>) {
    let mut out: Vec<DocLine> = Vec::with_capacity(lines.len());
    let mut map: Vec<usize> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let dims = match &lines[i] {
            DocLine::Text(t) => parse_glyph_header_dims(t),
            DocLine::Grid(_) => None,
        };
        // A header that already has its grid is left alone: the empty grid the
        // parser hands a row-less header is the grid, and a second one here
        // would orphan it.
        let has_grid = matches!(lines.get(i + 1), Some(DocLine::Grid(_)));
        let Some((w, h)) = dims.filter(|_| !has_grid) else {
            map.push(out.len());
            out.push(lines[i].clone());
            i += 1;
            continue;
        };

        let mut rows: Vec<Vec<crate::pixel::PixelShape>> = Vec::new();
        while rows.len() < h as usize {
            let Some(DocLine::Text(t)) = lines.get(i + 1 + rows.len()) else {
                break;
            };
            let Some(row) = decode_grid_row(t, w) else {
                break;
            };
            rows.push(row);
        }
        // No rows at all still gets the empty grid every dimensioned header
        // owns, so the glyph is drawable the moment it is uncommented.
        map.push(out.len());
        out.push(lines[i].clone());
        let mut grid = PixelGrid::new(w, h);
        for (r, row) in rows.iter().enumerate() {
            for (c, shape) in row.iter().enumerate() {
                grid.set(r as u16, c as u16, *shape);
            }
        }
        let grid_at = out.len();
        out.push(DocLine::Grid(grid));
        for _ in 0..rows.len() {
            map.push(grid_at);
        }
        i += 1 + rows.len();
    }
    (out, map)
}
