use crate::document::{DocLine, NamePartsMap};
use crate::document_io::{tokenize_tokens, tokenize_with_spans};
use crate::editor::line_fields::{FieldRole, LineField, classify_line};
use crate::pattern::NamePattern;

/// Whether the written name `token` is a pattern that denotes `name`.
///
/// Expanded exactly as the pipeline expands it — name parts substituted first,
/// then the grammar of the context the token sits in: a glyph block name reads
/// a top-level `a|b` as two verbatim names, an operand reads it as one group
/// (`pattern.rs` spells the difference out). Searching for `foo` therefore
/// lists a `glyph fo(o|q)` line, and the span pushed for it is the pattern
/// token as written, so that is what the pane highlights.
///
/// The same test decides where a *definition* is: a glyph block written as a
/// pattern declares every name it expands to, and that is the line a click on
/// one of those names has to reach. Most of `font/` is stated that way — the
/// whole of `han.unf` is seven `glyph han-($#…)` blocks — so matching a name
/// literally would leave those glyphs with no definition to go to at all.
///
/// `parts` is the app's collected map, which holds the *unqualified*
/// `name-parts` bindings only — a pattern spelled with a slice-qualified
/// variable expands to nothing here and is listed only if it matches
/// literally. That is the same map every other editor feature reads
/// (`app/resize.rs`, the derived data in `app/background.rs`), so the search
/// agrees with them rather than being right on its own.
pub fn pattern_denotes(token: &str, is_def: bool, name: &str, parts: &NamePartsMap) -> bool {
    if !token.contains(['(', '|', '$', '*']) {
        return false;
    }
    let substituted = crate::document::substitute_name_parts(token, parts);
    if substituted == name {
        return true;
    }
    if !crate::document::is_name_pattern(&substituted) {
        return false;
    }
    let parsed = if is_def {
        NamePattern::parse(&substituted)
    } else {
        NamePattern::parse_element(&substituted)
    };
    parsed.is_ok_and(|p| p.matches(name))
}

#[derive(Clone, Debug)]
pub(crate) struct LinkSpan {
    pub col_start: usize,
    pub col_end: usize,
    pub target: String,
    pub kind: LinkTargetKind,
    /// The token *is* the name's declaration rather than a use of it, so there
    /// is nowhere for a Ctrl/Cmd+click to go. The host lists every appearance
    /// instead — which is also what a reference whose target does not exist
    /// falls back to, so the two arrive at the same place by different routes.
    pub is_def: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkTargetKind {
    Glyph,
    NameParts,
    Remap,
    Color,
    /// An `anchor` name. Anchors are matched by name across glyphs and are
    /// declared nowhere in particular, so these never navigate.
    Anchor,
    /// An OpenType feature tag, likewise declared nowhere in particular.
    Feature,
    /// A typeface id, declared by a `face` line.
    Face,
    /// A slice id, declared by a `slice` line.
    Slice,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenameKind {
    Glyph,
    NameParts,
    Point,
    Color,
    Face,
    Slice,
    RemapGroup,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct RenameTarget {
    pub name: String,
    pub kind: RenameKind,
    pub col_start: usize,
    pub col_end: usize,
}

pub(crate) fn find_name_col_range_after_prefix(line: &str, prefix: &str) -> Option<(usize, usize)> {
    let trimmed = line.trim_start();
    let leading_chars = line.chars().count() - trimmed.chars().count();
    let spans = tokenize_with_spans(trimmed).ok()?;
    if spans.first().is_none_or(|t| t.value != prefix.trim()) {
        return None;
    }
    let name_span = spans.get(1)?;
    if name_span.value.is_empty() {
        return None;
    }
    Some((
        leading_chars + name_span.raw_start,
        leading_chars + name_span.raw_end,
    ))
}

pub(crate) fn scan_dollar_refs(text: &str, base_col: usize, out: &mut Vec<LinkSpan>) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '-' || chars[i] == '_')
            {
                i += 1;
            }
            if i > start + 1 {
                let var_name: String = chars[start..i].iter().collect();
                out.push(LinkSpan {
                    col_start: base_col + start,
                    col_end: base_col + i,
                    target: var_name,
                    kind: LinkTargetKind::NameParts,
                    is_def: false,
                });
            }
        } else {
            i += 1;
        }
    }
}

