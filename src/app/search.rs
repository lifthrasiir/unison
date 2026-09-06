//! The Search pane: every place a name — or a piece of text — appears.
//!
//! Two ways in. The pane's own header row runs whatever is typed into it under
//! the kind its dropdown names ([`SearchKind`]), and a Ctrl/Cmd+click in the
//! editor runs a name search without going through the box. Both end in the
//! same [`SearchResults`], and the rest of this module knows only the kind.
//!
//! **Adding a kind** is [`SearchKind`]'s variant, its arm in [`LineCarry::step`]
//! (what one line matches), its arm in [`SearchKind::label`], and an entry in
//! [`SearchKind::CHOICES`] if the dropdown is to offer it. Everything else —
//! the walk over files, the ordinals, the navigation — is kind-agnostic. A kind
//! reachable only from a Ctrl/Cmd+click stays out of `CHOICES`; the pane still
//! displays it, and the Ctrl/Cmd+F cycle steps out of it into the list.
//!
//! [`SearchKind::Name`] is what a Ctrl/Cmd+click does when "go to definition"
//! has nowhere to
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
//!
//! Either way the two ends have to divide a file into lines identically, and
//! they do not divide it the same way the filesystem does: a glyph's pixel rows
//! are one grid line to the caret, and no search reaches inside one. That split
//! is [`crate::document_io::walk_source_lines`], which both ends go through —
//! for a name search it changes nothing (a pixel row can hold no name), but a
//! verbatim text search would otherwise match `@@.@` in a closed file and lose
//! the row the moment the file opened, throwing every later ordinal off.

use super::*;
use crate::document_io::SourceLine;
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
const GLYPH_NAME_KEYWORDS: [&str; 6] = ["glyph", "ref", "map", "remap", "assert", "assume"];

/// Whether `text` — one line, or a whole file — could write a glyph name as a
/// *pattern* rather than in full.
///
/// The literal filters cannot see a pattern hit: `fo(o|q)` denotes `foo` while
/// containing neither `foo` nor anything derivable from it, so a line that
/// might carry one has to be tokenized. This keeps that from costing a pass
/// over every pixel row: `(`, `|` and `*` are all *shape codes* too, so a
/// metacharacter alone says nothing, and only a line whose first token is a
/// keyword that names a glyph can be a pattern.
/// Whether `text` could write a `$N` capture slot on a line an `exists`
/// governs.
///
/// The filter beside [`may_write_a_pattern`] rather than folded into it: that
/// one asks whether a line *is* a pattern, which a scoped line need not be —
/// `ref ($0)` is, but the question is asked of every body line of the block,
/// pixel rows included, and `$` is not a pixel code. So this is the same shape
/// of cheap rejection and stays as cheap.
pub(super) fn may_write_a_capture(text: &str) -> bool {
    text.as_bytes()
        .windows(2)
        .any(|w| w[0] == b'$' && w[1].is_ascii_digit())
}

pub(super) fn may_write_a_pattern(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start();
        let keyword = line.split_ascii_whitespace().next().unwrap_or_default();
        GLYPH_NAME_KEYWORDS.contains(&keyword)
            && line[keyword.len()..].contains(['(', '|', '$', '*'])
    })
}

/// What a search looks for — the pane's kind dropdown, and the one thing every
/// other part of the search is parameterized by.
///
/// See the module note for what adding a variant costs. The two the dropdown
/// offers are the two a reader asks for by typing; the rest of
/// [`LinkTargetKind`] arrives only through a Ctrl/Cmd+click on a token that
/// already says which role it is in, so offering them in a box that cannot say
/// so would be offering a search nobody can spell.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum SearchKind {
    /// A verbatim substring of the source text: no case folding, no whitespace
    /// collapsing, no tokenizing. What is typed is what is looked for, which is
    /// the only behaviour that needs no explaining when it finds nothing.
    #[default]
    Text,
    /// Every appearance of a name in the role the [`LinkTargetKind`] names,
    /// matched by what a token *denotes* rather than by how it is written; see
    /// [`match_spans`].
    Name(LinkTargetKind),
}

