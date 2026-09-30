//! Steps 2 and 3 of the specimen's three: which cells exist in which sections
//! ([`SpecimenState::rebuild_sections`]) and which row each cell is on ([`GridLayout`]).

use std::collections::{BTreeMap, BTreeSet};

use crate::ucd::format_block_range;

#[cfg(test)]
use super::status::{format_coverage, uvs_label};
use super::{
    CELL_H, CharEntry, ELLIPSIS_H, FOLD_EDGE_ROWS, GridLayout, Group, HEADING_H, Item, Row,
    Section, SpecimenState, UvsEntry,
};

impl GridLayout {
    pub(super) fn total_height(&self) -> f32 {
        self.row_y.last().copied().unwrap_or(0.0)
    }

    /// The top of row `idx`, or the grid's full height for `rows.len()`.
    pub(super) fn row_top(&self, idx: usize) -> f32 {
        self.row_y
            .get(idx)
            .copied()
            .unwrap_or_else(|| self.total_height())
    }

    /// The row containing `y`, an offset from the grid origin.
    pub(super) fn row_at(&self, y: f32) -> Option<usize> {
        if y < 0.0 {
            return None;
        }
        let idx = self.row_y.partition_point(|ry| *ry <= y).checked_sub(1)?;
        (idx < self.rows.len()).then_some(idx)
    }

    /// The rows overlapping the vertical band `top..bottom`, as offsets from the
    /// grid origin. A heading row is shorter than a cell row, so this is a
    /// binary search rather than a division.
    pub(super) fn visible_rows(&self, top: f32, bottom: f32) -> std::ops::Range<usize> {
        let first = self
            .row_y
            .partition_point(|y| *y <= top)
            .saturating_sub(1)
            .min(self.rows.len());
        let last = self
            .row_y
            .partition_point(|y| *y < bottom)
            .min(self.rows.len());
        first..last.max(first)
    }
}

impl SpecimenState {
    /// Step 2: which cells the grid has, and how they are grouped. Reads only
    /// what step 1 left behind, so a change of options never re-reads the
    /// documents.
    pub(super) fn rebuild_sections(&mut self) {
        self.sections_key = Some(self.options);
        self.layout = None;
        self.unfolded.clear();
        self.entries.clear();
        self.uvs_entries.clear();
        self.items.clear();
        self.sections.clear();

        // Group the mapped characters by block, in code point order. A code
        // point in a gap between blocks is in the gap's `Unassigned` block
        // ([`crate::ucd::BlockMap`]), which is never filled.
        let mut by_block: BTreeMap<(u32, u32), (String, bool, Vec<u32>)> = BTreeMap::new();
        // A base that only variation sequences name still gets a cell — see
        // [`UvsEntry`] — so the grouping runs over both sets of code points.
        let cell_cps: BTreeSet<u32> = self
            .declared
            .keys()
            .chain(self.uvs.keys())
            .copied()
            .collect();
        for cp in cell_cps {
            let b = self.blocks.block_of(cp);
            by_block
                .entry((b.start, b.end))
                .or_insert_with(|| (b.name.to_string(), b.unassigned, Vec::new()))
                .2
                .push(cp);
        }

        let mut groups: Vec<Group> = Vec::new();
        for ((start, end), (name, unassigned, cps)) in by_block {
            // The coverage is a count of *characters*, not of cells, so it
            // reads the same whether or not the grid is filled — and a block
            // with a glyph on a code point it has no character for states none
            // at all, the fraction being one that would read over 100%. A gap
            // has no characters to be a fraction of.
            let coverage = (!unassigned && cps.iter().all(|&cp| self.char_props.is_assigned(cp)))
                .then(|| (cps.len(), self.block_total(start, end)));
            let cps = if self.options.show_undeclared && !unassigned {
                self.block_members(start, end).collect()
            } else {
                cps
            };
            let range = format_block_range(start, end);
            groups.push(Group {
                heading: Some(format!("{name}  {range}")),
                coverage,
                cps,
            });
        }

        let grouped = self.options.group_by_block;
        for Group {
            heading,
            coverage,
            cps,
        } in groups
        {
            let start = self.items.len();
            for cp in cps {
                let declared = self.declared.get(&cp).cloned();
                let unresolved = declared.as_ref().is_some_and(|(_, u)| *u);
                self.entries.push(CharEntry {
                    cp,
                    glyph_name: declared.map(|(n, _)| n),
                    unresolved,
                });
                self.items.push(Item::Char(self.entries.len() - 1));
                // `BTreeMap`, so the selectors come out in order.
                for (selector, (glyph_name, unresolved)) in
                    self.uvs.get(&cp).cloned().unwrap_or_default()
                {
                    self.uvs_entries.push(UvsEntry {
                        base: cp,
                        selector,
                        glyph_name,
                        unresolved,
                    });
                    self.items.push(Item::Uvs(self.uvs_entries.len() - 1));
                }
            }
            if grouped {
                let len = self.items.len() - start;
                self.sections.push(Section {
                    heading,
                    coverage,
                    start,
                    len,
                });
            }
        }

        // Remap-only glyphs come last: they have no code point to sort among
        // the blocks, so a grouped grid gives them a heading of their own.
        let remap_start = self.items.len();
        for i in 0..self.remap_entries.len() {
            self.items.push(Item::Remap(i));
        }
        if !grouped {
            self.sections.push(Section {
                heading: None,
                coverage: None,
                start: 0,
                len: self.items.len(),
            });
        } else if self.items.len() > remap_start {
            self.sections.push(Section {
                heading: Some("Remaps".to_string()),
                coverage: None,
                start: remap_start,
                len: self.items.len() - remap_start,
            });
        }
    }