fn extract_name_parts_vars(name: &str, base_col: usize) -> Vec<LinkSpan> {
    let mut links = Vec::new();
    scan_dollar_refs(name, base_col, &mut links);
    links
}

fn extract_glyph_and_parts_links(
    name: &str,
    base_col: usize,
    at_base: Option<&str>,
) -> Vec<LinkSpan> {
    let mut links = Vec::new();
    let has_dollar = name.contains('$');
    if has_dollar {
        links.extend(extract_name_parts_vars(name, base_col));
    }
    let name_char_len = name.chars().count();
    if name_char_len > 0 {
        links.push(LinkSpan {
            col_start: base_col,
            col_end: base_col + name_char_len,
            // The link goes where the *glyph* is, so an `@` name links to what
            // it expands to and not to a name nothing declares.
            target: crate::document::expand_at_name(name, at_base),
            kind: LinkTargetKind::Glyph,
            is_def: false,
        });
    }
    links
}

fn whole_field_link(f: &LineField, kind: LinkTargetKind, is_def: bool) -> LinkSpan {
    LinkSpan {
        col_start: f.col_start,
        col_end: f.col_end,
        target: f.token.clone(),
        kind,
        is_def,
    }
}

/// `at_base` is the `@` base in force on this line — see
/// [`crate::document::at_base_at_line`], which is what every caller computes it
/// with.
pub(crate) fn extract_line_links(line: &str, at_base: Option<&str>) -> Vec<LinkSpan> {
    let mut links = Vec::new();
    for f in classify_line(line) {
        match f.role {
            FieldRole::GlyphRef => {
                links.extend(extract_glyph_and_parts_links(
                    &f.token,
                    f.col_start,
                    at_base,
                ));
            }
            // A definition is not a link to itself, but it is still worth
            // clicking: with nothing to go to, the click lists the name's uses.
            // The `$var`s inside a pattern name stay ordinary references, and a
            // pattern name as a whole is not a name anything can refer to, so
            // only plain names get the definition link.
            FieldRole::GlyphDef => {
                links.extend(extract_name_parts_vars(&f.token, f.col_start));
                if !f.token.contains('$') && !f.token.contains('(') {
                    let mut link = whole_field_link(&f, LinkTargetKind::Glyph, true);
                    link.target = crate::document::expand_at_name(&link.target, at_base);
                    links.push(link);
                }
            }
            FieldRole::NamePartsValue => {
                scan_dollar_refs(&f.token, f.col_start, &mut links);
            }
            FieldRole::ColorRef => {
                links.push(whole_field_link(&f, LinkTargetKind::Color, false));
            }
            FieldRole::RemapGroupRef => {
                links.push(whole_field_link(&f, LinkTargetKind::Remap, false));
            }
            FieldRole::NamePartsDef => {
                links.push(whole_field_link(&f, LinkTargetKind::NameParts, true));
            }
            FieldRole::ColorDef => {
                links.push(whole_field_link(&f, LinkTargetKind::Color, true));
            }
            FieldRole::RemapGroupDef => {
                links.push(whole_field_link(&f, LinkTargetKind::Remap, true));
            }
            FieldRole::FeatureDef => {
                links.push(whole_field_link(&f, LinkTargetKind::Feature, true));
            }
            FieldRole::FaceDef => {
                links.push(whole_field_link(&f, LinkTargetKind::Face, true));
            }
            FieldRole::FaceRef => {
                links.push(whole_field_link(&f, LinkTargetKind::Face, false));
            }
            FieldRole::SliceDef => {
                links.push(whole_field_link(&f, LinkTargetKind::Slice, true));
            }
            FieldRole::SliceRef => {
                links.push(whole_field_link(&f, LinkTargetKind::Slice, false));
            }
            // The `+`/`-` prefix says which side of the attachment this is, and
            // both sides are the same anchor, so the link drops it.
            FieldRole::PointDef => {
                let mut link = whole_field_link(&f, LinkTargetKind::Anchor, true);
                link.target = link
                    .target
                    .strip_prefix(['+', '-'])
                    .unwrap_or(&link.target)
                    .to_string();
                links.push(link);
            }
        }
    }
    links
}

