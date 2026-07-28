//! One classification pass over a `.unf` text line, naming every token that
//! refers to (or defines) a named entity.
//!
//! This is the single place that knows *where names live* in each directive.
//! Everything the editor does with names on a line — clickable links
//! (`doc_links::extract_line_links`), rename-target detection
//! (`doc_links::find_renameable_at_caret`), rename text mutation
//! (`app::rename_in_place`) and completion of existing tokens
//! (`autocomplete::detect_context`) — consumes the fields produced here, so
//! a new directive form only needs to be described once and every feature
//! picks it up together.  (What completion offers *between* tokens is a
//! different question and stays in `autocomplete`.)

use crate::document_io::{TokenSpan, tokenize_with_spans};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FieldRole {
    /// `glyph NAME` — the defining name (may be a pattern).
    GlyphDef,
    /// A glyph reference: `ref` target, `map`/alias target, remap operands,
    /// `assert` names, `assume unused` / `exclude-from-sample` arguments.
    GlyphRef,
    /// `name-parts $NAME` — the defining name.
    NamePartsDef,
    /// A `name-parts` value token (may contain `$var` references).
    NamePartsValue,
    /// `point`/`anchor` name, including its `+`/`-` prefix.
    PointDef,
    /// `color NAME` — the defining name.
    ColorDef,
    /// A color-name reference: `ref ... fill NAME`, `color X = NAME`.
    ColorRef,
    /// `feature ... : GROUP` — the remap-group reference.
    RemapGroupRef,
}

#[derive(Clone, Debug)]
pub(crate) struct LineField {
    pub role: FieldRole,
    /// Unquoted token value; a remap operand's structural `:` suffix is
    /// stripped (and excluded from the span).
    pub token: String,
    /// Char-column range in the full (untrimmed) line; `col_end` is one past
    /// the last char.  Caret checks treat both ends as inclusive, matching
    /// how a caret sits between characters.
    pub col_start: usize,
    pub col_end: usize,
}

impl LineField {
    pub fn contains_col(&self, col: usize) -> bool {
        col >= self.col_start && col <= self.col_end
    }
}

fn field(role: FieldRole, leading: usize, span: &TokenSpan) -> LineField {
    LineField {
        role,
        token: span.value.clone(),
        col_start: leading + span.raw_start,
        col_end: leading + span.raw_end,
    }
}

