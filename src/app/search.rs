//! The Search pane: every place a name appears.
//!
//! This is what a Ctrl/Cmd+click does when "go to definition" has nowhere to
//! go — either because the token clicked *is* the declaration, or because the
//! name it refers to is not declared anywhere. Both routes end here, so a typo
//! in a `ref` lists the lines that share the typo rather than doing nothing.
//!
//! For the first route to exist at all, `doc_links::extract_line_links` emits
//! links for **definitions** too, flagged `is_def` — that flag is what stops the
//! editor from "navigating" to the line the click was already on. Two
//! `LinkTargetKind`s are search-only and never navigate: `Anchor` and `Feature`
//! have no declaration site (an anchor is matched by name across glyphs, a
//! feature tag is declared once per target). A *pattern* glyph name gets no
//! definition link either — it is not a name anything can refer to, and only the
//! `$var`s inside it are.
//!
//! Matching goes through [`crate::editor::line_fields`] exactly as links and
//! rename do, so what the search calls an appearance of a glyph name is what
//! the editor calls one — a `remap` group that happens to read like a glyph
//! name is not a hit, and an anchor is hit through both of its signs.
//!
//! Results are addressed by their **ordinal within their file**, not by a line
//! number: opening a file canonicalizes its text, so the line a hit sits at on
//! disk need not be the line it ends up at in the editor. Canonicalization
//! rewrites spacing and comments, never the order names appear in, so the
//! ordinal survives it. The ordinal counts *occurrences*, not lines — a line
//! naming the same glyph twice is two rows — and both ends have to agree on that
//! or every later hit in the file lands one off. Open documents are searched as
//! they stand, unsaved edits included; unopened ones through `file_text`.
//! (Like the navigation history, nothing rewrites a
//! recorded position when the document is edited underneath it; a stale
//! search is re-run by clicking the name again.)

use super::*;
use crate::editor::doc_links::{LinkSpan, scan_dollar_refs};
use crate::editor::line_fields::{FieldRole, classify_line};

/// Char-column spans on `line` at which `name` appears in the role `kind`
/// names — the whole written token, so the search pane can highlight exactly
/// what it matched (an anchor's sign and a quoted token's backticks included).
///
/// The substring test in front is what keeps a search cheap: classifying a
/// line costs a tokenizing pass, and a font directory is mostly pixel rows
/// that can never match. Every kind's name occurs **literally** in the source
/// — a name-parts name carries its own `$`, and an anchor's sign only ever
/// precedes the name — so the filter cannot hide a hit. `search_name` leans on
/// the same invariant one level up, per file. Together they are what keeps a
/// search a click and not a wait; over `font/`, 9.9 ms → 1.7 ms.
///
/// The returned span is the written token as a whole, which the pane highlights
/// so a long `remap` or `assert` row says where on it the name actually is.
pub(super) fn match_spans(line: &str, name: &str, kind: LinkTargetKind) -> Vec<(usize, usize)> {
    if !line.contains(name) {
        return Vec::new();
    }
    let mut cols = Vec::new();
    for f in classify_line(line) {
        match kind {
            // A name-parts variable appears *inside* other tokens, so the
            // column is the `$var`'s own, not the token's.
            LinkTargetKind::NameParts => match f.role {
                FieldRole::NamePartsDef if f.token == name => {
                    cols.push((f.col_start, f.col_end))
                }
                FieldRole::GlyphDef | FieldRole::GlyphRef | FieldRole::NamePartsValue => {
                    let mut spans: Vec<LinkSpan> = Vec::new();
                    scan_dollar_refs(&f.token, f.col_start, &mut spans);
                    cols.extend(
                        spans
                            .into_iter()
                            .filter(|s| s.target == name)
                            .map(|s| (s.col_start, s.col_end)),
                    );
                }
                _ => {}
            },
            LinkTargetKind::Glyph => {
                if matches!(f.role, FieldRole::GlyphDef | FieldRole::GlyphRef)
                    && f.token == name
                {
                    cols.push((f.col_start, f.col_end));
                }
            }
            LinkTargetKind::Color => {
                if matches!(f.role, FieldRole::ColorDef | FieldRole::ColorRef)
                    && f.token == name
                {
                    cols.push((f.col_start, f.col_end));
                }
            }
            LinkTargetKind::Remap => {
                if matches!(f.role, FieldRole::RemapGroupDef | FieldRole::RemapGroupRef)
                    && f.token == name
                {
                    cols.push((f.col_start, f.col_end));
                }
            }
            LinkTargetKind::Feature => {
                if f.role == FieldRole::FeatureDef && f.token == name {
                    cols.push((f.col_start, f.col_end));
                }
            }
            LinkTargetKind::Face => {
                if matches!(f.role, FieldRole::FaceDef | FieldRole::FaceRef) && f.token == name {
                    cols.push((f.col_start, f.col_end));
                }
            }
            LinkTargetKind::Slice => {
                if matches!(f.role, FieldRole::SliceDef | FieldRole::SliceRef) && f.token == name {
                    cols.push((f.col_start, f.col_end));
                }
            }
            // Attachment is symmetric, so `+above` and `-above` are the same
            // anchor and both are listed without distinction.
            LinkTargetKind::Anchor => {
                if f.role == FieldRole::PointDef
                    && f.token.strip_prefix(['+', '-']).unwrap_or(&f.token) == name
                {
                    cols.push((f.col_start, f.col_end));
                }
            }
        }
    }
    cols.sort_unstable();
    cols.dedup();
    cols
}

