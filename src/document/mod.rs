//! The `.unf` data model: [`Document`] and its [`DocumentItem`]s, [`DocLine`]
//! (the line-level model the editor actually edits), [`PixelGrid`] and the glyph
//! bodies hanging off them.
//!
//! The parser and serializer — and the reference for the surface syntax — are in
//! [`crate::document_io`]. Name expansion lives in [`crate::pattern`], whose API
//! this module re-exports for the legacy import paths that predate the split.

mod glyph;
mod name_parts;
mod names;
mod pixel_grid;
mod remap;
mod serialize;

pub use glyph::*;
pub use name_parts::*;
pub use names::*;
pub use pixel_grid::*;
pub use remap::*;

use std::path::PathBuf;

/// What a [`DocumentItem::Directive`]'s raw text means.
///
/// `document_io` keeps directives that have no typed item as raw text, so
/// every consumer used to re-parse them with its own `strip_prefix` chain and
/// its own idea of which keywords are recognized — five copies that had to be
/// kept in sync with the parser by hand. This is that knowledge, once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Directive<'a> {
    /// `exclude-from-sample NAME...` — the argument text.
    ExcludeFromSample(&'a str),
    /// `assume unused NAME...` — the argument text.
    AssumeUnused(&'a str),
    /// A `|| …` continuation line with nothing in front of it to continue.
    /// See [`crate::document_io`] (`# Continuation lines`).
    OrphanContinuation,
    /// Blank or whitespace only.
    Empty,
    /// A keyword we do not know, or a known keyword whose arguments did not
    /// parse into a typed item.
    Unrecognized,
}

/// Note: this deliberately does *not* know about the directives that parse
/// into typed items (`name-parts`, `remap`, `feature`, `color`, `assert`).
/// Those only reach [`DocumentItem::Directive`] when malformed, and are
/// reported as unrecognized so the author hears about the typo.
pub fn classify_directive(text: &str) -> Directive<'_> {
    // Raw-text directives keep their `// …` comment inline, so it has to come
    // off before the arguments are read.
    let (text, _) = crate::document_io::split_comment(text);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Directive::Empty;
    }
    if let Some(rest) = trimmed.strip_prefix("exclude-from-sample ") {
        return Directive::ExcludeFromSample(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("assume unused ") {
        return Directive::AssumeUnused(rest);
    }
    // A continuation that reached the directive list is one no `sample` line
    // claimed: the parser hands every claimed one to its own item.
    if trimmed.starts_with(crate::document_io::CONTINUATION) {
        return Directive::OrphanContinuation;
    }
    // `assert` lines that parse become typed items, so an `assert` reaching
    // here is malformed and should be reported like any other unknown line.
    Directive::Unrecognized
}

/// Every code point the `exclude-from-sample` lines of `items` name.
///
/// An argument is either a single character spelling `parse_map_char` reads or a
/// pattern `expand_map_pairs` expands, which is what makes
/// `exclude-from-sample U+AC00..D7A3` one line rather than 11,172. Both the
/// `sample.html` writer and the specimen panel ask this, so "excluded" means the
/// same set of characters in either place.
pub fn excluded_from_sample<'a>(
    items: impl IntoIterator<Item = &'a DocumentItem>,
) -> std::collections::BTreeSet<u32> {
    let mut excluded = std::collections::BTreeSet::new();
    for item in items {
        if let DocumentItem::Directive(s) = item
            && let Directive::ExcludeFromSample(rest) = classify_directive(s)
        {
            for tok in rest.split_whitespace() {
                if let Some(cp) = crate::render::ttf_builder::parse_map_char(tok) {
                    excluded.insert(cp);
                } else {
                    for (cp, _) in crate::render::ttf_builder::expand_map_pairs(tok, "") {
                        excluded.insert(cp);
                    }
                }
            }
        }
    }
    excluded
}

