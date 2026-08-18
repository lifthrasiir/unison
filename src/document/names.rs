//! Glyph names as the data model carries them: [`GlyphName`], the `@` prefix
//! and what it stands for, and the written form kept beside an expanded name.

#[cfg(feature = "editor")]
use super::DocLine;

/// A glyph's name with any leading `@` already expanded, which is what every
/// stage after the parser looks it up by. The written form, when it differs,
/// lives beside it (`GlyphBody::raw_name`, `DocumentItem::GlyphAlias::raw_name`)
/// so serializing puts the line back as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphName(pub String);

impl GlyphName {
    pub fn display(&self) -> String {
        self.0.clone()
    }
}

/// Expand a leading `@` in a name written inside (or as the header of) a glyph
/// block.
///
/// `@` stands for the last glyph name declared *without* one, which is what
/// lets a family of helper glyphs be named after the glyph they belong to
/// without repeating it:
///
/// ```text
/// glyph foo        // base
/// ref @-bar        // → foo-bar
/// glyph @-bar      // → foo-bar; `@` still stands for foo, not foo-bar
/// ref @-baz        // → foo-baz
/// glyph @-baz      // → foo-baz
/// ```
///
/// The base is the declared name with its `:variant` suffix taken off, so a
/// variant's helpers hang off the glyph rather than off the variant: under
/// `glyph foo:mono`, `@-bar` is `foo-bar` and the mono variant of it is
/// `@-bar:mono`. See [`at_base_from_glyph_name`].
///
/// `@` is a name character in first position only; a name is otherwise
/// unchanged, so a full name is always writable. What `@` yields is textual and
/// happens before pattern expansion, so a base that is a pattern carries
/// through (`glyph a($1..3)` + `ref @-b` → `a($1..3)-b`).
///
/// With no base in scope the written form is returned unchanged, `@` and all:
/// [`is_valid_glyph_name`] rejects it and [`crate::issues`] reports what the
/// author actually wrote.
pub fn expand_at_name(raw: &str, base: Option<&str>) -> String {
    match (raw.strip_prefix('@'), base) {
        (Some(rest), Some(base)) => format!("{base}{rest}"),
        _ => raw.to_string(),
    }
}

/// The written form to keep beside an expanded name, or `None` when the two
/// agree and there is nothing to remember.
pub fn written_form(raw: &str, expanded: &str) -> Option<String> {
    (raw != expanded).then(|| raw.to_string())
}

/// The `@` base a `glyph` header sets, or `None` for one that sets none.
///
/// A header written with `@` is a helper of the base already in force and does
/// not become a base itself — otherwise a chain of helpers would nest instead
/// of staying siblings. Everything else sets the base to its name with the
/// `:variant` suffix taken off: `foo:mono`'s helpers are `foo`'s helpers, each
/// with a `:mono` of its own, and writing them under the variant is what makes
/// that spellable. A name that is *only* a suffix leaves the base alone, having
/// nothing to offer it.
///
/// The one place this rule is written; the parser and the editor both ask here
/// so a link or a completion cannot disagree with what was built.
pub fn at_base_from_glyph_name(name: &str) -> Option<String> {
    if name.starts_with('@') {
        return None;
    }
    let base = name.split(':').next().unwrap_or(name);
    (!base.is_empty()).then(|| base.to_string())
}

/// The `@` base in force on line `line` of a buffer: the nearest `glyph` header
/// *above* it whose name carries no `@` of its own.
///
/// Above and not at, because a header's own `@` expands against the base that
/// was already in force — the same rule `document_io::derive_document` applies
/// while it walks the file, which is what lets the editor's links and
/// completion agree with what the parser built.
#[cfg(feature = "editor")]
pub fn at_base_at_line(lines: &[DocLine], line: usize) -> Option<String> {
    lines[..line.min(lines.len())]
        .iter()
        .rev()
        .filter_map(|l| l.as_text())
        .filter_map(|t| {
            let tokens = crate::document_io::tokenize_tokens(t.trim()).ok()?;
            if tokens.first()? != "glyph" {
                return None;
            }
            at_base_from_glyph_name(tokens.get(1)?)
        })
        .next()
}

pub fn parse_glyph_name(s: &str) -> GlyphName {
    GlyphName(s.trim().to_string())
}