fn scan_dollar_ref_at(text: &str, base_col: usize, col: usize) -> Option<RenameTarget> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '-' || chars[i] == '_')
            {
                i += 1;
            }
            if i > start + 1 {
                let abs_start = base_col + start;
                let abs_end = base_col + i;
                if col >= abs_start && col <= abs_end {
                    let var_name: String = chars[start..i].iter().collect();
                    return Some(RenameTarget {
                        name: var_name,
                        kind: RenameKind::NameParts,
                        col_start: abs_start,
                        col_end: abs_end,
                    });
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

/// A glyph-name field is renameable as a whole when it is a plain name;
/// pattern names are only renameable at the `$var` under the caret.
fn glyph_rename_at(field: &LineField, col: usize, at_base: Option<&str>) -> Option<RenameTarget> {
    if let Some(t) = scan_dollar_ref_at(&field.token, field.col_start, col) {
        return Some(t);
    }
    if field.contains_col(col) && !field.token.contains('$') && !field.token.contains('(') {
        return Some(RenameTarget {
            // The glyph being renamed is the one the name resolves to, so an
            // `@` token renames `foo-bar` and not a glyph spelled `@-bar`.
            name: crate::document::expand_at_name(&field.token, at_base),
            kind: RenameKind::Glyph,
            col_start: field.col_start,
            col_end: field.col_end,
        });
    }
    None
}

fn whole_field_rename(field: &LineField, col: usize, kind: RenameKind) -> Option<RenameTarget> {
    if !field.contains_col(col) {
        return None;
    }
    Some(RenameTarget {
        name: field.token.clone(),
        kind,
        col_start: field.col_start,
        col_end: field.col_end,
    })
}

pub(crate) fn find_renameable_at_caret(
    line: &str,
    col: usize,
    at_base: Option<&str>,
) -> Option<RenameTarget> {
    for f in classify_line(line) {
        let target = match f.role {
            FieldRole::GlyphDef | FieldRole::GlyphRef => glyph_rename_at(&f, col, at_base),
            FieldRole::NamePartsValue => scan_dollar_ref_at(&f.token, f.col_start, col),
            FieldRole::NamePartsDef => whole_field_rename(&f, col, RenameKind::NameParts),
            FieldRole::ColorDef | FieldRole::ColorRef => {
                whole_field_rename(&f, col, RenameKind::Color)
            }
            FieldRole::PointDef => whole_field_rename(&f, col, RenameKind::Point).map(|mut t| {
                t.name = t
                    .name
                    .strip_prefix(['+', '-'])
                    .unwrap_or(&t.name)
                    .to_string();
                t
            }),
            // A face or slice id is renamed from either end: the declaration
            // and every qualifier that names it are the same id, and unlike a
            // remap group the declaration is mandatory, so there is a name to
            // rename rather than a spelling that happens to recur.
            FieldRole::FaceDef | FieldRole::FaceRef => {
                whole_field_rename(&f, col, RenameKind::Face)
            }
            FieldRole::SliceDef | FieldRole::SliceRef => {
                whole_field_rename(&f, col, RenameKind::Slice)
            }
            // A remap group has no single declaration site — every `remap`
            // line that writes a rule names it — so a rename rewrites every
            // appearance and there is no "the" definition to start from. The
            // name never leaves the source, so that is the whole rename.
            FieldRole::RemapGroupDef | FieldRole::RemapGroupRef => {
                whole_field_rename(&f, col, RenameKind::RemapGroup)
            }
            // A feature tag is a registered OpenType tag rather than a name of
            // this font's choosing, so renaming one is retyping it.
            FieldRole::FeatureDef => None,
        };
        if target.is_some() {
            return target;
        }
    }
    None
}

/// The line in `lines` that declares `name`, if this document declares it.
///
/// `parts` is only read for the glyph case, where a block's name may be a
/// pattern standing for every name it expands to — see [`pattern_denotes`].
pub fn find_link_target_in_doc(
    lines: &[DocLine],
    name: &str,
    kind: &LinkTargetKind,
    parts: &NamePartsMap,
) -> Option<usize> {
    match kind {
        LinkTargetKind::Glyph => {
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed)
                        && tokens.first().is_some_and(|t| t == "glyph")
                        && tokens
                            .get(1)
                            .is_some_and(|t| t == name || pattern_denotes(t, true, name, parts))
                    {
                        return Some(i);
                    }
                }
            }
            None
        }
        LinkTargetKind::NameParts => {
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed)
                        && tokens.first().is_some_and(|t| t == "name-parts")
                        && tokens.get(1).is_some_and(|t| t == name)
                    {
                        return Some(i);
                    }
                }
            }
            None
        }
        LinkTargetKind::Remap => {
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed)
                        && tokens.first().is_some_and(|t| t == "remap")
                        && let Some(first) = tokens.get(1)
                        && (first == name || first.trim_end_matches(':') == name)
                    {
                        return Some(i);
                    }
                }
            }
            None
        }
        LinkTargetKind::Color => {
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed)
                        && tokens.first().is_some_and(|t| t == "color")
                        && tokens.get(1).is_some_and(|t| t == name)
                    {
                        return Some(i);
                    }
                }
            }
            None
        }
        LinkTargetKind::Face | LinkTargetKind::Slice => {
            let keyword = if *kind == LinkTargetKind::Face {
                "face"
            } else {
                "slice"
            };
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed)
                        && tokens.first().is_some_and(|t| t == keyword)
                        && tokens.get(1).is_some_and(|t| t == name)
                    {
                        return Some(i);
                    }
                }
            }
            None
        }
        // Neither has a declaration site to go to; both only ever search.
        LinkTargetKind::Anchor | LinkTargetKind::Feature => None,
    }
}

