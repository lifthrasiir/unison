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
//! A glyph name is matched against what a token **denotes**, not against how it
//! is written: a name written as a pattern (`fo(o|q)`, `hangul-($init)`) is an
//! appearance of every name it expands to, and the row highlights the pattern
//! token as written. Only exact-name matching would list a fraction of a
//! `font/` where most names are stated by pattern — the search would say a
//! glyph is referred to nowhere while the font composes it. See
//! [`pattern_denotes`] for which grammar each token is read with; navigation
//! matches a *definition* with the same test, so a click on a pattern-declared
//! name goes to the block that declares it.
//!
//! The pane lists **declarations before uses**, each group in source order, and
//! rules a line between them; see [`collect_hits`] for why, and [`MatchSpan`]
//! for what counts as a declaration. It is only the display order that moves —
//! the ordinal below is assigned before the sort.
//!
//! Results are addressed by their **ordinal within their file**, not by a line
//! number: opening a file canonicalizes its text, so the line a hit sits at on
//! disk need not be the line it ends up at in the editor. Canonicalization
//! rewrites spacing and comments, never the order names appear in, so the
//! ordinal survives it. The ordinal counts *occurrences*, not lines — a line
//! naming the same glyph twice is two rows — and both ends have to agree on that
//! or every later hit in the file lands one off. (Like the navigation history,
//! nothing rewrites a recorded position when the document is edited underneath
//! it; a stale search is re-run by clicking the name again.)
//!
//! Open documents are searched as they stand, unsaved edits included; unopened
//! ones come from the
//! directory snapshot's [`super::docs::FontSource`], never from disk — the
//! click is on the UI thread, and the font directory is routinely a network
//! volume where one `stat` per file is already a stall. That also makes the
//! search agree with navigation, which has always read the same snapshot.

use super::*;
use crate::editor::doc_links::{LinkSpan, pattern_denotes, scan_dollar_refs};
use crate::editor::line_fields::{FieldRole, LineField, classify_line};

/// Whether `text` could write a glyph name with a leading `@`.
///
/// A name token always begins after whitespace or a backtick, and `@` is a name
/// character in first position only, so that is the whole test — and it is what
/// keeps the literal-name filters below from hiding an `@` hit. Pixel rows,
/// where `@@` is the full-ink code, contain neither, so the cheap rejection
/// that makes a search a click and not a wait still rejects them.
pub(super) fn may_write_an_at_name(text: &str) -> bool {
    text.char_indices().any(|(i, c)| {
        c == '@'
            && text[..i]
                .chars()
                .next_back()
                .is_some_and(|p| p.is_whitespace() || p == '`')
    })
}

/// The keywords whose line can name a glyph — the ones `classify_line` reads a
/// `GlyphDef`/`GlyphRef` off. Nothing else can hold a name pattern, which is
/// what makes the filter below cheap.
const GLYPH_NAME_KEYWORDS: [&str; 7] = [
    "glyph",
    "ref",
    "map",
    "remap",
    "assert",
    "assume",
    "exclude-from-sample",
];

/// Whether `text` — one line, or a whole file — could write a glyph name as a
/// *pattern* rather than in full.
///
/// The literal filters cannot see a pattern hit: `fo(o|q)` denotes `foo` while
/// containing neither `foo` nor anything derivable from it, so a line that
/// might carry one has to be tokenized. This keeps that from costing a pass
/// over every pixel row: `(`, `|` and `*` are all *shape codes* too, so a
/// metacharacter alone says nothing, and only a line whose first token is a
/// keyword that names a glyph can be a pattern.
pub(super) fn may_write_a_pattern(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start();
        let keyword = line.split_ascii_whitespace().next().unwrap_or_default();
        GLYPH_NAME_KEYWORDS.contains(&keyword)
            && line[keyword.len()..].contains(['(', '|', '$', '*'])
    })
}

