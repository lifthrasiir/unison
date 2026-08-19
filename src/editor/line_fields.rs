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
//!
//! Keeping the roles apart is also what keeps the namespaces apart — a `remap`
//! group named `liga` is not an appearance of a glyph named `liga`. Two of those
//! distinctions are easy to lose:
//!
//! - a `remap` line's first operand names the *group*, not a glyph, so a glyph
//!   rename must not rewrite it;
//! - `feature ... : anchor NAME` names an anchor where the plain form names a
//!   remap group, and the `anchor` keyword is the only thing telling them apart.
//!
//! The `SLICE :` qualifier (and `meta`'s `FACE :` scope) is taken off before a
//! directive's operands are read, the same way the parser takes it off — see
//! [`split_qualifier`]. Doing it anywhere else would shift every arity check
//! below by two and quietly stop a qualified line from naming anything.

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
    /// `anchor` name, including its `+`/`-` prefix.
    PointDef,
    /// `color NAME` — the defining name.
    ColorDef,
    /// A color-name reference: `ref ... fill NAME`, `color X = NAME`.
    ColorRef,
    /// `feature ... : GROUP` — the remap-group reference.
    RemapGroupRef,
    /// `remap GROUP : ...` and `remap group GROUP ...` — the remap group's own
    /// name.  Several `remap` lines share one group and the declaration is
    /// optional, so this is a definition in the sense that it is not a
    /// reference, not in the sense of a unique site.
    RemapGroupDef,
    /// `feature TAG for ...` — the OpenType feature tag.  Like a remap group
    /// it has no single declaration site, so every appearance is one of these.
    FeatureDef,
    /// `face FACE ...` — the face's own declaration.
    FaceDef,
    /// A face reference: the `FACE :` scope on a `meta` line.
    FaceRef,
    /// `slice SLICE ...` — the slice's own declaration.
    SliceDef,
    /// A slice reference: the `SLICE :` qualifier on `map`/`feature`, the
    /// slices a `face` includes, a `slice ... = ...` union, and the
    /// `for SLICE...` of an `assert shape`.
    SliceRef,
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

/// Whether a `remap` line's tokens — the keyword already dropped — declare a
/// group rather than state a rule.
///
/// Told apart the same way the parser tells them apart: a rule always writes a
/// colon straight after its group name, so a group named `group` needs no
/// special case. Completion asks this too, so the two cannot drift.
pub(crate) fn is_remap_group_decl(rest: &[TokenSpan]) -> bool {
    rest.first().is_some_and(|s| s.value == "group")
        && rest
            .get(1)
            .is_some_and(|s| s.value != ":" && !s.value.ends_with(':'))
}

/// Splits the optional leading `SLICE :` (or, on `meta`, `FACE :`) qualifier
/// off a directive's operands.
///
/// Told apart exactly as [`crate::document::DocumentItem::split_slice_qualifier`]
/// tells it apart — a *bare* `:` as the second token — so a `map : = colon`
/// keeps its own colon and the classification cannot disagree with the parser
/// about where the operands begin.
fn split_qualifier(rest: &[TokenSpan]) -> (Option<&TokenSpan>, &[TokenSpan]) {
    if rest.len() >= 2 && rest[1].value == ":" && rest[0].value != ":" {
        (Some(&rest[0]), &rest[2..])
    } else {
        (None, rest)
    }
}