pub(crate) fn classify_line(line: &str) -> Vec<LineField> {
    let trimmed = line.trim_start();
    let leading = line.chars().count() - trimmed.chars().count();
    let Ok(spans) = tokenize_with_spans(trimmed) else {
        return Vec::new();
    };
    let Some((keyword_span, rest)) = spans.split_first() else {
        return Vec::new();
    };

    let mut fields = Vec::new();
    match keyword_span.value.as_str() {
        "ref" => {
            if let Some(name) = rest.first()
                && !name.value.is_empty()
            {
                fields.push(field(FieldRole::GlyphRef, leading, name));
            }
            if let Some(fill_pos) = rest.iter().position(|s| s.value == "fill")
                && let Some(color) = rest.get(fill_pos + 1)
                && !color.value.starts_with('#')
                && color.value != "fg"
                && !color.value.is_empty()
            {
                fields.push(field(FieldRole::ColorRef, leading, color));
            }
        }
        "glyph" => {
            if let Some(name) = rest.first()
                && !name.value.is_empty()
                && name.value != "="
            {
                fields.push(field(FieldRole::GlyphDef, leading, name));
            }
            if let Some(eq_pos) = rest.iter().position(|s| s.value == "=")
                && let Some(alias) = rest.get(eq_pos + 1)
                && !alias.value.is_empty()
            {
                fields.push(field(FieldRole::GlyphRef, leading, alias));
            }
        }
        "name-parts" => {
            if rest.len() >= 3 && rest[1].value == "=" {
                fields.push(field(FieldRole::NamePartsDef, leading, &rest[0]));
                for span in &rest[2..] {
                    fields.push(field(FieldRole::NamePartsValue, leading, span));
                }
            }
        }
        "map" => {
            if rest.len() == 3 && rest[1].value == "=" {
                fields.push(field(FieldRole::GlyphRef, leading, &rest[2]));
            }
        }
        "remap" => {
            for span in rest {
                let clean = span.value.trim_end_matches(':');
                if !clean.is_empty() && clean != "->" && clean != ":" {
                    fields.push(LineField {
                        role: FieldRole::GlyphRef,
                        token: clean.to_string(),
                        col_start: leading + span.raw_start,
                        col_end: leading + span.raw_start + clean.chars().count(),
                    });
                }
            }
        }
        "feature" => {
            if let Some(colon_pos) = rest.iter().position(|s| s.value == ":")
                && let Some(group) = rest.get(colon_pos + 1)
            {
                fields.push(field(FieldRole::RemapGroupRef, leading, group));
            }
        }
        "color" => {
            if rest.len() >= 3 && rest[1].value == "=" {
                fields.push(field(FieldRole::ColorDef, leading, &rest[0]));
                let value = &rest[2];
                if !value.value.starts_with('#') && !value.value.is_empty() {
                    fields.push(field(FieldRole::ColorRef, leading, value));
                }
            }
        }
        "point" | "anchor" => {
            if let Some(name) = rest.first()
                && !name.value.is_empty()
            {
                fields.push(field(FieldRole::PointDef, leading, name));
            }
        }
        "exclude-from-sample" => {
            if let Some(name) = rest.first()
                && !name.value.is_empty()
            {
                fields.push(field(FieldRole::GlyphRef, leading, name));
            }
        }
        "assume" => {
            if rest.first().is_some_and(|s| s.value == "unused") {
                for span in &rest[1..] {
                    if !span.value.is_empty() {
                        fields.push(field(FieldRole::GlyphRef, leading, span));
                    }
                }
            }
        }
        "assert" => match rest.first().map(|s| s.value.as_str()) {
            Some("same") | Some("distinct") => {
                for span in &rest[1..] {
                    if !span.value.is_empty() {
                        fields.push(field(FieldRole::GlyphRef, leading, span));
                    }
                }
            }
            Some("shape") => {
                // assert shape TEXT [+feat]... : GLYPH1 ... : GLYPH2 ...
                // Only the first token after each `:` is a glyph name.
                let mut after_colon = false;
                for span in &rest[1..] {
                    if span.value == ":" {
                        after_colon = true;
                        continue;
                    }
                    if after_colon {
                        if !span.value.is_empty() {
                            fields.push(field(FieldRole::GlyphRef, leading, span));
                        }
                        after_colon = false;
                    }
                }
            }
            _ => {}
        },
        _ => {}
    }

    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(line: &str) -> Vec<(FieldRole, String)> {
        classify_line(line)
            .into_iter()
            .map(|f| (f.role, f.token))
            .collect()
    }

    #[test]
    fn ref_with_fill() {
        assert_eq!(
            roles("ref foo 0 0 fill red"),
            vec![
                (FieldRole::GlyphRef, "foo".to_string()),
                (FieldRole::ColorRef, "red".to_string()),
            ],
        );
    }

    #[test]
    fn ref_fill_fg_and_hex_are_not_color_refs() {
        assert_eq!(roles("ref foo 0 0 fill fg").len(), 1);
        assert_eq!(roles("ref foo 0 0 fill #ff0000").len(), 1);
    }

    #[test]
    fn glyph_alias_has_def_and_ref() {
        assert_eq!(
            roles("glyph foo advance 8 = bar"),
            vec![
                (FieldRole::GlyphDef, "foo".to_string()),
                (FieldRole::GlyphRef, "bar".to_string()),
            ],
        );
    }

    #[test]
    fn remap_strips_structural_colons() {
        let fields = classify_line("remap liga : a -> b");
        let tokens: Vec<&str> = fields.iter().map(|f| f.token.as_str()).collect();
        assert_eq!(tokens, vec!["liga", "a", "b"]);
        assert!(fields.iter().all(|f| f.role == FieldRole::GlyphRef));
    }

    #[test]
    fn indentation_offsets_spans() {
        let fields = classify_line("    ref foo");
        assert_eq!(fields[0].col_start, 8);
        assert_eq!(fields[0].col_end, 11);
    }

    #[test]
    fn assert_shape_names_after_colons_only() {
        assert_eq!(
            roles("assert shape AB +liga : a-b extra : c"),
            vec![
                (FieldRole::GlyphRef, "a-b".to_string()),
                (FieldRole::GlyphRef, "c".to_string()),
            ],
        );
    }

    /// A trailing comment names nothing, on any directive.
    #[test]
    fn comments_contribute_no_fields() {
        assert_eq!(
            roles("assert same foo bar // baz quux"),
            vec![
                (FieldRole::GlyphRef, "foo".to_string()),
                (FieldRole::GlyphRef, "bar".to_string()),
            ],
        );
        assert_eq!(
            roles("ref foo 0 0 // fill red"),
            vec![(FieldRole::GlyphRef, "foo".to_string())],
        );
        assert_eq!(
            roles("assert shape AB : a-b // : c"),
            vec![(FieldRole::GlyphRef, "a-b".to_string())],
        );
        // The quoted `//` is a token like any other, so the comment is the
        // *unquoted* one that follows it.
        assert_eq!(
            roles("map `//` = solidus // the slash"),
            vec![(FieldRole::GlyphRef, "solidus".to_string())],
        );
    }

    #[test]
    fn unknown_keyword_has_no_fields() {
        assert!(roles("font-meta height 16 ascent 12 descent 4").is_empty());
        assert!(roles("// comment").is_empty());
        assert!(roles("").is_empty());
    }
}