/// One matched token on a line: where it is written, and whether the role it
/// was matched in *declares* the name rather than referring to it.
///
/// The distinction is `line_fields`' own `…Def`/`…Ref` split, so what the pane
/// calls a declaration is what a Ctrl/Cmd+click calls a definition. Two kinds
/// have no declaration site at all — an anchor is matched by name across
/// glyphs, a feature tag is stated once per target — and every appearance of
/// those is a use, which leaves the pane's grouping to say nothing rather than
/// to claim each line declares the thing.
///
/// Ordered by column first, so sorting a line's matches is still positional.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(super) struct MatchSpan {
    pub col_start: usize,
    pub col_end: usize,
    pub is_decl: bool,
}

/// Char-column spans on `line` at which `name` appears in the role `kind`
/// names — the whole written token, so the search pane can highlight exactly
/// what it matched (an anchor's sign, a quoted token's backticks and a pattern's
/// alternatives included).
///
/// The cheap tests in front are what keep a search a click and not a wait:
/// classifying a line costs a tokenizing pass, and a font directory is mostly
/// pixel rows that can never match; over `font/` they took 9.9 ms → 1.7 ms.
/// Most kinds' names occur **literally** in the source — a name-parts name
/// carries its own `$`, and an anchor's sign only ever precedes the name — so
/// the substring test cannot hide those. The two ways a glyph name can be
/// written *without* occurring literally each have their own filter beside it,
/// [`may_write_an_at_name`] and [`may_write_a_pattern`]. `search_name` leans on
/// the same three tests one level up, per file.
///
/// The returned span is the written token as a whole, which the pane highlights
/// so a long `remap` or `assert` row says where on it the name actually is.
///
/// `at_base` is the `@` base in force on this line — see
/// [`crate::document::at_base_at_line`] for the rule, which the walkers below
/// carry along as they go rather than re-deriving per line.
pub(super) fn match_spans(
    line: &str,
    name: &str,
    kind: LinkTargetKind,
    at_base: Option<&str>,
    name_parts: &NamePartsMap,
) -> Vec<MatchSpan> {
    let at_possible =
        kind == LinkTargetKind::Glyph && at_base.is_some() && may_write_an_at_name(line);
    let pattern_possible = kind == LinkTargetKind::Glyph && may_write_a_pattern(line);
    if !line.contains(name) && !at_possible && !pattern_possible {
        return Vec::new();
    }
    fn span(f: &LineField, is_decl: bool) -> MatchSpan {
        MatchSpan {
            col_start: f.col_start,
            col_end: f.col_end,
            is_decl,
        }
    }
    let mut cols = Vec::new();
    for f in classify_line(line) {
        match kind {
            // A name-parts variable appears *inside* other tokens, so the
            // column is the `$var`'s own, not the token's.
            LinkTargetKind::NameParts => match f.role {
                FieldRole::NamePartsDef if f.token == name => cols.push(span(&f, true)),
                FieldRole::GlyphDef | FieldRole::GlyphRef | FieldRole::NamePartsValue => {
                    let mut spans: Vec<LinkSpan> = Vec::new();
                    scan_dollar_refs(&f.token, f.col_start, &mut spans);
                    cols.extend(spans.into_iter().filter(|s| s.target == name).map(|s| {
                        MatchSpan {
                            col_start: s.col_start,
                            col_end: s.col_end,
                            is_decl: false,
                        }
                    }));
                }
                _ => {}
            },
            LinkTargetKind::Glyph => {
                if matches!(f.role, FieldRole::GlyphDef | FieldRole::GlyphRef) {
                    let is_def = f.role == FieldRole::GlyphDef;
                    let written = crate::document::expand_at_name(&f.token, at_base);
                    if written == name || pattern_denotes(&written, is_def, name, name_parts) {
                        cols.push(span(&f, is_def));
                    }
                }
            }
            LinkTargetKind::Color => {
                if matches!(f.role, FieldRole::ColorDef | FieldRole::ColorRef) && f.token == name {
                    cols.push(span(&f, f.role == FieldRole::ColorDef));
                }
            }
            LinkTargetKind::Remap => {
                if matches!(f.role, FieldRole::RemapGroupDef | FieldRole::RemapGroupRef)
                    && f.token == name
                {
                    cols.push(span(&f, f.role == FieldRole::RemapGroupDef));
                }
            }
            // A feature tag has no declaration site: every `feature` line
            // states it again, so none of them is the one that introduces it.
            LinkTargetKind::Feature => {
                if f.role == FieldRole::FeatureDef && f.token == name {
                    cols.push(span(&f, false));
                }
            }
            LinkTargetKind::Face => {
                if matches!(f.role, FieldRole::FaceDef | FieldRole::FaceRef) && f.token == name {
                    cols.push(span(&f, f.role == FieldRole::FaceDef));
                }
            }
            LinkTargetKind::Slice => {
                if matches!(f.role, FieldRole::SliceDef | FieldRole::SliceRef) && f.token == name {
                    cols.push(span(&f, f.role == FieldRole::SliceDef));
                }
            }
            // Attachment is symmetric, so `+above` and `-above` are the same
            // anchor and both are listed without distinction — and neither
            // declares it, the anchor being matched by name across glyphs.
            LinkTargetKind::Anchor => {
                if f.role == FieldRole::PointDef
                    && f.token.strip_prefix(['+', '-']).unwrap_or(&f.token) == name
                {
                    cols.push(span(&f, false));
                }
            }
        }
    }
    cols.sort_unstable();
    cols.dedup();
    cols
}

