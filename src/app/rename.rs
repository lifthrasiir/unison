//! Renaming a name across every open document — and across the unopened files
//! that mention it, which [`doc_may_reference`] finds and opens first.
//!
//! Every kind in [`crate::editor::doc_links::RenameKind`] is renamed the same
//! way: the line classification says which tokens are that kind's name, and
//! this file splices over exactly those. So the two halves cannot disagree
//! about, say, whether a `remap` line's first operand is a glyph.
//!
//! `doc_may_reference` is the one place that has to be kept in step by hand:
//! it decides which *unopened* files to load, from parsed items rather than
//! from the classification, and a kind missing an arm there renames only the
//! files that happen to be open.

use super::docs::{load_open_document, shadowed_by_open};
use super::*;

/// Apply rename in place, returning the old text values of changed lines
/// (as `(line_index, old_text)` pairs) so callers can build undo entries
/// without cloning the entire document (which is expensive when Grid lines
/// dominate).
/// `caret`, when given, is moved along with the text it sits in, so the
/// editor's caret still points at the same place once the popup closes.
fn rename_in_place(
    lines: &mut [DocLine],
    old_name: &str,
    new_name: &str,
    kind: &crate::editor::doc_links::RenameKind,
    caret: Option<&mut crate::editor::caret::Caret>,
) -> Vec<(usize, String)> {
    let mut changed = Vec::new();
    let mut caret = caret;
    let mut at_base: Option<String> = None;
    for (i, line) in lines.iter_mut().enumerate() {
        let DocLine::Text(s) = line else { continue };
        // The base as it stands *before* this rename touches anything: an `@`
        // is matched against the name the glyph has now, not the one it is
        // about to get. Taken from the header line itself only after that line
        // has been rewritten, for the same reason the parser reads it that way.
        let line_base = at_base.clone();
        super::search::advance_at_base(&mut at_base, s);
        if let Some((t, spans)) = rename_in_line(s, old_name, new_name, kind, line_base.as_deref())
        {
            if let Some(c) = caret.as_deref_mut()
                && c.line == i
            {
                c.col = shift_caret_col(c.col, &spans);
            }
            changed.push((i, std::mem::replace(s, t)));
        }
    }
    changed
}