/// Every appearance in a line list, as `(line index, char span)` in order.
fn hits_in_doclines(
    lines: &[DocLine],
    name: &str,
    kind: LinkTargetKind,
) -> Vec<(usize, (usize, usize))> {
    let mut hits = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let DocLine::Text(text) = line else { continue };
        hits.extend(match_spans(text, name, kind).into_iter().map(|s| (i, s)));
    }
    hits
}

/// One listed appearance.
pub(super) struct SearchHit {
    pub path: PathBuf,
    /// Position among this file's own hits; see the module note on why the
    /// line number is not what a click navigates by.
    pub ordinal: usize,
    /// 1-based, for display only.
    pub file_line: usize,
    /// The source line, trimmed.
    pub text: String,
    /// Char range **within `text`** of the token this row matched, so the pane
    /// highlights this occurrence and not every one on a line that has
    /// several — each occurrence is its own row.
    pub highlight: (usize, usize),
}

/// Builds one hit from a matched line, moving the span into the trimmed text
/// the pane displays.
fn hit(path: &std::path::Path, ordinal: usize, file_line: usize, line: &str, span: (usize, usize)) -> SearchHit {
    let leading = line.chars().count() - line.trim_start().chars().count();
    SearchHit {
        path: path.to_path_buf(),
        ordinal,
        file_line,
        text: line.trim().to_string(),
        highlight: (span.0.saturating_sub(leading), span.1.saturating_sub(leading)),
    }
}

pub(super) struct SearchResults {
    pub name: String,
    pub kind: LinkTargetKind,
    pub hits: Vec<SearchHit>,
    pub file_count: usize,
}

impl SearchResults {
    /// What the pane's header says: the kind and name searched for.
    pub(super) fn title(&self) -> String {
        let kind = match self.kind {
            LinkTargetKind::Glyph => "glyph",
            LinkTargetKind::NameParts => "name-parts",
            LinkTargetKind::Color => "color",
            LinkTargetKind::Remap => "remap group",
            LinkTargetKind::Feature => "feature",
            LinkTargetKind::Anchor => "anchor",
            LinkTargetKind::Face => "face",
            LinkTargetKind::Slice => "slice",
        };
        let n = self.hits.len();
        if n == 0 {
            return format!("{kind} '{}' — no appearances", self.name);
        }
        format!(
            "{kind} '{}' — {n} appearance{} in {} file{}",
            self.name,
            if n == 1 { "" } else { "s" },
            self.file_count,
            if self.file_count == 1 { "" } else { "s" },
        )
    }
}