/// Push one `SliceRef` per slice of a `SLICE[|SLICE...] :` qualifier, each with
/// its own columns — so a link, a hover or an F2 lands on the slice under the
/// caret rather than on the whole list.
///
/// A quoted token is left whole: its raw span no longer lines up with its
/// value, and a slice id never needs quoting anyway.
fn push_slice_refs(fields: &mut Vec<LineField>, leading: usize, span: &TokenSpan) {
    if span.value.is_empty() {
        return;
    }
    let unquoted = span.raw_end - span.raw_start == span.value.chars().count();
    if !span.value.contains('|') || !unquoted {
        fields.push(field(FieldRole::SliceRef, leading, span));
        return;
    }
    let mut at = span.raw_start;
    for part in span.value.split('|') {
        let len = part.chars().count();
        if !part.is_empty() {
            fields.push(LineField {
                role: FieldRole::SliceRef,
                token: part.to_string(),
                col_start: leading + at,
                col_end: leading + at + len,
            });
        }
        at += len + 1; // the `|` itself
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
        // An IDC line's tokens are components (glyph names) and gaps (numbers),
        // told apart exactly as the parser tells them apart, so a rename of a
        // component reaches the line that uses it.
        kw if crate::compose::IdcOp::from_token(kw).is_some() => {
            // A trailing `ifexists` is the line's flag, not a component, so it
            // is no more a name here than it is on a `map`.
            let rest = match rest.split_last() {
                Some((last, head)) if last.value == "ifexists" && !head.is_empty() => head,
                _ => rest,
            };
            for span in rest {
                if !span.value.is_empty() && span.value.parse::<i16>().is_err() {
                    fields.push(field(FieldRole::GlyphRef, leading, span));
                }
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
            let (slice, rest) = split_qualifier(rest);
            if let Some(slice) = slice {
                push_slice_refs(&mut fields, leading, slice);
            }
            if rest.len() >= 3 && rest[1].value == "=" {
                fields.push(field(FieldRole::NamePartsDef, leading, &rest[0]));
                for span in &rest[2..] {
                    fields.push(field(FieldRole::NamePartsValue, leading, span));
                }
            }
        }
        "map" => {
            // The `SLICE :` qualifier comes off first, so the arities below are
            // the ones the unqualified form has always had.
            let (slice, rest) = split_qualifier(rest);
            if let Some(slice) = slice {
                push_slice_refs(&mut fields, leading, slice);
            }
            // …and so does a trailing `ifexists`, for the same reason: it is a
            // flag on those arities, not one of their tokens.
            let rest = match rest.split_last() {
                Some((last, head)) if last.value == "ifexists" && !head.is_empty() => head,
                _ => rest,
            };
            if rest.len() == 3 && rest[1].value == "=" {
                fields.push(field(FieldRole::GlyphRef, leading, &rest[2]));
            } else if rest.len() == 4 && rest[0].value == "generate" && rest[2].value == "=" {
                // `map generate CHAR = NAME` names a glyph that does not exist
                // anywhere else, so this token *is* its definition.
                fields.push(field(FieldRole::GlyphDef, leading, &rest[3]));
            }
        }
        // `remap group NAME [reversed] [after GROUP]...` — a declaration, whose
        // every name is a group name and none of them a glyph.
        "remap" if is_remap_group_decl(rest) => {
            fields.push(field(FieldRole::RemapGroupDef, leading, &rest[1]));
            let mut i = 2;
            while i < rest.len() {
                if rest[i].value == "after"
                    && let Some(target) = rest.get(i + 1)
                    && !target.value.is_empty()
                {
                    fields.push(field(FieldRole::RemapGroupRef, leading, target));
                    i += 2;
                } else {
                    i += 1;
                }
            }
        }
        "remap" => {
            // The first operand names the group, not a glyph — the two live in
            // different namespaces, so a glyph rename must not rewrite it.
            let mut first = true;
            for span in rest {
                let clean = span.value.trim_end_matches(':');
                if !clean.is_empty() && clean != "->" && clean != ":" {
                    let role = if first {
                        FieldRole::RemapGroupDef
                    } else {
                        FieldRole::GlyphRef
                    };
                    first = false;
                    fields.push(LineField {
                        role,
                        token: clean.to_string(),
                        col_start: leading + span.raw_start,
                        col_end: leading + span.raw_start + clean.chars().count(),
                    });
                }
            }
        }
        "feature" => {
            let (slice, rest) = split_qualifier(rest);
            if let Some(slice) = slice {
                push_slice_refs(&mut fields, leading, slice);
            }
            if let Some(tag) = rest.first()
                && !tag.value.is_empty()
                && tag.value != "for"
            {
                fields.push(field(FieldRole::FeatureDef, leading, tag));
            }
            // `: anchor NAME` is the mark-attachment variant, so what follows
            // the colon there is an anchor name and not a remap group.
            if let Some(colon_pos) = rest.iter().position(|s| s.value == ":") {
                match rest.get(colon_pos + 1) {
                    Some(kw) if kw.value == "anchor" => {
                        if let Some(name) = rest.get(colon_pos + 2)
                            && !name.value.is_empty()
                        {
                            fields.push(field(FieldRole::PointDef, leading, name));
                        }
                    }
                    Some(group) if !group.value.is_empty() => {
                        fields.push(field(FieldRole::RemapGroupRef, leading, group));
                    }
                    _ => {}
                }
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
        // `face FACE [: SLICE...]` and `slice SLICE [= SLICE...]`. The
        // separator differs because the two mean different things, and a
        // malformed line names nothing rather than half of something.
        "face" | "slice" => {
            let is_face = keyword_span.value == "face";
            let sep = if is_face { ":" } else { "=" };
            if let Some(id) = rest.first()
                && !id.value.is_empty()
                && id.value != sep
            {
                let role = if is_face {
                    FieldRole::FaceDef
                } else {
                    FieldRole::SliceDef
                };
                fields.push(field(role, leading, id));
                if rest.get(1).is_some_and(|s| s.value == sep) {
                    for span in &rest[2..] {
                        if !span.value.is_empty() {
                            fields.push(field(FieldRole::SliceRef, leading, span));
                        }
                    }
                }
            }
        }
        // `meta [FACE :] KEY VALUE...`. `*` is the explicit spelling of "every
        // face" rather than a face anything could go to.
        "meta" => {
            if let (Some(scope), _) = split_qualifier(rest)
                && !scope.value.is_empty()
                && scope.value != "*"
            {
                fields.push(field(FieldRole::FaceRef, leading, scope));
            }
        }
        "anchor" => {
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
                // assert shape TEXT [+feat]... [for SLICE...] : GLYPH1 ... : GLYPH2 ...
                // Only the first token after each `:` is a glyph name, and
                // `for ...` runs to the first `:` — everything in it is a slice.
                let head_end = rest
                    .iter()
                    .position(|s| s.value == ":")
                    .unwrap_or(rest.len());
                if let Some(for_pos) = rest[..head_end].iter().position(|s| s.value == "for") {
                    for span in &rest[for_pos + 1..head_end] {
                        if !span.value.is_empty() {
                            fields.push(field(FieldRole::SliceRef, leading, span));
                        }
                    }
                }
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

    /// A rename has to reach a glyph named on an IDC line, and a gap is a
    /// number rather than a very short glyph.
    #[test]
    fn idc_components_are_glyph_references() {
        assert_eq!(
            roles("\u{2FF0} a:4x16 -1 b:12x16"),
            vec![
                (FieldRole::GlyphRef, "a:4x16".to_string()),
                (FieldRole::GlyphRef, "b:12x16".to_string()),
            ]
        );
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
        // The group name is not a glyph name, however much it looks like one.
        assert_eq!(fields[0].role, FieldRole::RemapGroupDef);
        assert!(fields[1..].iter().all(|f| f.role == FieldRole::GlyphRef));
    }

    /// Every name on a group declaration is a group name. A glyph rename that
    /// rewrote `after flag` here would silently move a lookup.
    #[test]
    fn remap_group_declaration_names_only_groups() {
        assert_eq!(
            roles("remap group ascii-arrow reversed after eq-liga after flag"),
            vec![
                (FieldRole::RemapGroupDef, "ascii-arrow".to_string()),
                (FieldRole::RemapGroupRef, "eq-liga".to_string()),
                (FieldRole::RemapGroupRef, "flag".to_string()),
            ],
        );
    }

    /// A group named `group` still writes rules the ordinary way, and the
    /// colon after the group name is what says so.
    #[test]
    fn a_group_named_group_still_parses_as_a_rule() {
        assert_eq!(
            roles("remap group : a -> b"),
            vec![
                (FieldRole::RemapGroupDef, "group".to_string()),
                (FieldRole::GlyphRef, "a".to_string()),
                (FieldRole::GlyphRef, "b".to_string()),
            ],
        );
        assert_eq!(
            roles("remap group: a -> b"),
            vec![
                (FieldRole::RemapGroupDef, "group".to_string()),
                (FieldRole::GlyphRef, "a".to_string()),
                (FieldRole::GlyphRef, "b".to_string()),
            ],
        );
    }

    #[test]
    fn feature_names_its_tag_and_its_group() {
        assert_eq!(
            roles("feature ljmo for hang : hangul-ljmo"),
            vec![
                (FieldRole::FeatureDef, "ljmo".to_string()),
                (FieldRole::RemapGroupRef, "hangul-ljmo".to_string()),
            ],
        );
    }

    /// The mark-attachment variant names an anchor after the colon, not a
    /// remap group — the `anchor` keyword is what tells the two apart.
    #[test]
    fn feature_anchor_variant_names_an_anchor() {
        assert_eq!(
            roles("feature abvm for hang : anchor above"),
            vec![
                (FieldRole::FeatureDef, "abvm".to_string()),
                (FieldRole::PointDef, "above".to_string()),
            ],
        );
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

    /// The `SLICE :` qualifier is a slice reference, and the operands after it
    /// are read exactly as the unqualified form's are.
    #[test]
    fn map_slice_qualifier_keeps_the_glyph_name() {
        assert_eq!(
            roles("map narrow : A = latin-a"),
            vec![
                (FieldRole::SliceRef, "narrow".to_string()),
                (FieldRole::GlyphRef, "latin-a".to_string()),
            ],
        );
        assert_eq!(
            roles("map narrow : generate 가 = hangul-ga"),
            vec![
                (FieldRole::SliceRef, "narrow".to_string()),
                (FieldRole::GlyphDef, "hangul-ga".to_string()),
            ],
        );
    }

    /// A listed qualifier is one field per slice, each with its own columns —
    /// otherwise a link or an F2 would act on `wide|narrow` as if that were one
    /// id. `name-parts` takes the same qualifier.
    #[test]
    fn each_slice_of_a_listed_qualifier_is_its_own_field() {
        let fields = classify_line("map wide|narrow : A = latin-a");
        assert_eq!(
            fields
                .iter()
                .map(|f| (f.role, f.token.as_str(), f.col_start, f.col_end))
                .collect::<Vec<_>>(),
            vec![
                (FieldRole::SliceRef, "wide", 4, 8),
                (FieldRole::SliceRef, "narrow", 9, 15),
                (FieldRole::GlyphRef, "latin-a", 22, 29),
            ],
        );
        assert_eq!(
            roles("name-parts narrow : $half = -half"),
            vec![
                (FieldRole::SliceRef, "narrow".to_string()),
                (FieldRole::NamePartsDef, "$half".to_string()),
                (FieldRole::NamePartsValue, "-half".to_string()),
            ],
        );
    }

    /// `map : = colon` maps a colon; the qualifier needs a *bare* `:` in second
    /// place, so the character being mapped is never mistaken for a slice.
    #[test]
    fn a_colon_being_mapped_is_not_a_qualifier() {
        assert_eq!(
            roles("map : = colon"),
            vec![(FieldRole::GlyphRef, "colon".to_string())],
        );
    }

    #[test]
    fn feature_slice_qualifier_shifts_nothing_else() {
        assert_eq!(
            roles("feature wide : liga for latn : eq-liga"),
            vec![
                (FieldRole::SliceRef, "wide".to_string()),
                (FieldRole::FeatureDef, "liga".to_string()),
                (FieldRole::RemapGroupRef, "eq-liga".to_string()),
            ],
        );
        assert_eq!(
            roles("feature wide : abvm for hang : anchor above"),
            vec![
                (FieldRole::SliceRef, "wide".to_string()),
                (FieldRole::FeatureDef, "abvm".to_string()),
                (FieldRole::PointDef, "above".to_string()),
            ],
        );
    }

    #[test]
    fn face_and_slice_declarations_name_faces_and_slices() {
        assert_eq!(
            roles("face term : narrow"),
            vec![
                (FieldRole::FaceDef, "term".to_string()),
                (FieldRole::SliceRef, "narrow".to_string()),
            ],
        );
        assert_eq!(
            roles("slice narrow"),
            vec![(FieldRole::SliceDef, "narrow".to_string())]
        );
        assert_eq!(
            roles("slice both = narrow wide"),
            vec![
                (FieldRole::SliceDef, "both".to_string()),
                (FieldRole::SliceRef, "narrow".to_string()),
                (FieldRole::SliceRef, "wide".to_string()),
            ],
        );
        // The separators are not interchangeable: a `face` unions nothing and a
        // `slice` includes nothing, so the wrong one names only the id.
        assert_eq!(
            roles("face term = narrow"),
            vec![(FieldRole::FaceDef, "term".to_string())]
        );
        assert_eq!(
            roles("slice both : narrow"),
            vec![(FieldRole::SliceDef, "both".to_string())]
        );
    }

    /// A `meta` scope names a face; a bare key and the `*` spelling name none.
    #[test]
    fn meta_scope_names_a_face() {
        assert_eq!(
            roles("meta term : family Unison Term"),
            vec![(FieldRole::FaceRef, "term".to_string())],
        );
        assert!(roles("meta * : family Unison").is_empty());
        assert!(roles("meta family Unison").is_empty());
    }

    #[test]
    fn assert_shape_for_names_slices() {
        assert_eq!(
            roles("assert shape AB +liga for narrow wide : a-b : c"),
            vec![
                (FieldRole::SliceRef, "narrow".to_string()),
                (FieldRole::SliceRef, "wide".to_string()),
                (FieldRole::GlyphRef, "a-b".to_string()),
                (FieldRole::GlyphRef, "c".to_string()),
            ],
        );
    }

    #[test]
    fn unknown_keyword_has_no_fields() {
        assert!(roles("meta height 16").is_empty());
        assert!(roles("// comment").is_empty());
        assert!(roles("").is_empty());
    }
}