/// The deepest heading the format has. Three, because the editor nests one
/// group per level and a glyph block is the fourth; see
/// [`crate::document_io`] (`# Headings`).
pub const MAX_HEADING_LEVEL: u8 = 3;

#[derive(Clone, Debug, PartialEq)]
pub enum DocumentItem {
    Comment(String),
    BlankLine,
    Directive(String),
    /// `#`/`##`/`###` followed by a space and free text — a section heading.
    ///
    /// A heading carries nothing the font is built from: it is a comment as far
    /// as every build stage is concerned, and exists so the *editor* can fold a
    /// file by its sections and show them as landmarks. See
    /// [`crate::editor::folding`] for the grouping and
    /// [`crate::document_io`] (`# Headings`) for the syntax.
    ///
    /// `level` is the number of `#` as written, so a level past 3 survives
    /// parsing to be reported by [`crate::issues`] rather than silently read as
    /// something else. `text` is everything after the `#` run, comment
    /// included, so serializing is lossless — like [`DocumentItem::Meta`].
    Heading {
        level: u8,
        text: String,
    },
    /// `meta [FACE :] KEY VALUE...` — one key per line. Holds the text after
    /// the keyword, comment included, so serializing is lossless.
    Meta(String),
    /// `audit KEY ARGUMENT...` — a rule the source is held to rather than a
    /// value the font carries; stored like [`DocumentItem::Meta`], and read by
    /// [`crate::audit`].
    Audit(String),
    /// `exists PATTERN` — a *search* over the glyph names the source declares,
    /// binding the item written on the very next line and repeating it once per
    /// match, with `$0` the matched name and `$1`… its captures.
    ///
    /// The pattern is kept as written rather than compiled: a document is
    /// cloned per edit and compared for equality, and a `Regex` is neither
    /// cheap to clone nor comparable. [`crate::exists::ExistsPattern::parse`]
    /// is where it becomes one, and the errors it gives are reported by
    /// [`crate::issues`] like every other malformed line.
    ///
    /// The binding itself is *adjacency*, not nesting — the scoped item is the
    /// next `items` entry and stays an ordinary [`Glyph`](DocumentItem::Glyph)
    /// or [`Map`](DocumentItem::Map). Nesting would give the editor's folding,
    /// rename and resize paths a second kind of block to know about, for a
    /// relationship one index step already states. See [`crate::exists`].
    Exists {
        pattern: String,
        comment: Option<String>,
    },
    Glyph {
        name: GlyphName,
        body: GlyphBody,
    },
    /// `glyph NAME = TARGET` — a second name for one glyph, not a second
    /// glyph: both names end up on the same glyph id. Takes no flags, and both
    /// sides expand as name patterns in lock-step. See [`crate::alias`].
    GlyphAlias {
        name: GlyphName,
        target: String,
        /// The header name as written when it differs from `name` — an `@…`
        /// form. See [`expand_at_name`].
        raw_name: Option<String>,
        /// Likewise for `target`.
        raw_target: Option<String>,
        comment: Option<String>,
    },
    /// `face FACE [: SLICE...]` — one typeface in the output. Declaration order
    /// is the output order, which is user-visible: a consumer that does not
    /// choose a face gets the first.
    Face {
        id: String,
        slices: Vec<String>,
        comment: Option<String>,
    },
    /// `slice SLICE [= SLICE...]` — a named group of cmap, feature and
    /// assertion data. The `= ...` form is shorthand for including those
    /// slices too, transitively; it is not a precedence mechanism.
    Slice {
        id: String,
        inherits: Vec<String>,
        comment: Option<String>,
    },
    /// `[SLICE[|SLICE...] :] map CHAR[ SELECTOR] = GLYPH...` — cmap mapping
    /// from a Unicode character, or from a variation sequence, to a glyph name.
    /// `slices` is empty for the base slice, which every face includes; more
    /// than one means the line is stated once per slice, each with that slice's
    /// [`NameParts`](DocumentItem::NameParts) bindings in force.
    ///
    /// `selector` is the variation selector of a Unicode variation sequence,
    /// and `Option` rather than a list because that is the whole shape cmap
    /// format 14 can hold: a base and one selector, nothing longer. Anything
    /// longer belongs in a `remap`, and [`crate::issues`] says so in as many
    /// words rather than letting the parser truncate it.
    ///
    Map {
        slices: Vec<String>,
        char_repr: String,
        selector: Option<String>,
        /// The targets, in the order they were written: *ordered alternatives*.
        /// The first one that names a glyph the font actually has is the one
        /// the character maps to, and a name that stands for nothing is simply
        /// passed over — which is what lets one line cover a range whose glyphs
        /// come from more than one family. Never empty; a line with no target
        /// at all does not parse as a `map`.
        ///
        /// One entry *may* be the empty string, written `` `` ``: as the last
        /// alternative it says a character none of the others covered is
        /// dropped rather than faulted. Anywhere else it is an error, since it
        /// always matches.
        ///
        /// Resolved once, in
        /// [`crate::render::ttf_builder::expand`], because the choice is *per
        /// codepoint*: a target is a pattern expanded in lock-step with
        /// `char_repr`, so two characters of one line may well pick different
        /// alternatives. Everything downstream sees the resolved single-target
        /// items that pass leaves behind.
        glyphs: Vec<String>,
        comment: Option<String>,
    },
    /// `map generate CHAR [= GLYPH]` — auto-decomposed cmap mapping. The glyph
    /// is synthesized from the character's Unicode canonical decomposition and
    /// named `uniXXXX` unless `glyph` names it.
    ///
    /// `selector` exists only so that a sequence written here parses and then
    /// fails validation with a real message. It is never valid: a variation
    /// sequence has no canonical decomposition — `0030 FE0F` is its own NFD —
    /// so there is nothing for `generate` to synthesize from.
    MapDecomposed {
        slices: Vec<String>,
        char_repr: String,
        selector: Option<String>,
        glyph: Option<String>,
        comment: Option<String>,
    },
    /// `[SLICE[|SLICE...] :] name-parts $NAME = token1 token2 $ref3 ...`
    ///
    /// A slice-scoped binding takes exactly one value and is what makes a
    /// slice-varying name writable once: see [`SliceNameParts`].
    NameParts {
        slices: Vec<String>,
        name: String,
        values: Vec<String>,
        comment: Option<String>,
    },
    /// `remap FEATURE : [LOOKBEHIND... :] SOURCE -> TARGET [: LOOKAHEAD...]`
    Remap {
        feature: String,
        lookbehind: Vec<String>,
        source: Vec<String>,
        target: Vec<String>,
        lookahead: Vec<String>,
        comment: Option<String>,
    },
    /// `remap group NAME [reversed] [after GROUP]...` — declares a remap group
    /// and the properties that belong to the lookup as a whole rather than to
    /// any one rule. Optional: a group with no declaration is unreversed and
    /// unconstrained, ordered where its first rule appears.
    RemapGroup {
        name: String,
        reversed: bool,
        after: Vec<String>,
        comment: Option<String>,
    },
    /// `feature NAME for SCRIPT... : REMAP_GROUP`
    Feature {
        slices: Vec<String>,
        name: String,
        scripts: Vec<String>,
        remap_group: String,
        comment: Option<String>,
    },
    /// `feature NAME for SCRIPT... : anchor ANCHOR_NAME [align XX]`
    FeatureAnchor {
        slices: Vec<String>,
        name: String,
        scripts: Vec<String>,
        anchor: String,
        /// How both sides of this class reduce a ranged anchor to a point.
        /// The class is the only thing that may say — see [`AnchorAlign`].
        align: AnchorAlign,
        comment: Option<String>,
    },
    /// `prop block NAME = U+XXXX[..YYYY]` — a named area of the code space the
    /// source has claimed. Recorded so the claim is written down next to the
    /// characters that fill it; nothing derives anything from it yet.
    PropBlock {
        name: String,
        start: u32,
        end: u32,
        comment: Option<String>,
    },
    /// `prop CHAR [= NAME] [gc GC] [ccc N] [eaw EAW]` — Unicode character
    /// properties a source states for characters the UCD leaves blank (Private
    /// Use, mostly). `char_repr` is the same character spelling a
    /// [`Map`](DocumentItem::Map) takes and `name` the pattern expanded against
    /// it, so one line states a whole range. See [`crate::ucd`].
    PropChar {
        char_repr: String,
        name: Option<String>,
        values: crate::ucd::CharPropValues,
        comment: Option<String>,
    },
    /// `color NAME = #xxxxxx[xx]|COLORNAME [coloronly|monoonly]`
    Color {
        name: String,
        value: String,
        visibility: Option<LayerVisibility>,
        comment: Option<String>,
    },
    /// `assert shape \`text\` [@lang] [+feat] [-feat] [for SLICE...] : glyph1 [advance N] [offset X Y] : glyph2 ...`
    AssertShape {
        /// Slices a face must include for this assertion to apply to it. Empty
        /// means every face. A combination no face satisfies is an error, not a
        /// silently skipped assertion.
        slices: Vec<String>,
        text: String,
        features: Vec<ShapeFeatureFlag>,
        /// BCP 47 language the text is shaped as, from an `@tag` token.
        ///
        /// Deliberately *not* the `script/LANG` notation a `feature` directive
        /// uses: an assertion states the input a real client hands the shaper,
        /// and the OpenType language system is what the shaper is supposed to
        /// derive from it. Writing `@ROM` on both sides would make the two
        /// agree by construction and stop the assertion from noticing that
        /// Romanian does not resolve to the tag the font declared.
        language: Option<String>,
        expected: Vec<ExpectedGlyph>,
        comment: Option<String>,
    },
    /// `sample LABEL [SUBLABEL] [: MODE...]` plus the `||` continuation lines
    /// under it — a ready-made specimen text the source carries.
    ///
    /// It builds nothing: the demo page offers it in its sample panel and the
    /// editor puts it in the preview, and a font with no `sample` line at all
    /// is byte-for-byte the font it would be otherwise. Which is why it is not
    /// a `meta` key — see [`crate::meta`] — and why the two labels are prose
    /// rather than names: they are what the reader picks the text by.
    ///
    /// `label` is the heading the text is listed under and `sublabel` the entry
    /// beneath it; a line with no sublabel gives the heading *itself* a text.
    /// `mode` is the reserved `: MODE...` tail, kept as written so that the
    /// grammar is settled before there is anything to put in it — nothing is
    /// accepted yet and [`crate::issues`] says so.
    ///
    /// `text` is one entry per continuation line, already dedented (see
    /// [`crate::document_io::dedent_continuations`]); empty means the `sample`
    /// line had no `||` under it at all, which is an error `issues` reports
    /// rather than a silently empty sample.
    Sample {
        label: String,
        sublabel: Option<String>,
        mode: Vec<String>,
        text: Vec<String>,
        comment: Option<String>,
    },
    /// `assert same GLYPH1 GLYPH2 ...`
    AssertSame {
        names: Vec<String>,
        comment: Option<String>,
    },
    /// `assert distinct GLYPH1 GLYPH2 ...`
    AssertDistinct {
        names: Vec<String>,
        comment: Option<String>,
    },
}