impl UniformApp {
    /// The on-disk text of a file no pane is editing, cached until its
    /// modification time moves.
    ///
    /// A search runs on a click, so it must not wait on a directory's worth of
    /// I/O each time. The mtime is what keeps the cache honest: a closed file
    /// changes only from outside the editor, and that is exactly what a
    /// generation counter would not see.
    fn file_text(&mut self, path: &std::path::Path) -> Option<&str> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let stale = mtime.is_none()
            || self
                .search_file_cache
                .get(path)
                .is_none_or(|(cached, _)| *cached != mtime);
        if stale {
            let text = std::fs::read_to_string(path).ok()?;
            self.search_file_cache
                .insert(path.to_path_buf(), (mtime, text));
        }
        self.search_file_cache.get(path).map(|(_, t)| t.as_str())
    }

    /// Lists every appearance of `name` and reveals the Search pane.
    ///
    /// Open documents are searched as they stand, including unsaved edits; the
    /// rest come from [`file_text`], which serves them from memory after the
    /// first search. Both pre-filter on the literal name before tokenizing
    /// anything, per file and again per line.
    pub(super) fn search_name(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        kind: LinkTargetKind,
    ) {
        let paths: Vec<PathBuf> = self
            .collect_all_docs()
            .iter()
            .map(|doc| doc.path.clone())
            .collect();

        let mut hits: Vec<SearchHit> = Vec::new();
        let mut file_count = 0usize;
        for path in paths {
            let before = hits.len();
            if let Some(doc) = self
                .open_documents
                .iter()
                .find(|d| d.document.path == path)
            {
                for (ordinal, (line_idx, span)) in
                    hits_in_doclines(&doc.lines, name, kind).into_iter().enumerate()
                {
                    hits.push(hit(
                        &path,
                        ordinal,
                        doc.document.docline_file_line(line_idx),
                        doc.lines[line_idx].as_text().unwrap_or_default(),
                        span,
                    ));
                }
            } else if let Some(content) = self.file_text(&path)
                && content.contains(name)
            {
                // Enumerated over occurrences, not over lines: a line naming
                // the same glyph twice is two rows, and the ordinal has to
                // agree with `hits_in_doclines` once the file opens.
                let found: Vec<_> = content
                    .lines()
                    .enumerate()
                    .flat_map(|(i, text)| {
                        match_spans(text, name, kind)
                            .into_iter()
                            .map(move |s| (i, text, s))
                    })
                    .collect();
                for (ordinal, (line_idx, text, span)) in found.into_iter().enumerate() {
                    hits.push(hit(&path, ordinal, line_idx + 1, text, span));
                }
            }
            if hits.len() > before {
                file_count += 1;
            }
        }

        self.search = Some(SearchResults {
            name: name.to_string(),
            kind,
            hits,
            file_count,
        });
        self.bottom_panel_tab = Some(super::panels::SEARCH_TAB);
        let screen_h = ctx.input(|i| i.screen_rect.height());
        self.ensure_min_panel_height(screen_h);
    }

    /// Opens the file a listed hit is in and puts the caret on it.
    ///
    /// The search pane is not a link in a document, so there is no link
    /// position to come back to; "go back" returns to wherever the caret was
    /// left, which is the only position the user actually departed from.
    pub(super) fn goto_search_hit(&mut self, ctx: &egui::Context, hit_idx: usize) {
        let Some(search) = &self.search else { return };
        let Some(hit) = search.hits.get(hit_idx) else { return };
        let (path, ordinal) = (hit.path.clone(), hit.ordinal);
        let (name, kind) = (search.name.clone(), search.kind);

        let from = self.active_doc_idx().and_then(|idx| {
            let doc = self.open_documents.get(idx)?;
            Some(NavLoc::new(
                idx,
                doc.editor_state.cursor.line,
                doc.editor_state.cursor.col,
            ))
        });

        self.open_file(path.clone());
        let Some(idx) = self
            .open_documents
            .iter()
            .position(|d| d.document.path == path)
        else {
            return;
        };
        self.panes.show_document(idx);

        let doc = &mut self.open_documents[idx];
        let Some(&(line, (col, _))) = hits_in_doclines(&doc.lines, &name, kind).get(ordinal)
        else {
            return;
        };
        doc.editor_state.goto_caret(&doc.lines, line, col);
        if let Some(from) = from {
            self.nav_history.push(NavEntry {
                from,
                to: NavLoc::new(idx, line, col),
            });
        }
        self.focus_pane_editor(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Start columns only; the spans' ends are pinned separately, by the
    /// highlight tests.
    fn cols(line: &str, name: &str, kind: LinkTargetKind) -> Vec<usize> {
        match_spans(line, name, kind).into_iter().map(|(s, _)| s).collect()
    }

    #[test]
    fn glyph_name_is_found_where_it_is_defined_and_used() {
        assert_eq!(cols("glyph foo 8 16", "foo", LinkTargetKind::Glyph), vec![6]);
        assert_eq!(cols("ref foo 0 0", "foo", LinkTargetKind::Glyph), vec![4]);
        assert_eq!(cols("map A = foo", "foo", LinkTargetKind::Glyph), vec![8]);
        assert_eq!(cols("glyph bar = foo", "foo", LinkTargetKind::Glyph), vec![12]);
        assert_eq!(
            cols("remap liga : foo -> bar", "foo", LinkTargetKind::Glyph),
            vec![13],
        );
        assert_eq!(
            cols("assert same foo bar", "foo", LinkTargetKind::Glyph),
            vec![12],
        );
    }

    #[test]
    fn glyph_search_does_not_match_partial_names_or_comments() {
        assert!(cols("glyph foobar 8 16", "foo", LinkTargetKind::Glyph).is_empty());
        assert!(cols("ref foo-ext 0 0", "foo", LinkTargetKind::Glyph).is_empty());
        assert!(cols("ref bar 0 0 // foo", "foo", LinkTargetKind::Glyph).is_empty());
    }

    /// A remap group and a glyph can share a name; they are different things,
    /// and a glyph search must not list the group.
    #[test]
    fn a_remap_group_is_not_a_glyph_name() {
        assert!(cols("remap foo : a -> b", "foo", LinkTargetKind::Glyph).is_empty());
        assert_eq!(cols("remap foo : a -> b", "foo", LinkTargetKind::Remap), vec![6]);
        assert_eq!(
            cols("feature liga for latn : foo", "foo", LinkTargetKind::Remap),
            vec![24],
        );
    }

    #[test]
    fn a_feature_tag_is_found_on_every_declaration() {
        assert_eq!(
            cols("feature ccmp for latn : g", "ccmp", LinkTargetKind::Feature),
            vec![8],
        );
        assert_eq!(
            cols("feature ccmp for cyrl/SRB : g", "ccmp", LinkTargetKind::Feature),
            vec![8],
        );
        // The group it points at is not the tag.
        assert!(cols("feature liga for latn : ccmp", "ccmp", LinkTargetKind::Feature).is_empty());
    }

    /// Both signs of an anchor are the same anchor, and the anchor-driven
    /// `feature` variant names one too.
    #[test]
    fn an_anchor_is_found_through_both_signs() {
        assert_eq!(cols("anchor +above 4 1", "above", LinkTargetKind::Anchor), vec![7]);
        assert_eq!(cols("anchor -above 2 1", "above", LinkTargetKind::Anchor), vec![7]);
        assert_eq!(
            cols("feature abvm for hang : anchor above", "above", LinkTargetKind::Anchor),
            vec![31],
        );
    }

    #[test]
    fn a_name_parts_variable_is_found_inside_the_names_it_builds() {
        assert_eq!(
            cols("name-parts $init = a b c", "$init", LinkTargetKind::NameParts),
            vec![11],
        );
        assert_eq!(
            cols("name-parts $combo = $init $final", "$init", LinkTargetKind::NameParts),
            vec![20],
        );
        assert_eq!(
            cols("glyph hangul-($init)-l 8 16", "$init", LinkTargetKind::NameParts),
            vec![14],
        );
        assert_eq!(
            cols("ref hangul-$init 0 0", "$init", LinkTargetKind::NameParts),
            vec![11],
        );
        // No partial matches: `$initial` is a different variable.
        assert!(cols("ref hangul-$initial 0 0", "$init", LinkTargetKind::NameParts).is_empty());
    }

    #[test]
    fn a_color_is_found_at_its_definition_and_its_uses() {
        assert_eq!(cols("color red = #ff0000", "red", LinkTargetKind::Color), vec![6]);
        assert_eq!(
            cols("color light-red = red", "red", LinkTargetKind::Color),
            vec![18],
        );
        assert_eq!(
            cols("ref foo 0 0 fill red", "red", LinkTargetKind::Color),
            vec![17],
        );
    }

    #[test]
    fn several_appearances_on_one_line_are_all_reported() {
        assert_eq!(
            cols("glyph foo = foo", "foo", LinkTargetKind::Glyph),
            vec![6, 12],
        );
    }

    /// The pane shows the line trimmed, so the highlight has to move with it.
    #[test]
    fn the_highlight_follows_the_trimmed_text() {
        let line = "    ref foo 0 0";
        let span = match_spans(line, "foo", LinkTargetKind::Glyph)[0];
        let h = hit(std::path::Path::new("a.unf"), 0, 3, line, span);
        assert_eq!(h.text, "ref foo 0 0");
        assert_eq!(&h.text[h.highlight.0..h.highlight.1], "foo");
    }

    /// The span is the *written* token, so an anchor's sign and a quoted
    /// token's backticks are highlighted with it — what is picked out is what
    /// the line actually says, not a reconstruction of the bare name.
    #[test]
    fn the_highlight_covers_the_token_as_written() {
        for (line, name, kind, expected) in [
            ("anchor +above 4 1", "above", LinkTargetKind::Anchor, "+above"),
            ("ref `foo bar` 0 0", "foo bar", LinkTargetKind::Glyph, "`foo bar`"),
            ("glyph x-$init 2 2", "$init", LinkTargetKind::NameParts, "$init"),
        ] {
            let span = *match_spans(line, name, kind)
                .first()
                .unwrap_or_else(|| panic!("no match in {line:?}"));
            let h = hit(std::path::Path::new("a.unf"), 0, 1, line, span);
            assert_eq!(&h.text[h.highlight.0..h.highlight.1], expected, "{line:?}");
        }
    }

    /// Two occurrences on one line are two rows, each highlighting its own.
    #[test]
    fn each_row_highlights_its_own_occurrence() {
        let line = "glyph foo = foo";
        let spans = match_spans(line, "foo", LinkTargetKind::Glyph);
        assert_eq!(spans.len(), 2);
        let hits: Vec<_> = spans
            .into_iter()
            .enumerate()
            .map(|(i, s)| hit(std::path::Path::new("a.unf"), i, 1, line, s))
            .collect();
        assert_eq!(hits[0].highlight, (6, 9));
        assert_eq!(hits[1].highlight, (12, 15));
    }

    #[test]
    fn hits_run_over_a_document_in_order_and_skip_pixel_grids() {
        use crate::document::PixelGrid;
        let lines = vec![
            DocLine::Text("glyph foo 2 2".to_string()),
            DocLine::Grid(PixelGrid::new(2, 2)),
            DocLine::Text("ref foo 0 0".to_string()),
            DocLine::Text("map A = foo".to_string()),
        ];
        assert_eq!(
            hits_in_doclines(&lines, "foo", LinkTargetKind::Glyph),
            vec![(0, (6, 9)), (2, (4, 7)), (3, (8, 11))],
        );
    }
}