/// Every appearance in a line list, as `(line index, match)` in **source**
/// order — the order the ordinal counts in, which is not the order the pane
/// lists them in (see [`collect_hits`]).
fn hits_in_doclines(
    lines: &[DocLine],
    name: &str,
    kind: LinkTargetKind,
    name_parts: &NamePartsMap,
) -> Vec<(usize, MatchSpan)> {
    let mut hits = Vec::new();
    let mut at_base: Option<String> = None;
    for (i, line) in lines.iter().enumerate() {
        let DocLine::Text(text) = line else { continue };
        hits.extend(
            match_spans(text, name, kind, at_base.as_deref(), name_parts)
                .into_iter()
                .map(|s| (i, s)),
        );
        // After matching, never before: a header's own `@` stands for the base
        // that was already in force, exactly as the parser reads it.
        advance_at_base(&mut at_base, text);
    }
    hits
}

/// Carry the `@` base across one source line, as
/// `document_io::derive_document` does while it walks the file.
pub(super) fn advance_at_base(at_base: &mut Option<String>, line: &str) {
    if let Ok(tokens) = crate::document_io::tokenize_tokens(line.trim())
        && tokens.first().is_some_and(|t| t == "glyph")
        && let Some(name) = tokens.get(1)
        && let Some(base) = crate::document::at_base_from_glyph_name(name)
    {
        *at_base = Some(base);
    }
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
    /// Whether this occurrence *declares* the name; see [`MatchSpan`]. The pane
    /// lists the declarations first and rules a line under them.
    pub is_decl: bool,
}