impl DocumentItem {
    /// The glyph names a `remap` rule names, in rule order. Empty for every
    /// other item. Enumerating the four operand lists by hand is easy to get
    /// subtly wrong — a forgotten `lookahead` silently narrows a check.
    pub fn remap_operands(&self) -> impl Iterator<Item = &String> {
        let lists: [&[String]; 4] = match self {
            DocumentItem::Remap {
                source,
                target,
                lookbehind,
                lookahead,
                ..
            } => [source, target, lookbehind, lookahead],
            _ => [&[], &[], &[], &[]],
        };
        lists.into_iter().flatten()
    }

    #[cfg(feature = "editor")]
    pub fn affects_font(&self) -> bool {
        !matches!(
            self,
            DocumentItem::Comment(_)
                | DocumentItem::BlankLine
                | DocumentItem::Heading { .. }
                | DocumentItem::Directive(_)
                | DocumentItem::AssertShape { .. }
                | DocumentItem::AssertSame { .. }
                | DocumentItem::AssertDistinct { .. }
                // A sample is read by the demo page and by the preview; no
                // stage of the font build ever sees one.
                | DocumentItem::Sample { .. }
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShapeFeatureFlag {
    pub tag: String,
    pub enable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExpectedGlyph {
    pub name: String,
    pub advance: Option<i32>,
    pub offset: Option<(i32, i32)>,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub struct Document {
    pub items: Vec<DocumentItem>,
    pub item_line_starts: Vec<usize>,
    /// Maps each DocLine index to its 0-based file line number.
    pub docline_file_lines: Vec<usize>,
    pub path: PathBuf,
    pub dirty: bool,
    pub edit_gen: u64,
    pub pixel_gen: u64,
    /// Incremented only when `items` actually change (not on every keystroke).
    pub content_gen: u64,
}

impl Document {
    pub fn new(path: PathBuf) -> Self {
        Self {
            items: Vec::new(),
            item_line_starts: Vec::new(),
            docline_file_lines: Vec::new(),
            path,
            dirty: false,
            edit_gen: 0,
            pixel_gen: 0,
            content_gen: 0,
        }
    }

    /// 1-based line of `docline_idx` in the serialized file.
    pub fn docline_file_line(&self, docline_idx: usize) -> usize {
        self.docline_file_lines
            .get(docline_idx)
            .copied()
            .unwrap_or(docline_idx)
            + 1
    }

    /// `(docline, 1-based file line)` of item `item_idx`'s defining header.
    pub fn item_lines(&self, item_idx: usize) -> (usize, usize) {
        let line = self.item_line_starts.get(item_idx).copied().unwrap_or(0);
        (line, self.docline_file_line(line))
    }
}

pub fn compute_docline_file_lines(lines: &[DocLine]) -> Vec<usize> {
    let mut result = Vec::with_capacity(lines.len());
    let mut file_line = 0usize;
    for line in lines {
        result.push(file_line);
        match line {
            DocLine::Text(_) => file_line += 1,
            DocLine::Grid(grid) => {
                if !grid.is_all_empty() {
                    file_line += grid.height as usize;
                }
            }
        }
    }
    result
}

// Name pattern parsing/expansion and `$var` substitution live in
// `crate::pattern`; re-exported here because most consumers reach them
// through `crate::document`.
pub use crate::pattern::{
    MAX_EXPANSION, NamePartsMap, NamePattern, expand_name_element, find_invalid_inline_ranges,
    has_top_level_pipe, is_name_pattern, is_valid_glyph_name, parse_name_element,
    split_top_level_pipes, substitute_name_parts,
};

// ---------------------------------------------------------------------------
// Remap group ordering
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum DocLine {
    Text(String),
    Grid(PixelGrid),
}

#[cfg(feature = "editor")]
impl DocLine {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            DocLine::Text(s) => Some(s),
            DocLine::Grid(_) => None,
        }
    }

    pub fn as_text_mut(&mut self) -> Option<&mut String> {
        match self {
            DocLine::Text(s) => Some(s),
            DocLine::Grid(_) => None,
        }
    }

    #[cfg(test)]
    pub fn as_grid(&self) -> Option<&PixelGrid> {
        match self {
            DocLine::Grid(g) => Some(g),
            DocLine::Text(_) => None,
        }
    }

    pub fn char_len(&self) -> usize {
        match self {
            DocLine::Text(s) => s.chars().count(),
            DocLine::Grid(_) => 0,
        }
    }
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;