fn doc_may_reference(
    items: &[crate::document::DocumentItem],
    name: &str,
    kind: &crate::editor::doc_links::RenameKind,
) -> bool {
    use crate::document::DocumentItem;
    use crate::editor::doc_links::RenameKind;

    for item in items {
        match (kind, item) {
            (RenameKind::Glyph, DocumentItem::Glyph { name: gn, body }) => {
                if gn.0 == name {
                    return true;
                }
                if body.refs.iter().any(|r| r.name == name) {
                    return true;
                }
                // An IDC line names glyphs as directly as a `ref` does, and it
                // is the only way most han parts are ever mentioned.
                if body
                    .compose
                    .iter()
                    .any(|c| c.part_names().any(|p| p == name))
                {
                    return true;
                }
            }
            (
                RenameKind::Glyph,
                DocumentItem::GlyphAlias {
                    name: gn, target, ..
                },
            ) => {
                if gn.0 == name || target == name {
                    return true;
                }
            }
            (RenameKind::Glyph, DocumentItem::Map { glyph, .. }) => {
                if glyph == name {
                    return true;
                }
            }
            (
                RenameKind::Glyph,
                DocumentItem::MapDecomposed {
                    glyph: Some(glyph), ..
                },
            ) => {
                if glyph == name {
                    return true;
                }
            }
            (RenameKind::Glyph, DocumentItem::Remap { .. }) => {
                let mut all = item.remap_operands();
                if all.any(|s| s == name) {
                    return true;
                }
            }
            (RenameKind::Glyph, DocumentItem::Directive(s)) => {
                if s.contains(name) {
                    return true;
                }
            }
            (RenameKind::Glyph, DocumentItem::AssertShape { expected, .. }) => {
                if expected.iter().any(|e| e.name == name) {
                    return true;
                }
            }
            (
                RenameKind::Glyph,
                DocumentItem::AssertSame { names, .. } | DocumentItem::AssertDistinct { names, .. },
            ) => {
                if names.iter().any(|n| n == name) {
                    return true;
                }
            }
            (
                RenameKind::NameParts,
                DocumentItem::NameParts {
                    name: n, values, ..
                },
            ) => {
                // Values embed a `$var` inside larger tokens too, so the test
                // is substring, not equality.
                if n == name || values.iter().any(|v| v.contains(name)) {
                    return true;
                }
            }
            (RenameKind::NameParts, DocumentItem::Glyph { name: gn, body }) => {
                if gn.0.contains(name) {
                    return true;
                }
                if body.refs.iter().any(|r| r.name.contains(name)) {
                    return true;
                }
                if body
                    .compose
                    .iter()
                    .any(|c| c.part_names().any(|p| p.contains(name)))
                {
                    return true;
                }
            }
            // A `$var` is embedded inside larger glyph-name tokens, so every
            // item that names glyphs can carry one. Substring only opens the
            // file; the boundary-checked rewrite is `rename_in_line`'s.
            (RenameKind::NameParts, DocumentItem::Map { glyph, .. }) => {
                if glyph.contains(name) {
                    return true;
                }
            }
            (
                RenameKind::NameParts,
                DocumentItem::MapDecomposed {
                    glyph: Some(glyph), ..
                },
            ) => {
                if glyph.contains(name) {
                    return true;
                }
            }
            (RenameKind::NameParts, DocumentItem::Remap { .. }) => {
                let mut all = item.remap_operands();
                if all.any(|s| s.contains(name)) {
                    return true;
                }
            }
            (
                RenameKind::NameParts,
                DocumentItem::GlyphAlias {
                    name: gn, target, ..
                },
            ) => {
                if gn.0.contains(name) || target.contains(name) {
                    return true;
                }
            }
            (RenameKind::Point, DocumentItem::Glyph { body, .. }) => {
                let stripped = name.trim_start_matches(['+', '-']);
                if body.points.iter().any(|p| {
                    let ps = p.position.trim_start_matches(['+', '-']);
                    ps == stripped
                }) {
                    return true;
                }
            }
            // An anchor-driven `feature` names an anchor without any glyph in
            // the file having to carry that point.
            (RenameKind::Point, DocumentItem::FeatureAnchor { anchor, .. }) => {
                if anchor.trim_start_matches(['+', '-']) == name.trim_start_matches(['+', '-']) {
                    return true;
                }
            }
            // A color alias's value is a reference to the color it points at.
            (RenameKind::Color, DocumentItem::Color { name: n, value, .. }) => {
                if n == name || value == name {
                    return true;
                }
            }
            (RenameKind::Color, DocumentItem::Glyph { body, .. }) => {
                if body
                    .refs
                    .iter()
                    .any(|r| r.fill.as_ref().is_some_and(|f| f.color == name))
                {
                    return true;
                }
            }
            (RenameKind::Face, DocumentItem::Face { id, .. }) => {
                if id == name {
                    return true;
                }
            }
            // A `meta` line keeps its text unparsed, so its `FACE :` scope is
            // read the one way every other consumer reads a name: through the
            // line classification, keyword and all.
            (RenameKind::Face, DocumentItem::Meta(text)) => {
                if meta_scope_is(text, name) {
                    return true;
                }
            }
            (RenameKind::Slice, DocumentItem::Slice { id, inherits, .. }) => {
                if id == name || inherits.iter().any(|s| s == name) {
                    return true;
                }
            }
            (RenameKind::Slice, DocumentItem::Face { slices, .. }) => {
                if slices.iter().any(|s| s == name) {
                    return true;
                }
            }
            (RenameKind::Slice, item) if item.slice_qualifier().iter().any(|s| s == name) => {
                return true;
            }
            (RenameKind::Slice, DocumentItem::AssertShape { slices, .. }) => {
                if slices.iter().any(|s| s == name) {
                    return true;
                }
            }
            (RenameKind::RemapGroup, DocumentItem::Remap { feature, .. }) => {
                if feature == name {
                    return true;
                }
            }
            (RenameKind::RemapGroup, DocumentItem::RemapGroup { name: n, after, .. }) => {
                if n == name || after.iter().any(|g| g == name) {
                    return true;
                }
            }
            (RenameKind::RemapGroup, DocumentItem::Feature { remap_group, .. })
                if remap_group == name =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Whether a `meta` item's text — everything after the keyword — is scoped to
/// the face `name`.
fn meta_scope_is(text: &str, name: &str) -> bool {
    use crate::editor::line_fields::{FieldRole, classify_line};
    classify_line(&format!("meta {text}"))
        .iter()
        .any(|f| f.role == FieldRole::FaceRef && f.token == name)
}

/// What a glyph token becomes when the glyph it names is renamed to `new_name`.
///
/// A token written with `@` keeps its `@` whenever the new name is still in the
/// base's family — renaming `foo-bar` to `foo-qux` leaves `@-qux`, so the
/// helper goes on following its base. A new name outside the family has no `@`
/// spelling, so it is written out in full.
fn renamed_glyph_token(token: &str, new_name: &str, at_base: Option<&str>) -> String {
    match (token.starts_with('@'), at_base) {
        (true, Some(base)) => match new_name.strip_prefix(base) {
            Some(rest) => format!("@{rest}"),
            None => new_name.to_string(),
        },
        _ => new_name.to_string(),
    }
}

/// One replacement `rename_in_line` made, in character columns of the *old*
/// line: `[start, end)` became `new_len` characters.  Ascending by `start`.
#[derive(Clone, Copy)]
struct RenameSpan {
    start: usize,
    end: usize,
    new_len: usize,
}

/// Applies a rename to one line by splicing new text over the classified
/// name fields (`crate::editor::line_fields`).  Detection and mutation share
/// the classification, so whatever the rename popup identified is exactly
/// what gets rewritten.  Returns `None` when the line is unaffected.
fn rename_in_line(
    full: &str,
    old_name: &str,
    new_name: &str,
    kind: &crate::editor::doc_links::RenameKind,
    at_base: Option<&str>,
) -> Option<(String, Vec<RenameSpan>)> {
    use crate::editor::doc_links::RenameKind;
    use crate::editor::line_fields::{FieldRole, classify_line};

    // (char col start, char col end, replacement)
    let mut reps: Vec<(usize, usize, String)> = Vec::new();
    for f in classify_line(full) {
        let rep = match (kind, f.role) {
            (RenameKind::Glyph, FieldRole::GlyphDef | FieldRole::GlyphRef)
                if crate::document::expand_at_name(&f.token, at_base) == old_name =>
            {
                Some(crate::document_io::quote_token(&renamed_glyph_token(
                    &f.token, new_name, at_base,
                )))
            }
            (
                RenameKind::NameParts,
                FieldRole::GlyphDef | FieldRole::GlyphRef | FieldRole::NamePartsValue,
            ) => {
                let new_tok = replace_dollar_var(&f.token, old_name, new_name);
                (new_tok != f.token).then(|| crate::document_io::quote_token(&new_tok))
            }
            (RenameKind::NameParts, FieldRole::NamePartsDef) if f.token == old_name => {
                Some(new_name.to_string())
            }
            (RenameKind::Point, FieldRole::PointDef) => {
                let (prefix, bare) = match f.token.strip_prefix(['+', '-']) {
                    Some(stripped) => (&f.token[..1], stripped),
                    None => ("", f.token.as_str()),
                };
                (bare == old_name).then(|| format!("{prefix}{new_name}"))
            }
            (RenameKind::Color, FieldRole::ColorDef | FieldRole::ColorRef)
                if f.token == old_name =>
            {
                Some(crate::document_io::quote_token(new_name))
            }
            (RenameKind::Face, FieldRole::FaceDef | FieldRole::FaceRef) if f.token == old_name => {
                Some(crate::document_io::quote_token(new_name))
            }
            // The classification already stripped a rule's structural `:` off
            // the token *and* off its span, so the colon stays put.
            (RenameKind::RemapGroup, FieldRole::RemapGroupDef | FieldRole::RemapGroupRef)
                if f.token == old_name =>
            {
                Some(crate::document_io::quote_token(new_name))
            }
            (RenameKind::Slice, FieldRole::SliceDef | FieldRole::SliceRef)
                if f.token == old_name =>
            {
                Some(crate::document_io::quote_token(new_name))
            }
            _ => None,
        };
        if let Some(r) = rep {
            reps.push((f.col_start, f.col_end, r));
        }
    }

    if reps.is_empty() {
        return None;
    }

    use crate::editor::caret::char_to_byte;
    reps.sort_by_key(|&(start, _, _)| std::cmp::Reverse(start));
    let mut out = full.to_string();
    let mut spans = Vec::with_capacity(reps.len());
    for (start, end, replacement) in reps {
        let byte_start = char_to_byte(&out, start);
        let byte_end = char_to_byte(&out, end);
        spans.push(RenameSpan {
            start,
            end,
            new_len: replacement.chars().count(),
        });
        out.replace_range(byte_start..byte_end, &replacement);
    }
    spans.reverse(); // back to ascending order
    Some((out, spans))
}

/// Maps a caret column across the replacements one `rename_in_line` made,
/// given as ascending [`RenameSpan`]s.  A
/// caret anywhere on a rewritten token lands just *after* the new text, so
/// the user sees the symbol they renamed; everything further right shifts
/// with it.
fn shift_caret_col(col: usize, spans: &[RenameSpan]) -> usize {
    let mut delta: isize = 0;
    for &RenameSpan {
        start,
        end,
        new_len,
    } in spans
    {
        if start <= col && col <= end {
            return (start as isize + delta) as usize + new_len;
        }
        if end < col {
            delta += new_len as isize - (end - start) as isize;
        } else {
            break;
        }
    }
    (col as isize + delta).max(0) as usize
}

fn replace_dollar_var(text: &str, old_var: &str, new_var: &str) -> String {
    // Replace $old_name with $new_name, being careful about word boundaries
    // old_var includes the $ prefix
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let old_chars: Vec<char> = old_var.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '$' && i + old_chars.len() <= chars.len() {
            let slice: String = chars[i..i + old_chars.len()].iter().collect();
            if slice == old_var {
                // Check that the next char is NOT alphanumeric/dash/underscore (word boundary)
                let next_idx = i + old_chars.len();
                let at_boundary = next_idx >= chars.len()
                    || !(chars[next_idx].is_alphanumeric()
                        || chars[next_idx] == '-'
                        || chars[next_idx] == '_');
                if at_boundary {
                    result.push_str(new_var);
                    i += old_chars.len();
                    continue;
                }
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

#[cfg(test)]
mod rename_tests {
    use super::*;
    use crate::document::DocLine;
    use crate::editor::doc_links::RenameKind;

    fn t(s: &str) -> DocLine {
        DocLine::Text(s.to_string())
    }

    fn do_rename(lines: &[DocLine], old: &str, new: &str, kind: &RenameKind) -> Vec<String> {
        let mut lines = lines.to_vec();
        rename_in_place(&mut lines, old, new, kind, None);
        lines
            .into_iter()
            .filter_map(|l| {
                if let DocLine::Text(s) = l {
                    Some(s)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn rename_glyph_header() {
        let lines = vec![t("glyph foo 8 16")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph bar 8 16"]);
    }

    #[test]
    fn rename_glyph_ref() {
        let lines = vec![t("ref foo 0 0")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["ref bar 0 0"]);
    }

    #[test]
    fn rename_glyph_map() {
        let lines = vec![t("map A = foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["map A = bar"]);
    }

    #[test]
    fn rename_glyph_alias() {
        let lines = vec![t("glyph new-name = foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph new-name = bar"]);
    }

    #[test]
    fn rename_glyph_def_in_alias_form() {
        let lines = vec![t("glyph foo = other")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph bar = other"]);
    }

    #[test]
    /// The line no longer parses — an alias takes no flags — but a rename
    /// sweeps the text, and leaving a half-migrated line behind would be worse
    /// than renaming through it.
    fn rename_glyph_alias_survives_stray_flags() {
        let lines = vec![t("glyph new-name advance 8 = foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph new-name advance 8 = bar"]);
    }

    #[test]
    fn rename_glyph_remap() {
        let lines = vec![t("remap liga : a b : foo -> bar-lig : c")];
        let result = do_rename(&lines, "foo", "quux", &RenameKind::Glyph);
        assert_eq!(result, vec!["remap liga : a b : quux -> bar-lig : c"]);
    }

    #[test]
    fn rename_glyph_exclude() {
        let lines = vec![t("exclude-from-sample foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["exclude-from-sample bar"]);
    }

    #[test]
    fn rename_glyph_no_partial_match() {
        let lines = vec![t("glyph foobar 8 16"), t("ref foo-ext 0 0")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph foobar 8 16", "ref foo-ext 0 0"]);
    }

    #[test]
    fn rename_name_parts_def() {
        let lines = vec![t("name-parts $init = a b c")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        assert_eq!(result, vec!["name-parts $vowel = a b c"]);
    }

    #[test]
    fn rename_name_parts_ref_in_glyph() {
        let lines = vec![t("glyph hangul-($init)-l 8 16")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        assert_eq!(result, vec!["glyph hangul-($vowel)-l 8 16"]);
    }

    #[test]
    fn rename_name_parts_ref_in_ref() {
        let lines = vec![t("ref hangul-init-$init 0 0")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        assert_eq!(result, vec!["ref hangul-init-$vowel 0 0"]);
    }

    #[test]
    fn rename_name_parts_in_values() {
        let lines = vec![t("name-parts $combo = $init $final")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        assert_eq!(result, vec!["name-parts $combo = $vowel $final"]);
    }

    #[test]
    fn rename_name_parts_no_partial() {
        let lines = vec![t("ref hangul-$initial 0 0")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        // $initial should NOT be renamed to $vowelial
        assert_eq!(result, vec!["ref hangul-$initial 0 0"]);
    }

    #[test]
    fn rename_anchor_plus() {
        let lines = vec![t("anchor +above 4 1")];
        let result = do_rename(&lines, "above", "top", &RenameKind::Point);
        assert_eq!(result, vec!["anchor +top 4 1"]);
    }

    #[test]
    fn rename_anchor_minus() {
        let lines = vec![t("anchor -above 2 1")];
        let result = do_rename(&lines, "above", "top", &RenameKind::Point);
        assert_eq!(result, vec!["anchor -top 2 1"]);
    }

    #[test]
    fn rename_anchor_both_variants() {
        let lines = vec![t("anchor +above 4 1"), t("anchor -above 2 1")];
        let result = do_rename(&lines, "above", "top", &RenameKind::Point);
        assert_eq!(result, vec!["anchor +top 4 1", "anchor -top 2 1"]);
    }

    #[test]
    fn rename_glyph_assert_same() {
        let lines = vec![t("assert same foo bar")];
        let result = do_rename(&lines, "foo", "quux", &RenameKind::Glyph);
        assert_eq!(result, vec!["assert same quux bar"]);
    }

    #[test]
    fn rename_glyph_assert_shape() {
        // Mutation follows the same classification the rename popup uses,
        // so `assert shape` glyph slots rename too.
        let lines = vec![t("assert shape AB : foo : b-upper")];
        let result = do_rename(&lines, "foo", "quux", &RenameKind::Glyph);
        assert_eq!(result, vec!["assert shape AB : quux : b-upper"]);
    }

    /// A slice id is one id wherever it is written: the declaration, a
    /// qualifier, a face's include list and a union all move together.
    #[test]
    fn rename_slice_everywhere_it_appears() {
        let lines = vec![
            t("slice narrow"),
            t("slice both = narrow wide"),
            t("face term : narrow"),
            t("map narrow : A = latin-a"),
            t("map wide|narrow : B = latin-b($half)"),
            t("name-parts narrow : $half = -half"),
            t("feature narrow : liga for latn : eq-liga"),
            t("assert shape AB for narrow : a-b"),
        ];
        let result = do_rename(&lines, "narrow", "compact", &RenameKind::Slice);
        assert_eq!(
            result,
            vec![
                "slice compact",
                "slice both = compact wide",
                "face term : compact",
                "map compact : A = latin-a",
                // One slice of a list renames on its own.
                "map wide|compact : B = latin-b($half)",
                "name-parts compact : $half = -half",
                "feature compact : liga for latn : eq-liga",
                "assert shape AB for compact : a-b",
            ],
        );
    }

    /// Faces and slices are different namespaces, and a `meta` scope is a face.
    #[test]
    fn rename_face_leaves_slices_alone() {
        let lines = vec![
            t("face term : narrow"),
            t("meta term : family Unison Term"),
            t("slice term"),
            t("map term : A = latin-a"),
        ];
        let result = do_rename(&lines, "term", "console", &RenameKind::Face);
        assert_eq!(
            result,
            vec![
                "face console : narrow",
                "meta console : family Unison Term",
                // The slice named `term` is a different name entirely.
                "slice term",
                "map term : A = latin-a",
            ],
        );
    }

    /// A remap group is named by every rule that writes into it and by every
    /// `feature` and `after` that points at it; the rule's `:` stays put
    /// whether or not it was written tight against the name.
    #[test]
    fn rename_remap_group_everywhere_it_appears() {
        let lines = vec![
            t("remap group liga reversed after flag"),
            t("remap liga : a -> b"),
            t("remap liga: c -> d"),
            t("remap group other after liga"),
            t("feature dlig for latn : liga"),
        ];
        let result = do_rename(&lines, "liga", "eq-liga", &RenameKind::RemapGroup);
        assert_eq!(
            result,
            vec![
                "remap group eq-liga reversed after flag",
                "remap eq-liga : a -> b",
                "remap eq-liga: c -> d",
                "remap group other after eq-liga",
                "feature dlig for latn : eq-liga",
            ],
        );
    }

    /// A group and a glyph may share a spelling; renaming one leaves the other
    /// alone, in both directions.
    #[test]
    fn rename_remap_group_is_not_a_glyph_rename() {
        let lines = vec![t("remap liga : liga -> b")];
        assert_eq!(
            do_rename(&lines, "liga", "eq-liga", &RenameKind::RemapGroup),
            vec!["remap eq-liga : liga -> b"],
        );
        assert_eq!(
            do_rename(&lines, "liga", "l-i-g-a", &RenameKind::Glyph),
            vec!["remap liga : l-i-g-a -> b"],
        );
    }

    /// The file list a rename opens comes from the parsed items, so a kind
    /// that only appears in an unopened file still gets rewritten.
    #[test]
    fn unopened_files_naming_the_target_are_found() {
        let items = |src: &str| {
            crate::document_io::parse_document_from_str(src, std::path::PathBuf::from("t.unf"))
                .unwrap()
                .items
        };

        for (src, kind) in [
            ("map narrow : A = latin-a\n", RenameKind::Slice),
            ("face term : narrow\n", RenameKind::Slice),
            ("assert shape AB for narrow : a-b\n", RenameKind::Slice),
            ("meta term : family Unison\n", RenameKind::Face),
            ("feature dlig for latn : liga\n", RenameKind::RemapGroup),
            ("remap liga : a -> b\n", RenameKind::RemapGroup),
            ("remap group other after liga\n", RenameKind::RemapGroup),
            // Assert directives name glyphs through their own items, not
            // through `Directive`, so the glyph arm has to look inside them.
            ("assert same liga other\n", RenameKind::Glyph),
            ("assert distinct other liga\n", RenameKind::Glyph),
            ("assert shape AB : liga : other\n", RenameKind::Glyph),
            // An anchor-driven feature names an anchor with no glyph carrying
            // that point anywhere in the file.
            ("feature abvm for hang : anchor liga\n", RenameKind::Point),
            // A color alias references the color it points at.
            ("color light = liga\n", RenameKind::Color),
            // Name patterns embed a `$var` inside larger tokens.
            ("map A = latin-$init\n", RenameKind::NameParts),
            ("remap x : a-$init -> b\n", RenameKind::NameParts),
            ("name-parts $combo = x-$init\n", RenameKind::NameParts),
        ] {
            let name = match kind {
                RenameKind::Slice => "narrow",
                RenameKind::Face => "term",
                RenameKind::NameParts => "$init",
                _ => "liga",
            };
            assert!(
                doc_may_reference(&items(src), name, &kind),
                "{src:?} does not look like it names {name}",
            );
        }
    }

    /// An IDC line names its components as glyphs — and, since a component is
    /// an ordinary name token, it can embed a `$var` too. Both have to open
    /// the file, or a rename reaches only the files that happen to be open.
    #[test]
    fn doc_may_reference_sees_idc_components() {
        let items = |src: &str| {
            crate::document_io::parse_document_from_str(src, std::path::PathBuf::from("t.unf"))
                .unwrap()
                .items
        };

        for (src, kind, name) in [
            ("glyph x 8 16\n⿰ liga other\n", RenameKind::Glyph, "liga"),
            (
                "glyph x 8 16\n⿰ other liga ifexists\n",
                RenameKind::Glyph,
                "liga",
            ),
            (
                "glyph x 8 16\n⿱ 1 a-$init 2 other\n",
                RenameKind::NameParts,
                "$init",
            ),
        ] {
            assert!(
                doc_may_reference(&items(src), name, &kind),
                "{src:?} does not look like it names {name}",
            );
        }
    }

    #[test]
    fn rename_preserves_irregular_spacing() {
        let lines = vec![t("  remap liga :  foo   ->  bar")];
        let result = do_rename(&lines, "foo", "quux", &RenameKind::Glyph);
        assert_eq!(result, vec!["  remap liga :  quux   ->  bar"]);
    }

    #[test]
    fn rename_leaves_unrelated_lines() {
        let lines = vec![t("glyph foo 8 16"), t("ref baz 0 0"), t("map X = foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph bar 8 16", "ref baz 0 0", "map X = bar"]);
    }
    /// A glyph written as `@-bar` is an appearance of `foo-bar`, so a rename
    /// started anywhere rewrites it — and keeps the `@` whenever the new name
    /// is still in the base's family, so the helper goes on following its base.
    #[test]
    fn rename_reaches_a_glyph_written_with_an_at() {
        let lines = vec![
            t("glyph foo"),
            t("ref @-bar"),
            t("glyph @-bar"),
            t("map A = foo-bar"),
        ];
        assert_eq!(
            do_rename(&lines, "foo-bar", "foo-qux", &RenameKind::Glyph),
            vec!["glyph foo", "ref @-qux", "glyph @-qux", "map A = foo-qux"],
        );
        // Out of the family there is no `@` spelling left, so the name is
        // written out rather than quietly pointing somewhere else.
        assert_eq!(
            do_rename(&lines, "foo-bar", "zap", &RenameKind::Glyph),
            vec!["glyph foo", "ref zap", "glyph zap", "map A = zap"],
        );
    }

    /// Renaming the base leaves every `@` alone: they follow it by
    /// construction, and rewriting them would freeze today's name into the file.
    #[test]
    fn renaming_the_base_leaves_its_at_names_alone() {
        let lines = vec![t("glyph foo"), t("ref @-bar"), t("glyph @-bar")];
        assert_eq!(
            do_rename(&lines, "foo", "qux", &RenameKind::Glyph),
            vec!["glyph qux", "ref @-bar", "glyph @-bar"],
        );
    }
}

#[cfg(test)]
mod rename_caret_tests {
    use super::*;
    use crate::document::DocLine;
    use crate::editor::caret::Caret;
    use crate::editor::doc_links::RenameKind;

    fn t(s: &str) -> DocLine {
        DocLine::Text(s.to_string())
    }

    fn caret_after(lines: &[DocLine], caret: Caret, old: &str, new: &str) -> Caret {
        let mut lines = lines.to_vec();
        let mut caret = caret;
        rename_in_place(&mut lines, old, new, &RenameKind::Glyph, Some(&mut caret));
        caret
    }

    /// The caret sits inside the renamed token, so it lands right after the
    /// new name.
    #[test]
    fn caret_lands_after_the_renamed_symbol() {
        let lines = vec![t("glyph foo 8 16")];
        // "glyph fo|o 8 16" → "glyph quux| 8 16"
        let c = caret_after(&lines, Caret { line: 0, col: 8 }, "foo", "quux");
        assert_eq!(c, Caret { line: 0, col: 10 });
    }

    /// Same for a shorter new name, and for a caret already at the token's
    /// start or end.
    #[test]
    fn caret_lands_after_a_shorter_name() {
        let lines = vec![t("glyph foobar 8 16")];
        for col in [6, 11, 12] {
            let c = caret_after(&lines, Caret { line: 0, col }, "foobar", "ab");
            assert_eq!(c, Caret { line: 0, col: 8 }, "caret was at {col}");
        }
    }

    /// Text after the renamed token shifts with it.
    #[test]
    fn caret_after_the_token_shifts() {
        let lines = vec![t("glyph foo 8 16")];
        // caret on the "16"
        let c = caret_after(&lines, Caret { line: 0, col: 12 }, "foo", "quux");
        assert_eq!(c, Caret { line: 0, col: 13 });
    }

    /// Several occurrences on one line: earlier ones shift the caret, and it
    /// still ends up after the occurrence it was on.
    #[test]
    fn caret_shifts_past_earlier_occurrences() {
        let lines = vec![t("glyph foo = foo")];
        // caret at the start of the second "foo" → "glyph quux = quux|"
        let c = caret_after(&lines, Caret { line: 0, col: 12 }, "foo", "quux");
        assert_eq!(c, Caret { line: 0, col: 17 });
    }

    /// A caret on a different line still tracks that line's own rewrite.
    #[test]
    fn caret_on_another_line_tracks_that_line() {
        let lines = vec![t("glyph foo 8 16"), t("ref foo 0 0")];
        let c = caret_after(&lines, Caret { line: 1, col: 9 }, "foo", "quux");
        assert_eq!(c, Caret { line: 1, col: 10 });
    }
}

impl UniformApp {
    pub(super) fn execute_rename(&mut self, action: &crate::editor::document_view::RenameAction) {
        use crate::editor::doc_links::RenameKind;

        // Documents opened below are only appended, so the pane's document
        // indices — and with them the focus — stay valid throughout.
        let mut changed_count = 0usize;

        // First pass: check which unopened files would be affected and open them.
        // Uses already-parsed font_base_docs (in memory) to avoid disk I/O
        // for the check; affected files are loaded in parallel.
        let to_open: Vec<PathBuf> = self
            .font_base_docs
            .iter()
            .filter(|base| {
                !shadowed_by_open(&self.open_documents, &base.path)
                    && doc_may_reference(&base.items, &action.old_name, &action.kind)
            })
            .map(|base| base.path.clone())
            .collect();

        if !to_open.is_empty() {
            let base_docs = &self.font_base_docs;
            let loaded: Vec<_> = std::thread::scope(|s| {
                let handles: Vec<_> = to_open
                    .iter()
                    .map(|path| {
                        let path = path.clone();
                        let base_gen = base_docs
                            .iter()
                            .find(|b| b.path == path)
                            .map(|b| (b.edit_gen, b.content_gen));
                        s.spawn(move || load_open_document(path, base_gen).ok())
                    })
                    .collect();
                handles
                    .into_iter()
                    .filter_map(|h| h.join().ok().flatten())
                    .collect()
            });
            for open_doc in loaded {
                self.open_documents.push(open_doc);
            }
        }

        // Second pass: apply rename in place to all open documents.
        // Only touches Text lines; Grid lines are never cloned or compared.
        for doc in &mut self.open_documents {
            let cursor_before = doc.editor_state.cursor;
            let changed_text = rename_in_place(
                &mut doc.lines,
                &action.old_name,
                &action.new_name,
                &action.kind,
                Some(&mut doc.editor_state.cursor),
            );
            if !changed_text.is_empty() {
                doc.editor_state.undo.break_coalesce();
                let ops: Vec<_> = changed_text
                    .iter()
                    .map(|(idx, old_text)| {
                        let new_text = match &doc.lines[*idx] {
                            DocLine::Text(s) => s.clone(),
                            _ => unreachable!(),
                        };
                        crate::editor::undo::UndoOp::Lines {
                            at: *idx,
                            old: vec![DocLine::Text(old_text.clone())],
                            new: vec![DocLine::Text(new_text)],
                        }
                    })
                    .collect();
                doc.editor_state
                    .undo
                    .push_compound(ops, cursor_before, doc.editor_state.cursor);
                match crate::document_io::derive_document(&doc.lines, doc.document.path.clone()) {
                    Ok((new_doc, _)) => {
                        let items_changed = !doc
                            .document
                            .items
                            .iter()
                            .filter(|i| i.affects_font())
                            .eq(new_doc.items.iter().filter(|i| i.affects_font()));
                        let next_gen = doc.document.edit_gen + 1;
                        let pixel_gen = doc.document.pixel_gen;
                        let content_gen = if items_changed {
                            doc.document.content_gen + 1
                        } else {
                            doc.document.content_gen
                        };
                        doc.document = new_doc;
                        doc.document.dirty = true;
                        doc.document.edit_gen = next_gen;
                        doc.document.pixel_gen = pixel_gen;
                        doc.document.content_gen = content_gen;
                    }
                    Err(_) => {
                        doc.document.dirty = true;
                        doc.document.edit_gen += 1;
                    }
                }
                changed_count += 1;
            }
        }

        if changed_count > 0 {
            let kind_str = match action.kind {
                RenameKind::Glyph => "glyph",
                RenameKind::NameParts => "name-parts",
                RenameKind::Point => "point",
                RenameKind::Color => "color",
                RenameKind::Face => "face",
                RenameKind::Slice => "slice",
                RenameKind::RemapGroup => "remap group",
            };
            self.set_status(format!(
                "Renamed {} '{}' → '{}' ({} file{})",
                kind_str,
                action.old_name,
                action.new_name,
                changed_count,
                if changed_count == 1 { "" } else { "s" },
            ));
        }
    }
}