    /// Every code point of one block, minus the ones a nested `prop block`
    /// claims — those belong to that block's own section.
    fn block_range(&self, start: u32, end: u32) -> impl Iterator<Item = u32> + '_ {
        // Both bounds, not just the start: a `prop block` claim at the very
        // beginning of a Private Use plane shares its start with the UCD block
        // it overrides, and comparing starts alone would then fill the claimed
        // code points into both sections.
        (start..=end).filter(move |&cp| {
            let b = self.blocks.block_of(cp);
            (b.start, b.end) == (start, end)
        })
    }

    /// Every character of one block — the ones a filled grid gives a cell to:
    /// every one the source has ([`crate::ucd::CharProps::is_assigned`], which
    /// is the `prop` lines inside Private Use and the UCD outside it), plus the
    /// ones it draws. A block's permanent holes and its unassigned tail are not
    /// holes in the *font*, so they get no cell.
    fn block_members(&self, start: u32, end: u32) -> impl Iterator<Item = u32> + '_ {
        self.block_range(start, end).filter(move |&cp| {
            self.declared.contains_key(&cp)
                || self.uvs.contains_key(&cp)
                || self.char_props.is_assigned(cp)
        })
    }

    /// How many characters a block has — the denominator of the coverage its
    /// heading states, and the one thing there that is *not* a count of cells:
    /// a code point the source draws without stating is a cell but not a
    /// character.
    fn block_total(&self, start: u32, end: u32) -> usize {
        self.block_range(start, end)
            .filter(|&cp| self.char_props.is_assigned(cp))
            .count()
    }

    /// Step 3: which cells sit on which row, for `cols` columns.
    ///
    /// A long section is folded in the middle: [`FOLD_EDGE_ROWS`] rows stay at
    /// each end and everything between them becomes one [`Row::Fold`] a click
    /// opens. This is `demo.html`'s rule (`demo.js`, `FOLD_OVER`) applied to a
    /// grid whose rows are as wide as the panel: filling a block out to its
    /// whole range puts the 11,172 Hangul syllables on seven hundred rows, and
    /// a code chart is read by scrolling. Only *that* mode folds — with
    /// undeclared characters hidden every row on the grid is a glyph the source
    /// drew, which is what the panel is open to look at.
    pub(super) fn build_layout(&self, cols: usize) -> GridLayout {
        let mut rows: Vec<Row> = Vec::new();
        let mut row_y: Vec<f32> = vec![0.0];
        let mut y = 0.0_f32;
        for (si, sec) in self.sections.iter().enumerate() {
            let mut cell_rows: Vec<Row> = Vec::new();
            let mut i = sec.start;
            while i < sec.start + sec.len {
                let len = cols.min(sec.start + sec.len - i);
                cell_rows.push(Row::Cells { start: i, len });
                i += len;
            }
            if cell_rows.is_empty() {
                continue;
            }
            if self.options.show_undeclared
                && !self.unfolded.contains(&si)
                && cell_rows.len() > 2 * FOLD_EDGE_ROWS
            {
                let hidden = cell_rows.len() - 2 * FOLD_EDGE_ROWS;
                cell_rows.splice(
                    FOLD_EDGE_ROWS..FOLD_EDGE_ROWS + hidden,
                    [Row::Fold {
                        section: si,
                        hidden,
                    }],
                );
            }
            if sec.heading.is_some() {
                rows.push(Row::Heading(si));
                y += HEADING_H;
                row_y.push(y);
            }
            for row in cell_rows {
                y += match row {
                    Row::Fold { .. } => ELLIPSIS_H,
                    _ => CELL_H,
                };
                rows.push(row);
                row_y.push(y);
            }
        }
        GridLayout { cols, rows, row_y }
    }

    /// Whether the vertical border *before* item `idx` separates a variation
    /// sequence from the cell it varies — its base, or an earlier sequence of
    /// the same base. Those borders are not drawn at all: the `n + 1` cells of
    /// one base are one open box, which reads as a run at a glance where a
    /// lighter or dashed rule of the same width does not.
    ///
    /// The question is asked of the item alone and not of the pair around it,
    /// so a run broken across two rows is left open at *both* ends of the
    /// break: the right edge of the row that fills up (the border before the
    /// item that did not fit) and the left edge of the row that continues it,
    /// which is what says the run goes on. `idx` past the last item — the
    /// right edge of the final row — is simply not a boundary.
    pub(super) fn uvs_boundary(&self, idx: usize) -> bool {
        matches!(self.items.get(idx), Some(Item::Uvs(_)))
    }

    #[cfg(test)]
    pub(super) fn glyph_for_cp(&self, cp: u32) -> Option<&str> {
        self.declared.get(&cp).map(|(n, _)| n.as_str())
    }

    /// The grid as `show` would lay it out at `cols` columns, one string per
    /// drawn row: `# HEADING` for a heading row, the cells' code points (or a
    /// remap glyph's name) for a cell row. A row the exclusion rule hid is
    /// simply not there.
    #[cfg(test)]
    pub(super) fn row_summaries(&mut self, cols: usize) -> Vec<String> {
        if self.sections_key != Some(self.options) {
            self.rebuild_sections();
        }
        let layout = self.build_layout(cols);
        layout
            .rows
            .iter()
            .map(|row| match row {
                Row::Heading(si) => {
                    // `show` lays the coverage out at the right edge of the
                    // grid; two spaces is only this summary's way of saying so.
                    let sec = &self.sections[*si];
                    let cov = sec
                        .coverage
                        .map(|c| format!("  {}", format_coverage(c)))
                        .unwrap_or_default();
                    format!("# {}{cov}", sec.heading.as_deref().unwrap_or(""))
                }
                Row::Fold { hidden, .. } => format!("\u{2026} {hidden}"),
                Row::Cells { start, len } => self.items[*start..*start + *len]
                    .iter()
                    .map(|item| match item {
                        Item::Char(i) => format!("{:04X}", self.entries[*i].cp),
                        Item::Uvs(i) => uvs_label(&self.char_props, &self.uvs_entries[*i]),
                        Item::Remap(ri) => self.remap_entries[*ri].glyph_name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            })
            .collect()
    }

    /// Every character cell of the grid, in drawing order.
    #[cfg(test)]
    pub(super) fn cell_cps(&mut self) -> Vec<u32> {
        if self.sections_key != Some(self.options) {
            self.rebuild_sections();
        }
        self.items
            .iter()
            .filter_map(|item| match item {
                Item::Char(i) => Some(self.entries[*i].cp),
                Item::Uvs(_) | Item::Remap(_) => None,
            })
            .collect()
    }

    #[cfg(test)]
    pub(super) fn remap_glyph_names(&self) -> Vec<&str> {
        self.remap_entries
            .iter()
            .map(|e| e.glyph_name.as_str())
            .collect()
    }
}