impl SearchKind {
    /// What the dropdown offers, in the order Ctrl/Cmd+F steps through it.
    pub(super) const CHOICES: [SearchKind; 2] =
        [SearchKind::Text, SearchKind::Name(LinkTargetKind::Glyph)];

    /// The dropdown's text, and the noun the results header opens with.
    pub(super) fn label(self) -> &'static str {
        match self {
            SearchKind::Text => "Text",
            SearchKind::Name(LinkTargetKind::Glyph) => "Glyph",
            SearchKind::Name(LinkTargetKind::NameParts) => "Name parts",
            SearchKind::Name(LinkTargetKind::Color) => "Color",
            SearchKind::Name(LinkTargetKind::Remap) => "Remap group",
            SearchKind::Name(LinkTargetKind::Feature) => "Feature",
            SearchKind::Name(LinkTargetKind::Anchor) => "Anchor",
            SearchKind::Name(LinkTargetKind::Face) => "Face",
            SearchKind::Name(LinkTargetKind::Slice) => "Slice",
        }
    }

    /// One step through [`CHOICES`](Self::CHOICES), wrapping at both ends.
    ///
    /// A kind a Ctrl/Cmd+click left behind is not in the list at all, and steps
    /// *into* it from whichever end the direction comes from — the alternative
    /// is a chord that appears to do nothing.
    pub(super) fn cycled(self, forward: bool) -> SearchKind {
        let n = Self::CHOICES.len();
        let step = if forward { 1 } else { n - 1 };
        match Self::CHOICES.iter().position(|&k| k == self) {
            Some(i) => Self::CHOICES[(i + step) % n],
            None if forward => Self::CHOICES[0],
            None => Self::CHOICES[n - 1],
        }
    }
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
/// carry along as they go rather than re-deriving per line. `exists` is the
/// search in force, carried the same way ([`crate::exists::Carry`]): a `($1)`
/// on a scoped line names what that search matched, and nothing on the line
/// itself says so. `block_captures` is the third of them — the groups the
/// enclosing `glyph` header wrote, which a `$-N` on a `ref` line names; a line
/// that writes a leading pattern of its own (an alias, a `map`) binds them
/// itself instead, which is what [`line_captures`] decides.
pub(super) fn match_spans(
    line: &str,
    name: &str,
    kind: LinkTargetKind,
    at_base: Option<&str>,
    exists: Option<&str>,
    block_captures: &[Vec<String>],
    name_parts: &NamePartsMap,
) -> Vec<MatchSpan> {
    let at_possible =
        kind == LinkTargetKind::Glyph && at_base.is_some() && may_write_an_at_name(line);
    let pattern_possible = kind == LinkTargetKind::Glyph
        && (may_write_a_pattern(line) || (exists.is_some() && may_write_a_capture(line)));
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
    let own_captures;
    let captures = match line_captures(line, at_base, name_parts) {
        Some(own) => {
            own_captures = own;
            &own_captures[..]
        }
        None => block_captures,
    };
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
                    if written == name
                        || pattern_denotes(&written, is_def, name, name_parts, exists, captures)
                    {
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

/// Char-column spans at which `needle` occurs verbatim on `line`.
///
/// Occurrences do not overlap — the scan resumes past the one it just took —
/// which is what makes "the 3rd hit in this file" the same thing to a reader
/// counting rows and to [`hits_in_doclines`] re-deriving them after the file
/// opens. An empty needle matches nothing rather than everywhere.
fn text_spans(line: &str, needle: &str) -> Vec<MatchSpan> {
    if needle.is_empty() {
        return Vec::new();
    }
    let width = needle.chars().count();
    let mut spans = Vec::new();
    let mut from = 0usize;
    while let Some(off) = line[from..].find(needle) {
        let at = from + off;
        let col_start = line[..at].chars().count();
        spans.push(MatchSpan {
            col_start,
            col_end: col_start + width,
            // Nothing about a substring says it declares anything, so a text
            // search's rows are one group and the pane rules no line.
            is_decl: false,
        });
        from = at + needle.len();
    }
    spans
}

/// What a walk over one file carries from line to line: everything a name match
/// needs that the line itself does not state. A text search carries none of it,
/// and pays for none of it.
#[derive(Default)]
struct LineCarry {
    at_base: Option<String>,
    exists: crate::exists::Carry,
    captures: Vec<Vec<String>>,
}

impl LineCarry {
    /// Matches one text line under `kind`, then steps the carry over it.
    fn step(
        &mut self,
        text: &str,
        query: &str,
        kind: SearchKind,
        name_parts: &NamePartsMap,
    ) -> Vec<MatchSpan> {
        let SearchKind::Name(kind) = kind else {
            return text_spans(text, query);
        };
        self.exists.enter(text);
        let spans = match_spans(
            text,
            query,
            kind,
            self.at_base.as_deref(),
            self.exists.pattern(),
            &self.captures,
            name_parts,
        );
        // After matching, never before: a header's own `@` stands for the base
        // that was already in force, exactly as the parser reads it.
        advance_at_base(&mut self.at_base, text);
        advance_block_captures(
            &mut self.captures,
            text,
            self.at_base.as_deref(),
            name_parts,
        );
        spans
    }
}

/// Every appearance in a line list, as `(line index, match)` in **source**
/// order — the order the ordinal counts in, which is not the order the pane
/// lists them in (see [`collect_hits`]).
fn hits_in_doclines(
    lines: &[DocLine],
    query: &str,
    kind: SearchKind,
    name_parts: &NamePartsMap,
) -> Vec<(usize, MatchSpan)> {
    let mut hits = Vec::new();
    let mut carry = LineCarry::default();
    for (i, line) in lines.iter().enumerate() {
        // A grid is not a line anything can match or the caret can land in, and
        // it is not stepped either: it is inside the block an `exists` governs
        // and changes nothing about it — see `Carry::enter`. The source walk
        // skips exactly the same rows, which is what keeps the ordinals equal.
        let DocLine::Text(text) = line else { continue };
        hits.extend(
            carry
                .step(text, query, kind, name_parts)
                .into_iter()
                .map(|s| (i, s)),
        );
    }
    hits
}

/// The groups a line binds *itself*, for the `$-N` written further along it —
/// `None` when it binds none and the enclosing block's are the ones in force.
///
/// A `glyph NAME = TARGET` and a `map CHAR = GLYPH` each write their pattern
/// and name it again on the same line, so the binding never outlives them. A
/// `glyph` block header is the other shape: what it writes is named on the
/// `ref` lines *below* it, which is [`advance_block_captures`]'s job.
fn line_captures(
    line: &str,
    at_base: Option<&str>,
    name_parts: &NamePartsMap,
) -> Option<Vec<Vec<String>>> {
    if !line.contains('(') {
        return None;
    }
    let tokens = crate::document_io::tokenize_tokens(line.trim()).ok()?;
    let (keyword, rest) = tokens.split_first()?;
    match keyword.as_str() {
        "glyph" if rest.get(1).is_some_and(|t| t == "=") => Some(crate::pattern::capture_groups(
            &crate::document::substitute_name_parts(
                &crate::document::expand_at_name(&rest[0], at_base),
                name_parts,
            ),
        )),
        "map" => {
            // The `SLICE :` qualifier first, so what is left is the arity the
            // unqualified form has — as `line_fields` reads it.
            let rest = match rest.get(1) {
                Some(colon) if colon == ":" => &rest[2..],
                _ => rest,
            };
            let rest = match rest.first() {
                Some(g) if g == "generate" => &rest[1..],
                _ => rest,
            };
            let eq = rest.iter().position(|t| t == "=")?;
            let (base, selector) = match &rest[..eq] {
                [base] => (base, None),
                [base, selector] => (base, Some(selector.as_str())),
                _ => return None,
            };
            Some(crate::render::ttf_builder::map_char_captures(
                base, selector,
            ))
        }
        _ => None,
    }
}

/// Carry the groups a `glyph` block header wrote down to the lines under it,
/// which is where a `$-N` back-reference names them. A header that writes none
/// (and an alias line, which binds its own) clears what the last one left.
pub(super) fn advance_block_captures(
    captures: &mut Vec<Vec<String>>,
    line: &str,
    at_base: Option<&str>,
    name_parts: &NamePartsMap,
) {
    if let Ok(tokens) = crate::document_io::tokenize_tokens(line.trim())
        && tokens.first().is_some_and(|t| t == "glyph")
    {
        *captures = match tokens.get(1) {
            Some(name) if tokens.get(2).is_none_or(|t| t != "=") => {
                crate::pattern::capture_groups(&crate::document::substitute_name_parts(
                    &crate::document::expand_at_name(name, at_base),
                    name_parts,
                ))
            }
            _ => Vec::new(),
        };
    }
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

/// One completed search: what was asked for, and everything it found.
pub(super) struct SearchResults {
    /// The name or the text, exactly as it was searched for.
    pub query: String,
    pub kind: SearchKind,
    pub hits: Vec<SearchHit>,
    pub file_count: usize,
}

/// The Search pane's whole state: what the box holds, what the last run found,
/// and where in it the reader has got to.
///
/// One struct rather than fields on [`UniformApp`] because the header row, the
/// Ctrl/Cmd+F chords and the Ctrl/Cmd+G steps all read and write the same few
/// things, and every one of them is meaningless without the others.
#[derive(Default)]
pub(super) struct SearchState {
    /// What the kind dropdown shows. Not necessarily `results`' kind: changing
    /// it does not re-run anything, so what is listed is what was last *run*.
    pub kind: SearchKind,
    /// The text in the box.
    pub query: String,
    /// The last run's results; `None` before anything has been searched for.
    pub results: Option<SearchResults>,
    /// Which hit the caret was last put on, and so what Ctrl/Cmd+G steps from.
    /// `None` for a run nothing has been navigated to yet.
    pub current: Option<usize>,
    /// The line at the right of the header row — why the last thing asked for
    /// produced no jump. Cleared by the next run.
    pub message: Option<String>,
    /// The box is to take the keyboard focus on the next frame it is drawn.
    /// Set by the chord, cleared by the row that acts on it.
    pub focus_query: bool,
    /// …and with everything in it selected, so the first keystroke replaces the
    /// query rather than appending to it. Only when the focus is *arriving*:
    /// the chord that steps the kind of a box already focused must not throw
    /// away the caret the reader put there.
    pub select_query: bool,
    /// Whether the box held the focus on the last frame it was drawn — false
    /// while the tab is hidden. What tells a Ctrl/Cmd+F that opens the pane
    /// from one that steps the kind; the chord is read before the panel of the
    /// same frame is laid out, so this is the only answer available.
    pub query_focused: bool,
}

impl SearchState {
    pub(super) fn hits(&self) -> &[SearchHit] {
        self.results.as_ref().map_or(&[], |r| &r.hits[..])
    }

    /// The header row's `n/m`: which hit the caret is on, out of how many.
    pub(super) fn counter(&self) -> String {
        let total = self.hits().len();
        match self.current {
            Some(i) => format!("{}/{total}", i + 1),
            None => format!("–/{total}"),
        }
    }

    /// The rest of what the last run found, beside the counter on the header
    /// row: how many files those hits are spread over. The count itself is
    /// [`counter`](Self::counter)'s already, and the query is in the box, so
    /// this is the whole of what neither already says.
    pub(super) fn summary(&self) -> Option<String> {
        let results = self.results.as_ref()?;
        if results.hits.is_empty() {
            return None;
        }
        Some(format!(
            "in {} file{}",
            results.file_count,
            if results.file_count == 1 { "" } else { "s" },
        ))
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

/// Whether `content` — a whole file — could hold a hit at all.
///
/// The cheap rejection that keeps a search over a font directory a click and
/// not a wait: a file the literal query does not occur in is skipped without a
/// line of it being tokenized. `match_spans` leans on the same tests per line.
fn may_match(content: &str, query: &str, kind: SearchKind) -> bool {
    content.contains(query)
        || (kind == SearchKind::Name(LinkTargetKind::Glyph)
            && (may_write_an_at_name(content) || may_write_a_pattern(content)))
}

/// Every appearance of `query`, over files already in memory.
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
    query: &str,
    kind: SearchKind,
    name_parts: &NamePartsMap,
) -> (Vec<SearchHit>, usize) {
    let mut hits: Vec<SearchHit> = Vec::new();
    let mut file_count = 0usize;
    for (path, text) in files {
        let before = hits.len();
        match text {
            SearchText::Buffer(lines, doc) => {
                for (ordinal, (line_idx, span)) in hits_in_doclines(lines, query, kind, name_parts)
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
            SearchText::Source(content) if may_match(content, query, kind) => {
                // Enumerated over occurrences, not over lines: a line naming
                // the same glyph twice is two rows, and the ordinal has to
                // agree with `hits_in_doclines` once the file opens. The walk
                // skips the same pixel rows that walk does, for the same
                // reason — see the module note.
                let mut carry = LineCarry::default();
                let mut found: Vec<(usize, &str, MatchSpan)> = Vec::new();
                crate::document_io::walk_source_lines(content, |file_line, unit| {
                    let SourceLine::Text(text) = unit else { return };
                    found.extend(
                        carry
                            .step(text, query, kind, name_parts)
                            .into_iter()
                            .map(|s| (file_line, text, s)),
                    );
                });
                for (ordinal, (file_line, text, span)) in found.into_iter().enumerate() {
                    hits.push(hit(path, ordinal, file_line, text, span));
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
        // The box follows the click, so a Ctrl/Cmd+F straight afterwards edits
        // what was just searched for rather than something stale.
        self.search.kind = SearchKind::Name(kind);
        self.search.query = name.to_string();
        self.run_search(ctx);
    }

    /// Runs whatever the pane's box and dropdown now hold, and reveals the pane.
    ///
    /// It navigates nowhere: the two callers want different things afterwards —
    /// a Ctrl/Cmd+click stays where it is, the box's Enter jumps to the first
    /// hit — and both are spelled out at their own call site.
    pub(super) fn run_search(&mut self, ctx: &egui::Context) {
        let (query, kind) = (self.search.query.clone(), self.search.kind);
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
        let (hits, file_count) = collect_hits(&files, &query, kind, &self.name_parts);
        drop(files);

        self.search.results = Some(SearchResults {
            query,
            kind,
            hits,
            file_count,
        });
        self.search.current = None;
        self.search.message = None;
        let screen_h = ctx.input(|i| i.screen_rect.height());
        self.open_bottom_panel(super::panels::SEARCH_TAB, screen_h);
    }

    /// Runs the box's search and goes to its first hit, which is what Enter in
    /// the box and the Search button both do.
    ///
    /// A run that finds nothing leaves the focus where it is — in the box, with
    /// the query still there to correct — and says so on the header row. Moving
    /// the caret to the editor on a failed search would take the reader away
    /// from the one control they still have to use.
    pub(super) fn run_search_from_box(&mut self, ctx: &egui::Context) {
        self.run_search(ctx);
        if !self.search.hits().is_empty() {
            self.goto_search_hit(ctx, 0);
            return;
        }
        self.search.message = Some(if self.search.query.is_empty() {
            "Nothing to search for".to_string()
        } else {
            format!("No match for '{}'", self.search.query)
        });
        self.search.focus_query = true;
    }

    /// Ctrl/Cmd+G and Ctrl/Cmd+Shift+G: the next or previous hit of the last
    /// run, in the order the pane lists them, wrapping at both ends.
    ///
    /// Wrapping rather than stopping: the list is finite and on screen, so an
    /// end that swallows the chord says less than one that comes round again.
    pub(super) fn step_search_hit(&mut self, ctx: &egui::Context, forward: bool) {
        let n = self.search.hits().len();
        if n == 0 {
            self.search.message = Some(match &self.search.results {
                Some(r) => format!("No match for '{}'", r.query),
                None => "Nothing has been searched for yet".to_string(),
            });
            return;
        }
        let next = match self.search.current {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            // Nothing has been jumped to yet, so a step lands on whichever end
            // it is heading away from.
            None if forward => 0,
            None => n - 1,
        };
        self.goto_search_hit(ctx, next);
    }

    /// The search chords, read off the event queue before this frame's panels
    /// are laid out.
    ///
    /// Read here and *consumed* rather than left for the editor: the box is a
    /// `TextEdit`, and Escape would otherwise be its own surrender-focus and
    /// Ctrl/Cmd+F the grid's `f` shape shortcut. Taking them here is also what
    /// lets Ctrl/Cmd+F open the pane on the frame it is pressed rather than the
    /// next one — the panel that owns the box has not run yet.
    ///
    /// The Ctrl/Cmd+G step is returned rather than made, because making it
    /// opens a file and moves a caret; that belongs after this frame's editors
    /// have run, beside the click on a listed row which does the same thing.
    pub(super) fn handle_search_keys(&mut self, ctx: &egui::Context) -> Option<bool> {
        use egui::{Key, Modifiers};
        // Most specific first: `consume_key` ignores an extra Shift, so a
        // Cmd+Shift+F left to the Cmd+F arm would read as the plain chord.
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let (find_glyph, find, step_back, step, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(cmd_shift, Key::F),
                i.consume_key(cmd, Key::F),
                i.consume_key(cmd_shift, Key::G),
                i.consume_key(cmd, Key::G),
                self.search.query_focused && i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });

        if find || find_glyph {
            self.focus_search_box(ctx, find_glyph);
        }
        if escape {
            // Focus only. The pane keeps its query, its results and its place
            // in them, so a Ctrl/Cmd+G from the editor carries straight on.
            self.focus_pane_editor(ctx);
            self.search.query_focused = false;
        }
        match (step, step_back) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        }
    }

    /// Ctrl/Cmd+F and Ctrl/Cmd+Shift+F: reveal the pane with the box focused.
    ///
    /// The first press picks the kind the chord names; a press while the box
    /// *already* has the focus steps the dropdown instead, forward or back.
    /// That is the whole difference between the two, and it is why the box's
    /// focus as of the last frame is recorded — the chord is read before this
    /// frame's panel is laid out.
    pub(super) fn focus_search_box(&mut self, ctx: &egui::Context, shift: bool) {
        self.search.kind = if self.search.query_focused {
            self.search.kind.cycled(!shift)
        } else if shift {
            SearchKind::Name(LinkTargetKind::Glyph)
        } else {
            SearchKind::Text
        };
        // A box the focus is arriving at hands the reader a selected query, so
        // typing replaces it; one that already had the focus is left alone.
        self.search.select_query = !self.search.query_focused;
        self.search.focus_query = true;
        let screen_h = ctx.input(|i| i.screen_rect.height());
        self.open_bottom_panel(super::panels::SEARCH_TAB, screen_h);
    }

    /// Opens the file a listed hit is in and puts the caret on it.
    ///
    /// The search pane is not a link in a document, so there is no link
    /// position to come back to; "go back" returns to wherever the caret was
    /// left, which is the only position the user actually departed from.
    pub(super) fn goto_search_hit(&mut self, ctx: &egui::Context, hit_idx: usize) {
        let Some(search) = &self.search.results else {
            return;
        };
        let Some(hit) = search.hits.get(hit_idx) else {
            return;
        };
        let (path, ordinal) = (hit.path.clone(), hit.ordinal);
        let (name, kind) = (search.query.clone(), search.kind);
        self.search.current = Some(hit_idx);
        self.search.message = None;

        let from = self.active_doc_idx().and_then(|idx| {
            let doc = self.open_documents.get(idx)?;
            // The pane is not a position in a document, so the caret is what
            // was left — the page it was left on included.
            Some(
                NavLoc::new(
                    idx,
                    doc.editor_state.cursor.line,
                    doc.editor_state.cursor.col,
                )
                .seen_at(doc.editor_state.caret_view_offset),
            )
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
#[path = "search_tests.rs"]
mod search_tests;