/// Builds one hit from a matched line, moving the span into the trimmed text
/// the pane displays.
fn hit(
    path: &std::path::Path,
    ordinal: usize,
    file_line: usize,
    line: &str,
    span: MatchSpan,
) -> SearchHit {
    let leading = line.chars().count() - line.trim_start().chars().count();
    SearchHit {
        path: path.to_path_buf(),
        ordinal,
        file_line,
        text: line.trim().to_string(),
        highlight: (
            span.col_start.saturating_sub(leading),
            span.col_end.saturating_sub(leading),
        ),
        is_decl: span.is_decl,
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

/// Where one searched file's text comes from.
pub(super) enum SearchText<'a> {
    /// An open buffer, unsaved edits included, with the document that maps a
    /// docline back to a file line.
    Buffer(&'a [DocLine], &'a Document),
    /// The directory snapshot's source text.
    Source(&'a str),
}

/// Every appearance of `name`, over files already in memory.
///
/// Kept free of the application and of the filesystem both, which is the point:
/// a search runs on a click, and the click must not wait on a network volume.
/// See [`super::docs::FontSource`] for where the text of an unopened file comes
/// from and how it stays current.
///
/// **Declarations come first.** What a search is usually read for is where the
/// thing *is*, and a glyph used a hundred times would otherwise bury its own
/// `glyph` line somewhere in the middle of the list. The two groups are then in
/// source order — the sort is stable and reorders nothing else — so a name
/// declared several times (an alias, a slice-qualified pair) still reads
/// file by file. The `ordinal` is assigned before the sort and so still counts
/// in source order, which is the only order [`hits_in_doclines`] can re-derive
/// it in once the file is opened.
pub(super) fn collect_hits(
    files: &[(PathBuf, SearchText<'_>)],
    name: &str,
    kind: LinkTargetKind,
    name_parts: &NamePartsMap,
) -> (Vec<SearchHit>, usize) {
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut file_count = 0usize;
    for (path, text) in files {
        let before = hits.len();
        match text {
            SearchText::Buffer(lines, doc) => {
                for (ordinal, (line_idx, span)) in hits_in_doclines(lines, name, kind, name_parts)
                    .into_iter()
                    .enumerate()
                {
                    hits.push(hit(
                        path,
                        ordinal,
                        doc.docline_file_line(line_idx),
                        lines[line_idx].as_text().unwrap_or_default(),
                        span,
                    ));
                }
            }
            SearchText::Source(content)
                if content.contains(name)
                    || (kind == LinkTargetKind::Glyph
                        && (may_write_an_at_name(content) || may_write_a_pattern(content))) =>
            {
                // Enumerated over occurrences, not over lines: a line naming
                // the same glyph twice is two rows, and the ordinal has to
                // agree with `hits_in_doclines` once the file opens.
                let mut at_base: Option<String> = None;
                let mut found: Vec<(usize, &str, MatchSpan)> = Vec::new();
                for (i, text) in content.lines().enumerate() {
                    found.extend(
                        match_spans(text, name, kind, at_base.as_deref(), name_parts)
                            .into_iter()
                            .map(|s| (i, text, s)),
                    );
                    advance_at_base(&mut at_base, text);
                }
                for (ordinal, (line_idx, text, span)) in found.into_iter().enumerate() {
                    hits.push(hit(path, ordinal, line_idx + 1, text, span));
                }
            }
            SearchText::Source(_) => {}
        }
        if hits.len() > before {
            file_count += 1;
        }
    }
    hits.sort_by_key(|h| !h.is_decl);
    (hits, file_count)
}

impl UniformApp {
    /// Lists every appearance of `name` and reveals the Search pane.
    ///
    /// Open documents are searched as they stand, including unsaved edits; the
    /// rest come from the directory snapshot's sources, so the whole search is
    /// memory-only — a click never waits on the filesystem, which on a network
    /// volume is what made it a stall rather than a search. Both pre-filter on
    /// the literal name before tokenizing anything, per file and again per line.
    pub(super) fn search_name(&mut self, ctx: &egui::Context, name: &str, kind: LinkTargetKind) {
        let paths: Vec<PathBuf> = self
            .collect_all_docs()
            .iter()
            .map(|doc| doc.path.clone())
            .collect();

        // A path here is either an open document or a snapshot document, and
        // the snapshot's sources cover the latter; a path in neither has no
        // text to search and is skipped rather than read.
        let files: Vec<(PathBuf, SearchText<'_>)> = paths
            .into_iter()
            .filter_map(|path| {
                let text = match self.open_documents.iter().find(|d| d.document.path == path) {
                    Some(doc) => SearchText::Buffer(&doc.lines, &doc.document),
                    None => SearchText::Source(&self.font_sources.get(&path)?.text),
                };
                Some((path, text))
            })
            .collect();
        let (hits, file_count) = collect_hits(&files, name, kind, &self.name_parts);
        drop(files);

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
        let Some(hit) = search.hits.get(hit_idx) else {
            return;
        };
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

        let hit = hits_in_doclines(
            &self.open_documents[idx].lines,
            &name,
            kind,
            &self.name_parts,
        )
        .get(ordinal)
        .copied();
        let Some((line, span)) = hit else {
            return;
        };
        let col = span.col_start;
        let doc = &mut self.open_documents[idx];
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

    /// The point of the snapshot sources: a file no pane is editing is searched
    /// without the filesystem being consulted at all. The path here exists
    /// nowhere on disk, so any hit can only have come from memory.
    #[test]
    fn an_unopened_file_is_searched_from_the_snapshot_source() {
        let path = PathBuf::from("/nonexistent/never-read.unf");
        let source = "glyph foo 8 16\nref bar 0 0\n";
        let files = vec![(path.clone(), SearchText::Source(source))];
        let (hits, file_count) =
            collect_hits(&files, "bar", LinkTargetKind::Glyph, &NamePartsMap::new());
        assert_eq!(file_count, 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, path);
        assert_eq!(hits[0].file_line, 2);
        assert_eq!(hits[0].text, "ref bar 0 0");
    }

    /// Declarations are listed before uses, and each group keeps the order the
    /// files and their lines were walked in.
    #[test]
    fn declarations_are_listed_before_uses() {
        let one = "ref foo 0 0\nglyph foo 8 16\nmap A = foo\nglyph bar = foo\n";
        let two = "glyph foo = baz\n";
        let files = vec![
            (PathBuf::from("one.unf"), SearchText::Source(one)),
            (PathBuf::from("two.unf"), SearchText::Source(two)),
        ];
        let (hits, file_count) =
            collect_hits(&files, "foo", LinkTargetKind::Glyph, &NamePartsMap::new());
        assert_eq!(file_count, 2);
        assert_eq!(
            hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
            vec![
                "glyph foo 8 16",
                "glyph foo = baz",
                "ref foo 0 0",
                "map A = foo",
                "glyph bar = foo",
            ],
        );
        assert_eq!(
            hits.iter().map(|h| h.is_decl).collect::<Vec<_>>(),
            vec![true, true, false, false, false],
        );
    }

    /// A path the snapshot has no source for contributes nothing rather than
    /// sending the search back to disk for it.
    #[test]
    fn a_file_with_no_source_contributes_no_hits() {
        let files: Vec<(PathBuf, SearchText)> = Vec::new();
        let (hits, file_count) =
            collect_hits(&files, "bar", LinkTargetKind::Glyph, &NamePartsMap::new());
        assert!(hits.is_empty());
        assert_eq!(file_count, 0);
    }

    /// Start columns only; the spans' ends are pinned separately, by the
    /// highlight tests.
    fn cols(line: &str, name: &str, kind: LinkTargetKind) -> Vec<usize> {
        cols_with(line, name, kind, &NamePartsMap::new())
    }

    fn cols_with(line: &str, name: &str, kind: LinkTargetKind, parts: &NamePartsMap) -> Vec<usize> {
        match_spans(line, name, kind, None, parts)
            .into_iter()
            .map(|s| s.col_start)
            .collect()
    }

    #[test]
    fn glyph_name_is_found_where_it_is_defined_and_used() {
        assert_eq!(
            cols("glyph foo 8 16", "foo", LinkTargetKind::Glyph),
            vec![6]
        );
        assert_eq!(cols("ref foo 0 0", "foo", LinkTargetKind::Glyph), vec![4]);
        assert_eq!(cols("map A = foo", "foo", LinkTargetKind::Glyph), vec![8]);
        assert_eq!(
            cols("glyph bar = foo", "foo", LinkTargetKind::Glyph),
            vec![12]
        );
        assert_eq!(
            cols("remap liga : foo -> bar", "foo", LinkTargetKind::Glyph),
            vec![13],
        );
        assert_eq!(
            cols("assert same foo bar", "foo", LinkTargetKind::Glyph),
            vec![12],
        );
    }

    /// A name written as a pattern is an appearance of every name it denotes,
    /// wherever the pattern stands — the definition, a `ref`, an operand.
    #[test]
    fn a_pattern_token_that_denotes_the_name_is_an_appearance() {
        for (line, col) in [
            ("glyph fo(o|q) 8 16", 6),
            ("ref fo(o|q) 0 0", 4),
            ("remap liga : fo(o|q) -> bar", 13),
            ("glyph foo|bar 8 16", 6),
        ] {
            assert_eq!(
                cols(line, "foo", LinkTargetKind::Glyph),
                vec![col],
                "{line}"
            );
        }
        assert_eq!(
            cols(
                "glyph uni($#0041..0043) 8 16",
                "uni0042",
                LinkTargetKind::Glyph
            ),
            vec![6],
        );
    }

    /// The cyclic expansion is what decides it: `(a|b)-(1|2)` is `a-1` and
    /// `b-2`, so `a-2` is not one of its names and its line is not a hit.
    #[test]
    fn a_pattern_that_does_not_denote_the_name_is_not_an_appearance() {
        assert!(cols("glyph fo(p|q) 8 16", "foo", LinkTargetKind::Glyph).is_empty());
        assert!(cols("glyph (a|b)-(1|2) 2 2", "a-2", LinkTargetKind::Glyph).is_empty());
        assert_eq!(
            cols("glyph (a|b)-(1|2) 2 2", "b-2", LinkTargetKind::Glyph),
            vec![6]
        );
    }

    /// A pattern spelled with a `$var` denotes what the name parts say it
    /// does, so the search has to substitute them exactly as the pipeline does.
    #[test]
    fn a_name_part_is_substituted_before_the_pattern_is_matched() {
        let mut parts = NamePartsMap::new();
        parts.insert("$init".to_string(), vec!["g".to_string(), "n".to_string()]);
        assert_eq!(
            cols_with(
                "glyph hangul-($init) 8 16",
                "hangul-n",
                LinkTargetKind::Glyph,
                &parts
            ),
            vec![6],
        );
        assert!(
            cols_with(
                "glyph hangul-($init) 8 16",
                "hangul-d",
                LinkTargetKind::Glyph,
                &parts
            )
            .is_empty()
        );
        // With no parts in force the reference expands to nothing, and a
        // pattern that denotes no name is no appearance.
        assert!(
            cols(
                "glyph hangul-($init) 8 16",
                "hangul-n",
                LinkTargetKind::Glyph
            )
            .is_empty()
        );
    }

    /// The whole pattern token is the span, so the pane highlights what the
    /// line actually says rather than the name that was searched for.
    #[test]
    fn a_pattern_hit_highlights_the_whole_pattern_token() {
        let line = "    ref fo(o|q) 0 0";
        let span = match_spans(
            line,
            "foo",
            LinkTargetKind::Glyph,
            None,
            &NamePartsMap::new(),
        )[0];
        let h = hit(std::path::Path::new("a.unf"), 0, 1, line, span);
        assert_eq!(&h.text[h.highlight.0..h.highlight.1], "fo(o|q)");
    }

    /// The cheap filters in front decide whether a line is tokenized at all,
    /// and a pixel row must still be rejected — `(`, `|` and `*` are shape
    /// codes as much as they are pattern syntax.
    #[test]
    fn only_a_keyword_line_can_be_carrying_a_pattern() {
        assert!(may_write_a_pattern("glyph fo(o|q) 8 16"));
        assert!(may_write_a_pattern("  ref hangul-($init) 0 0"));
        assert!(may_write_a_pattern("assume unused foo*3"));
        assert!(!may_write_a_pattern("(((|.@@bb"));
        assert!(!may_write_a_pattern("glyph foo 8 16"));
        assert!(!may_write_a_pattern("color red = #ff0000"));
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
        assert_eq!(
            cols("remap foo : a -> b", "foo", LinkTargetKind::Remap),
            vec![6]
        );
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
            cols(
                "feature ccmp for cyrl/SRB : g",
                "ccmp",
                LinkTargetKind::Feature
            ),
            vec![8],
        );
        // The group it points at is not the tag.
        assert!(
            cols(
                "feature liga for latn : ccmp",
                "ccmp",
                LinkTargetKind::Feature
            )
            .is_empty()
        );
    }

    /// Both signs of an anchor are the same anchor, and the anchor-driven
    /// `feature` variant names one too.
    #[test]
    fn an_anchor_is_found_through_both_signs() {
        assert_eq!(
            cols("anchor +above 4 1", "above", LinkTargetKind::Anchor),
            vec![7]
        );
        assert_eq!(
            cols("anchor -above 2 1", "above", LinkTargetKind::Anchor),
            vec![7]
        );
        assert_eq!(
            cols(
                "feature abvm for hang : anchor above",
                "above",
                LinkTargetKind::Anchor
            ),
            vec![31],
        );
    }

    #[test]
    fn a_name_parts_variable_is_found_inside_the_names_it_builds() {
        assert_eq!(
            cols(
                "name-parts $init = a b c",
                "$init",
                LinkTargetKind::NameParts
            ),
            vec![11],
        );
        assert_eq!(
            cols(
                "name-parts $combo = $init $final",
                "$init",
                LinkTargetKind::NameParts
            ),
            vec![20],
        );
        assert_eq!(
            cols(
                "glyph hangul-($init)-l 8 16",
                "$init",
                LinkTargetKind::NameParts
            ),
            vec![14],
        );
        assert_eq!(
            cols("ref hangul-$init 0 0", "$init", LinkTargetKind::NameParts),
            vec![11],
        );
        // No partial matches: `$initial` is a different variable.
        assert!(
            cols(
                "ref hangul-$initial 0 0",
                "$init",
                LinkTargetKind::NameParts
            )
            .is_empty()
        );
    }

    #[test]
    fn a_color_is_found_at_its_definition_and_its_uses() {
        assert_eq!(
            cols("color red = #ff0000", "red", LinkTargetKind::Color),
            vec![6]
        );
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
        let span = match_spans(
            line,
            "foo",
            LinkTargetKind::Glyph,
            None,
            &NamePartsMap::new(),
        )[0];
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
            (
                "anchor +above 4 1",
                "above",
                LinkTargetKind::Anchor,
                "+above",
            ),
            (
                "ref `foo bar` 0 0",
                "foo bar",
                LinkTargetKind::Glyph,
                "`foo bar`",
            ),
            (
                "glyph x-$init 2 2",
                "$init",
                LinkTargetKind::NameParts,
                "$init",
            ),
        ] {
            let span = *match_spans(line, name, kind, None, &NamePartsMap::new())
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
        let spans = match_spans(
            line,
            "foo",
            LinkTargetKind::Glyph,
            None,
            &NamePartsMap::new(),
        );
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
            hits_in_doclines(&lines, "foo", LinkTargetKind::Glyph, &NamePartsMap::new())
                .into_iter()
                .map(|(i, s)| (i, s.col_start, s.col_end, s.is_decl))
                .collect::<Vec<_>>(),
            vec![(0, 6, 9, true), (2, 4, 7, false), (3, 8, 11, false)],
        );
    }

    /// A glyph written with `@` is an appearance of the name it expands to, so
    /// the Search pane lists it beside the full-name ones. The literal filters
    /// in front cannot hide it: `may_write_an_at_name` is what lets an `@` line
    /// through, and a pixel row — where `@@` is the full-ink code — still does
    /// not pay for a tokenizing pass.
    #[test]
    fn an_at_name_is_an_appearance_of_what_it_expands_to() {
        let path = PathBuf::from("/nonexistent/never-read.unf");
        let source = "glyph foo\nref @-bar\nglyph @-bar\nmap A = foo-bar\n";
        let files = vec![(path, SearchText::Source(source))];
        let (hits, _) = collect_hits(
            &files,
            "foo-bar",
            LinkTargetKind::Glyph,
            &NamePartsMap::new(),
        );
        assert_eq!(
            hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
            // The `glyph` line is a declaration and so is listed first.
            vec!["glyph @-bar", "ref @-bar", "map A = foo-bar"],
        );
        // And the base itself is not one of its own family's appearances.
        let files = vec![(PathBuf::from("x.unf"), SearchText::Source(source))];
        let (hits, _) = collect_hits(&files, "foo", LinkTargetKind::Glyph, &NamePartsMap::new());
        assert_eq!(
            hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
            vec!["glyph foo"],
        );
    }

    #[test]
    fn only_a_token_start_counts_as_an_at_name() {
        assert!(may_write_an_at_name("ref @-bar"));
        assert!(may_write_an_at_name("glyph `@ odd`"));
        // A pixel row is all shape codes, and `@@` is one of them.
        assert!(!may_write_an_at_name("@@..@@.."));
        assert!(!may_write_an_at_name("glyph foo"));
    }
}