#[cfg(test)]
mod rename_detection_tests {
    use super::*;

    #[test]
    fn glyph_header_name() {
        let t = find_renameable_at_caret("glyph foo 8 16", 6, None).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn glyph_header_name_at_end() {
        // col 9 = right after "foo" (col_end), should still match
        let t = find_renameable_at_caret("glyph foo 8 16", 9, None).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn glyph_header_on_dimensions() {
        assert!(find_renameable_at_caret("glyph foo 8 16", 10, None).is_none());
    }

    #[test]
    fn glyph_alias_def_name() {
        let t = find_renameable_at_caret("glyph foo = bar", 6, None).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn glyph_alias_target() {
        let t = find_renameable_at_caret("glyph foo = bar", 12, None).unwrap();
        assert_eq!(t.name, "bar");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    /// Flags before the `=` no longer parse (an alias takes none), but the
    /// editor reads the line as text and must keep working on one being typed
    /// or half-migrated.
    fn glyph_alias_target_survives_stray_flags() {
        let t = find_renameable_at_caret("glyph foo advance 8 = bar", 23, None).unwrap();
        assert_eq!(t.name, "bar");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn feature_for_script_links_remap_group() {
        let links = extract_line_links("feature ljmo for hang : hangul-ljmo", None);
        // The group is a reference and navigates; the tag names nothing else,
        // so it only ever searches.
        assert_eq!(
            links
                .iter()
                .map(|l| (l.target.as_str(), l.kind, l.is_def))
                .collect::<Vec<_>>(),
            vec![
                ("ljmo", LinkTargetKind::Feature, true),
                ("hangul-ljmo", LinkTargetKind::Remap, false),
            ],
        );
    }

    /// Every definition is clickable too — with nothing to go to, the click
    /// asks for the name's appearances instead.
    #[test]
    fn definitions_are_links_marked_as_definitions() {
        for (line, target, kind) in [
            ("glyph foo 8 16", "foo", LinkTargetKind::Glyph),
            ("name-parts $init = a b", "$init", LinkTargetKind::NameParts),
            ("color red = #ff0000", "red", LinkTargetKind::Color),
            ("remap liga : a -> b", "liga", LinkTargetKind::Remap),
        ] {
            let links = extract_line_links(line, None);
            let found = links
                .iter()
                .find(|l| l.target == target)
                .unwrap_or_else(|| panic!("no link for {target} in {line:?}"));
            assert_eq!(found.kind, kind, "{line:?}");
            assert!(found.is_def, "{line:?}");
        }
    }

    /// An anchor is one anchor whichever sign it carries, so the link drops it.
    #[test]
    fn an_anchor_link_drops_its_sign() {
        for line in ["anchor +above 4 1", "anchor -above 2 1"] {
            let links = extract_line_links(line, None);
            assert_eq!(links.len(), 1, "{line:?}");
            assert_eq!(links[0].target, "above");
            assert_eq!(links[0].kind, LinkTargetKind::Anchor);
            assert!(links[0].is_def);
        }
    }

    /// A pattern name is not a name anything can refer to, so only the `$var`s
    /// inside it are links.
    #[test]
    fn a_pattern_glyph_definition_links_only_its_variables() {
        let links = extract_line_links("glyph hangul-($init)-l 8 16", None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "$init");
        assert!(!links[0].is_def);
    }

    #[test]
    fn ref_name() {
        let t = find_renameable_at_caret("ref latin-a 0 0", 4, None).unwrap();
        assert_eq!(t.name, "latin-a");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn map_name() {
        let t = find_renameable_at_caret("map A = latin-a", 8, None).unwrap();
        assert_eq!(t.name, "latin-a");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn name_parts_def() {
        let t = find_renameable_at_caret("name-parts $init = a b c", 11, None).unwrap();
        assert_eq!(t.name, "$init");
        assert_eq!(t.kind, RenameKind::NameParts);
    }

    #[test]
    fn name_parts_ref_in_values() {
        let t = find_renameable_at_caret("name-parts $combo = $init $final", 20, None).unwrap();
        assert_eq!(t.name, "$init");
        assert_eq!(t.kind, RenameKind::NameParts);
    }

    #[test]
    fn dollar_var_in_glyph_header() {
        let t = find_renameable_at_caret("glyph hangul-($init)-l 8 16", 14, None).unwrap();
        assert_eq!(t.name, "$init");
        assert_eq!(t.kind, RenameKind::NameParts);
    }

    #[test]
    fn pattern_glyph_non_var_part() {
        // Caret on the non-$var part of a pattern name — not renameable
        assert!(find_renameable_at_caret("glyph hangul-($init)-l 8 16", 6, None).is_none());
    }

    #[test]
    fn point_plus() {
        let t = find_renameable_at_caret("anchor +above 4 1", 7, None).unwrap();
        assert_eq!(t.name, "above");
        assert_eq!(t.kind, RenameKind::Point);
    }

    #[test]
    fn point_minus() {
        let t = find_renameable_at_caret("anchor -above 2 1", 8, None).unwrap();
        assert_eq!(t.name, "above");
        assert_eq!(t.kind, RenameKind::Point);
    }

    #[test]
    fn point_on_coords() {
        assert!(find_renameable_at_caret("anchor +above 4 1", 15, None).is_none());
    }

    #[test]
    fn remap_token() {
        let t = find_renameable_at_caret("remap liga : a -> b", 13, None).unwrap();
        assert_eq!(t.name, "a");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn exclude_from_sample() {
        let t = find_renameable_at_caret("exclude-from-sample foo", 20, None).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn empty_line() {
        assert!(find_renameable_at_caret("", 0, None).is_none());
    }

    #[test]
    fn comment_line() {
        assert!(find_renameable_at_caret("# some comment", 2, None).is_none());
    }

    #[test]
    fn color_def_name() {
        let t = find_renameable_at_caret("color red = #ff0000", 6, None).unwrap();
        assert_eq!(t.name, "red");
        assert_eq!(t.kind, RenameKind::Color);
    }

    #[test]
    fn color_def_value_hex_is_renameable() {
        // Hex values are not color name references, but they are still renameable
        // (a hex value is not a color name, so no rename target)
        assert!(find_renameable_at_caret("color red = #ff0000", 12, None).is_none());
    }

    #[test]
    fn color_def_value_name_ref() {
        let t = find_renameable_at_caret("color light-red = red", 18, None).unwrap();
        assert_eq!(t.name, "red");
        assert_eq!(t.kind, RenameKind::Color);
    }

    #[test]
    fn ref_fill_color_name() {
        let t = find_renameable_at_caret("ref foo 0 0 fill red", 17, None).unwrap();
        assert_eq!(t.name, "red");
        assert_eq!(t.kind, RenameKind::Color);
    }

    #[test]
    fn ref_fill_fg_not_renameable() {
        assert!(find_renameable_at_caret("ref foo 0 0 fill fg", 17, None).is_none());
    }

    #[test]
    fn ref_fill_hex_not_renameable() {
        assert!(find_renameable_at_caret("ref foo 0 0 fill #ff0000", 17, None).is_none());
    }

    #[test]
    fn color_links_value_name() {
        let links = extract_line_links("color light-red = red", None);
        assert_eq!(
            links
                .iter()
                .map(|l| (l.target.as_str(), l.is_def))
                .collect::<Vec<_>>(),
            vec![("light-red", true), ("red", false)],
        );
        assert!(links.iter().all(|l| l.kind == LinkTargetKind::Color));
    }

    #[test]
    fn color_links_value_hex_no_link() {
        // The hex value is not a name; only the color being defined is.
        let links = extract_line_links("color red = #ff0000", None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "red");
        assert!(links[0].is_def);
    }

    #[test]
    fn ref_fill_links_color_name() {
        let links = extract_line_links("ref foo 0 0 fill red", None);
        // Should have glyph link for 'foo' AND color link for 'red'
        assert!(
            links
                .iter()
                .any(|l| l.target == "red" && matches!(l.kind, LinkTargetKind::Color))
        );
        assert!(
            links
                .iter()
                .any(|l| l.target == "foo" && matches!(l.kind, LinkTargetKind::Glyph))
        );
    }

    #[test]
    fn ref_fill_links_fg_no_color_link() {
        let links = extract_line_links("ref foo 0 0 fill fg", None);
        assert!(
            !links
                .iter()
                .any(|l| matches!(l.kind, LinkTargetKind::Color))
        );
    }

    #[test]
    fn ref_fill_links_hex_no_color_link() {
        let links = extract_line_links("ref foo 0 0 fill #ff0000", None);
        assert!(
            !links
                .iter()
                .any(|l| matches!(l.kind, LinkTargetKind::Color))
        );
    }

    #[test]
    fn assert_same_links_glyph_names() {
        let links = extract_line_links("assert same foo bar", None);
        assert_eq!(links.len(), 2);
        assert!(
            links
                .iter()
                .any(|l| l.target == "foo" && matches!(l.kind, LinkTargetKind::Glyph))
        );
        assert!(
            links
                .iter()
                .any(|l| l.target == "bar" && matches!(l.kind, LinkTargetKind::Glyph))
        );
    }

    #[test]
    fn assert_distinct_links_glyph_names() {
        let links = extract_line_links("assert distinct a b c", None);
        assert_eq!(links.len(), 3);
        assert!(links.iter().any(|l| l.target == "a"));
        assert!(links.iter().any(|l| l.target == "b"));
        assert!(links.iter().any(|l| l.target == "c"));
    }

    #[test]
    fn assert_same_comment_not_linked() {
        let links = extract_line_links("assert same foo bar // not a glyph", None);
        let glyph_names: Vec<&str> = links
            .iter()
            .filter(|l| matches!(l.kind, LinkTargetKind::Glyph))
            .map(|l| l.target.as_str())
            .collect();
        assert_eq!(glyph_names.len(), 2);
        assert!(glyph_names.contains(&"foo"));
        assert!(glyph_names.contains(&"bar"));
    }

    #[test]
    fn assert_shape_links_glyph_names() {
        let links = extract_line_links("assert shape AB : a-upper : b-upper", None);
        assert!(
            links
                .iter()
                .any(|l| l.target == "a-upper" && matches!(l.kind, LinkTargetKind::Glyph))
        );
        assert!(
            links
                .iter()
                .any(|l| l.target == "b-upper" && matches!(l.kind, LinkTargetKind::Glyph))
        );
    }

    #[test]
    fn assert_same_rename_glyph() {
        let t = find_renameable_at_caret("assert same foo bar", 12, None).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn assert_distinct_rename_glyph() {
        let t = find_renameable_at_caret("assert distinct abc def", 16, None).unwrap();
        assert_eq!(t.name, "abc");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn assert_same_rename_not_on_keyword() {
        assert!(find_renameable_at_caret("assert same foo bar", 0, None).is_none());
        assert!(find_renameable_at_caret("assert same foo bar", 7, None).is_none());
    }

    #[test]
    fn assert_shape_rename_glyph() {
        let t = find_renameable_at_caret("assert shape AB : a-upper : b-upper", 18, None).unwrap();
        assert_eq!(t.name, "a-upper");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn assert_rename_not_in_comment() {
        assert!(find_renameable_at_caret("assert same foo bar // comment", 23, None).is_none());
    }

    /// A face declares its id and refers to the slices it includes; a slice
    /// declares its id and refers to the ones it unions.
    /// F2 works from either end of a face, slice or remap-group id — the
    /// declaration and every use are the same name.
    #[test]
    fn faces_slices_and_remap_groups_are_renameable() {
        for (line, col, name, kind) in [
            ("face term : narrow", 6, "term", RenameKind::Face),
            ("face term : narrow", 13, "narrow", RenameKind::Slice),
            ("slice both = narrow wide", 7, "both", RenameKind::Slice),
            ("meta term : family Unison", 6, "term", RenameKind::Face),
            ("map narrow : A = latin-a", 6, "narrow", RenameKind::Slice),
            (
                "assert shape AB for narrow : a-b",
                22,
                "narrow",
                RenameKind::Slice,
            ),
            ("remap liga : a -> b", 7, "liga", RenameKind::RemapGroup),
            (
                "remap group liga after flag",
                13,
                "liga",
                RenameKind::RemapGroup,
            ),
            (
                "remap group liga after flag",
                24,
                "flag",
                RenameKind::RemapGroup,
            ),
            (
                "feature dlig for latn : liga",
                25,
                "liga",
                RenameKind::RemapGroup,
            ),
        ] {
            let t = find_renameable_at_caret(line, col, None)
                .unwrap_or_else(|| panic!("nothing renameable at {col} in {line:?}"));
            assert_eq!(t.name, name, "{line:?}");
            assert_eq!(t.kind, kind, "{line:?}");
        }
        // An OpenType tag is not this font's name to change.
        assert!(find_renameable_at_caret("feature dlig for latn : liga", 9, None).is_none());
        // `*` is "every face", not a face.
        assert!(find_renameable_at_caret("meta * : family Unison", 5, None).is_none());
    }

    #[test]
    fn face_and_slice_lines_link_both_ways() {
        assert_eq!(
            extract_line_links("face term : narrow", None)
                .iter()
                .map(|l| (l.target.as_str(), l.kind, l.is_def))
                .collect::<Vec<_>>(),
            vec![
                ("term", LinkTargetKind::Face, true),
                ("narrow", LinkTargetKind::Slice, false)
            ],
        );
        assert_eq!(
            extract_line_links("slice both = narrow wide", None)
                .iter()
                .map(|l| (l.target.as_str(), l.kind, l.is_def))
                .collect::<Vec<_>>(),
            vec![
                ("both", LinkTargetKind::Slice, true),
                ("narrow", LinkTargetKind::Slice, false),
                ("wide", LinkTargetKind::Slice, false),
            ],
        );
    }

    /// The qualifiers the new directives added are references too — a `map`
    /// under a slice links to the slice as well as to its glyph.
    #[test]
    fn qualifiers_link_to_their_slice_or_face() {
        for (line, target, kind) in [
            ("map narrow : A = latin-a", "narrow", LinkTargetKind::Slice),
            (
                "feature wide : liga for latn : eq-liga",
                "wide",
                LinkTargetKind::Slice,
            ),
            (
                "meta term : family Unison Term",
                "term",
                LinkTargetKind::Face,
            ),
            (
                "assert shape AB for narrow : a-b",
                "narrow",
                LinkTargetKind::Slice,
            ),
        ] {
            let links = extract_line_links(line, None);
            let found = links
                .iter()
                .find(|l| l.target == target)
                .unwrap_or_else(|| panic!("no link for {target} in {line:?}"));
            assert_eq!(found.kind, kind, "{line:?}");
            assert!(!found.is_def, "{line:?}");
        }
        // The glyph on a qualified `map` is still a link of its own.
        assert!(
            extract_line_links("map narrow : A = latin-a", None)
                .iter()
                .any(|l| l.target == "latin-a" && l.kind == LinkTargetKind::Glyph)
        );
    }

    /// The declaration a qualifier points at is what a Ctrl/Cmd+click goes to.
    #[test]
    fn face_and_slice_declarations_are_found_in_a_document() {
        let lines: Vec<DocLine> = ["slice narrow", "", "face term : narrow"]
            .iter()
            .map(|s| DocLine::Text(s.to_string()))
            .collect();
        assert_eq!(
            find_link_target_in_doc(
                &lines,
                "narrow",
                &LinkTargetKind::Slice,
                &NamePartsMap::new()
            ),
            Some(0),
        );
        assert_eq!(
            find_link_target_in_doc(&lines, "term", &LinkTargetKind::Face, &NamePartsMap::new()),
            Some(2),
        );
        // The two ids live in different namespaces and never cross.
        assert_eq!(
            find_link_target_in_doc(
                &lines,
                "narrow",
                &LinkTargetKind::Face,
                &NamePartsMap::new()
            ),
            None,
        );
    }

    /// A glyph block whose name is written as a *pattern* declares every name
    /// the pattern expands to, so a click on one of those names — from the
    /// specimen, or from a `ref` naming it — goes to that line.
    #[test]
    fn a_pattern_glyph_definition_is_found_in_a_document() {
        let lines: Vec<DocLine> = ["glyph latin-a 8 16", "glyph han-($#4e00..9fff) 16 16"]
            .iter()
            .map(|s| DocLine::Text(s.to_string()))
            .collect();
        let parts = NamePartsMap::new();
        assert_eq!(
            find_link_target_in_doc(&lines, "han-4e01", &LinkTargetKind::Glyph, &parts),
            Some(1),
        );
        // A name the pattern does not cover is still not declared here.
        assert_eq!(
            find_link_target_in_doc(&lines, "han-3400", &LinkTargetKind::Glyph, &parts),
            None,
        );
    }

    #[test]
    fn exclude_from_sample_links_glyph_name() {
        let links = extract_line_links("exclude-from-sample foo", None);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "foo");
        assert!(matches!(links[0].kind, LinkTargetKind::Glyph));
    }

    #[test]
    fn assume_unused_links_glyph_names() {
        let links = extract_line_links("assume unused foo bar", None);
        assert!(links.iter().any(|l| l.target == "foo"));
        assert!(links.iter().any(|l| l.target == "bar"));
    }

    /// An `@` name links to the glyph it expands to — the one that exists —
    /// and renaming from it renames that glyph, not a name spelled `@-bar`.
    #[test]
    fn an_at_name_links_to_what_it_expands_to() {
        let links = extract_line_links("ref @-bar", Some("foo"));
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "foo-bar");
        assert!(!links[0].is_def);

        let links = extract_line_links("glyph @-bar", Some("foo"));
        assert_eq!(links[0].target, "foo-bar");
        assert!(links[0].is_def);

        let t = find_renameable_at_caret("ref @-bar", 6, Some("foo")).unwrap();
        assert_eq!(t.name, "foo-bar");
        assert_eq!(t.kind, RenameKind::Glyph);

        // With no base the `@` stands for nothing, and the token is left to be
        // reported as the invalid name it is.
        assert_eq!(extract_line_links("ref @-bar", None)[0].target, "@-bar");
    }
}
