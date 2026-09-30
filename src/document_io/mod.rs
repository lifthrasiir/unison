//! Parser and serializer for the `.unf` font source format.
//!
//! The user-facing reference for the format is `doc/reference.md`; this doc
//! records only what the *parser* and *serializer* commit to, and where each directive's meaning is implemented.
//!
//! Parsing is incremental at the line level: [`crate::document::DocLine`] is
//! what the editor edits, and a pixel-only edit does not reparse the file. The
//! editor canonicalizes every file through [`serialize_document`] when it opens
//! it, so anything the model drops on the way in is something the user loses —
//! comments included (below).
//!
//! # Names
//!
//! A glyph name is letters, digits, `-`, `.`, `_` and `:`. Every character the
//! pattern syntax uses (`(`, `)`, `|`, `$`, `*`, `#`) is excluded, so a pattern
//! that failed to expand cannot reach the font as a name that merely looks odd.
//! The rule is checked against *expanded* names by [`crate::issues`]; see
//! [`crate::document::is_valid_glyph_name`]. Face and slice ids are narrower
//! still — see [`crate::faces`].
//!
//! A `glyph` header and a `ref` target may start with `@`, the last glyph name
//! declared *without* one ([`crate::document::expand_at_name`]). The
//! substitution is textual and happens before anything else reads the name, so
//! the rest of the pipeline sees an ordinary name; the written form is kept
//! beside it (`GlyphBody::raw_name`, `GlyphAlias::raw_name`/`raw_target`,
//! `GlyphRef::raw_name`) so [`serialize_document`] puts back what was written.
//!
//! `$-N` back-references and `exists` captures are substituted by the item's
//! own expansion, not here; see [`crate::pattern`] and [`crate::exists`].
//!
//! # Tokens and comments
//!
//! Whitespace-separated, with backtick quoting: `` `foo bar` ``, two backticks
//! for a literal one inside a quote, so a lone literal backtick is four.
//!
//! `//` starts a comment on every line *except* pixel rows, where `//` is a
//! legal pixel pair; see [`split_comment`] for the exact rule and why a pixel
//! row must never reach it. Comments are dropped by
//! [`tokenize_tokens`]/[`tokenize_with_spans`], so grammar, links, completion
//! and rename never see comment prose, and every item carries its own comment
//! (a `comment` field on the structured [`crate::document::DocumentItem`]
//! variants, on `GlyphBody`/`GlyphRef`/`GlyphPoint`, inline in the raw text of
//! `Meta`/`Directive`) so serializing does not lose it. Appending to a line
//! goes through `append_to_line`, which keeps the insertion in front of the
//! comment.
//!
//! # Headings
//!
//! `# TEXT`, `## TEXT`, `### TEXT` ([`DocumentItem::Heading`](crate::document::DocumentItem::Heading)): the `#` run must
//! be a token of its own, and the text after it is prose taken to the end of the
//! line the way a comment is. A heading is a second kind of comment — no build
//! stage reads one — spelled differently because the editor does read it
//! ([`crate::editor::folding`]). A fourth level is an error from
//! [`crate::issues`] rather than more of the same: the three levels plus the
//! glyph block are the four the editor nests.
//!
//! # Continuation lines
//!
//! `|| TEXT` continues the command above it (only `sample` takes one). It is a
//! line's keyword and not a mid-line escape: only whitespace may precede the
//! marker, everything after it is taken raw — no tokens, no quoting, no comment
//! — and a `||` with nothing above it to continue is an error. The whitespace
//! *every* continuation of one command shares is removed on the way in
//! ([`dedent_continuations`]), so a text whose every line is indented is read,
//! and written back, dedented; [`tokenize_strict`] never sees one, and
//! `serialize.rs`'s `sample_lines` relies on a dedented line starting at column
//! 0 for the round trip.
//!
//! # Directives
//!
//! One keyword per line, and the `SLICE :` qualifier a `map`, `feature` or
//! `name-parts` may carry (or `meta`'s `FACE :` scope) is told from the body by
//! the *second* token being a bare `:`, which no name or value can be — so
//! `map : = colon` still maps U+003A. `assert shape` takes `for SLICE...`
//! instead, since it already uses `:` as a separator, and the two mean opposite
//! things (a line stated once per slice, versus a face including all of them).
//! `editor/line_fields.rs`'s `split_qualifier` takes the qualifier off the same
//! way.
//!
//! Where each directive's meaning lives:
//!
//! - `meta` — [`crate::meta`]; `audit` — [`crate::audit`]; `face`/`slice` —
//!   [`crate::faces`]; `sample` — [`crate::samples`]; `prop` — [`crate::ucd`];
//!   `color` — `render/ttf_builder/color.rs`.
//! - `exists` — [`crate::exists`]; `name-parts` — [`crate::pattern`] and
//!   [`crate::document::SliceNameParts`] (each token of the right-hand side is
//!   itself a pattern, `resolve_name_part_values`).
//! - `map` — `render/ttf_builder/expand/` (`resolve_map_alternatives` for the
//!   ordered targets and the empty last one, `expand_uvs_map_triples` for a
//!   variation sequence, and `generate`); `Map::selector` in
//!   [`crate::document`] for the two spellings of a selector and why length
//!   stops at two.
//! - `remap`/`remap group` — `render/ttf_builder/gsub.rs` and
//!   `document/remap.rs`; `feature … : anchor … [align]` —
//!   [`crate::document::AnchorAlign`] and `render/ttf_builder/gpos.rs`.
//! - `assert` — [`crate::render::assert`]; `assume unused` — `issues/unused.rs`.
//!
//! `meta` and `audit` are single-assignment, one key per line, because their
//! keys are variadic; both say why in their own docs.
//!
//! # Glyph blocks
//!
//! `glyph NAME [W H] [flags...]`, with flags `keep`, `inline`, `mark`, `desync`,
//! `vectoronly`, `origin C R`, `advance W`, `extent W H` and `scale N`. `scale`
//! is the per-glyph sub-pixel resolution: the grid is stored N× finer, and this
//! module multiplies the declared dimensions by it but not the other flags.
//! `origin`/`advance`/`extent` state the declared box and meet in
//! [`crate::document::GlyphBody::declared_origin`] and
//! [`declared_extent`](crate::document::GlyphBody::declared_extent), which is
//! all anything downstream reads; `advance` beside `extent` is a parse error
//! (`parse_glyph_flag_parts_impl`). `desync`/`vectoronly` are the build's —
//! [`crate::render::ttf_builder`] — and `keep` on a pattern block is
//! [`crate::merge`]'s opt-out.
//!
//! With `W H`, exactly `H` pixel rows follow, two characters per pixel
//! ([`crate::pixel`]'s catalog; `$$` is [`crate::pixel::PX_HARDBLANK`]). A
//! first row that does not parse reads as "no rows"; a later one of the wrong
//! length or with an unknown pair is an error at that line.
//!
//! The lines under a header: `ref` ([`crate::ref_composite`]; `goto` is the one
//! flag no build stage reads, see `GlyphRef::goto`), `anchor`
//! ([`crate::ref_composite`] and `gpos.rs`), and the IDC lines
//! ([`crate::compose`] — each token a gap if it reads as a number, else a
//! component name, except on an enclosure where the two numbers are offsets;
//! `assume` in front of the operator is part of the line, not a directive).
//! `glyph NAME = TARGET` is an alias ([`crate::alias`]), takes no flags and no
//! body; `glyph NAME* = PREFIX*` is a multi-alias, read as the alias an
//! `exists PREFIX(.*)` would scope. NAME accepts the patterns of [`crate::pattern`], and a block expands
//! in lock-step with its `ref` and IDC patterns.
//!
//! A glyph needs a pixel grid, at least one `ref` or an IDC line to exist at
//! all. `origin`/`advance`/`extent`/`anchor` do not make one buildable, and a
//! contentless glyph never enters the resolution cache — so it is absent from
//! cmap, composites and GSUB coverage, and referring to it is an error; `keep`
//! is the one way to write a bodiless glyph (an empty outline carrying its
//! anchors). For a deliberately blank glyph, use `ref sp` or a grid with no
//! rows.

mod derive;
mod header;
#[cfg(any(feature = "editor", test))]
mod lines;
mod strict;
mod tokens;
mod write;

pub use derive::*;
#[cfg(any(feature = "editor", test))]
pub use header::*;
#[cfg(any(feature = "editor", test))]
pub use lines::*;
pub use strict::*;
pub use tokens::*;
pub use write::*;

#[cfg(test)]
#[path = "../document_io_tests/mod.rs"]
mod tests;
