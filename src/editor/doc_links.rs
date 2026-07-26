use crate::document::DocLine;
use crate::document_io::{tokenize_tokens, tokenize_with_spans};
use crate::editor::line_fields::{FieldRole, LineField, classify_line};

#[derive(Clone, Debug)]
pub(crate) struct LinkSpan {
    pub col_start: usize,
    pub col_end: usize,
    pub target: String,
    pub kind: LinkTargetKind,
}

#[derive(Clone, Debug)]
pub enum LinkTargetKind {
    Glyph,
    NameParts,
    Remap,
    Color,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenameKind {
    Glyph,
    NameParts,
    Point,
    Color,
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
    Some((leading_chars + name_span.raw_start, leading_chars + name_span.raw_end))
}

fn scan_dollar_refs(text: &str, base_col: usize, out: &mut Vec<LinkSpan>) {
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

fn extract_glyph_and_parts_links(name: &str, base_col: usize) -> Vec<LinkSpan> {
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
            target: name.to_string(),
            kind: LinkTargetKind::Glyph,
        });
    }
    links
}

pub(crate) fn extract_line_links(line: &str) -> Vec<LinkSpan> {
    let mut links = Vec::new();
    for f in classify_line(line) {
        match f.role {
            FieldRole::GlyphRef => {
                links.extend(extract_glyph_and_parts_links(&f.token, f.col_start));
            }
            // Definitions aren't links to themselves, but the `$var`s inside
            // a pattern name are.
            FieldRole::GlyphDef => {
                links.extend(extract_name_parts_vars(&f.token, f.col_start));
            }
            FieldRole::NamePartsValue => {
                scan_dollar_refs(&f.token, f.col_start, &mut links);
            }
            FieldRole::ColorRef => {
                links.push(LinkSpan {
                    col_start: f.col_start,
                    col_end: f.col_end,
                    target: f.token,
                    kind: LinkTargetKind::Color,
                });
            }
            FieldRole::RemapGroupRef => {
                links.push(LinkSpan {
                    col_start: f.col_start,
                    col_end: f.col_end,
                    target: f.token,
                    kind: LinkTargetKind::Remap,
                });
            }
            FieldRole::NamePartsDef | FieldRole::PointDef | FieldRole::ColorDef => {}
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
fn glyph_rename_at(field: &LineField, col: usize) -> Option<RenameTarget> {
    if let Some(t) = scan_dollar_ref_at(&field.token, field.col_start, col) {
        return Some(t);
    }
    if field.contains_col(col) && !field.token.contains('$') && !field.token.contains('(') {
        return Some(RenameTarget {
            name: field.token.clone(),
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

pub(crate) fn find_renameable_at_caret(line: &str, col: usize) -> Option<RenameTarget> {
    for f in classify_line(line) {
        let target = match f.role {
            FieldRole::GlyphDef | FieldRole::GlyphRef => glyph_rename_at(&f, col),
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
            // Remap groups have no rename support.
            FieldRole::RemapGroupRef => None,
        };
        if target.is_some() {
            return target;
        }
    }
    None
}

pub fn find_link_target_in_doc(
    lines: &[DocLine],
    name: &str,
    kind: &LinkTargetKind,
) -> Option<usize> {
    match kind {
        LinkTargetKind::Glyph => {
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed)
                        && tokens.first().is_some_and(|t| t == "glyph")
                            && tokens.get(1).is_some_and(|t| t == name)
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
                                && (first == name || first.trim_end_matches(':') == name) {
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
    }
}

#[cfg(test)]
mod rename_detection_tests {
    use super::*;

    #[test]
    fn glyph_header_name() {
        let t = find_renameable_at_caret("glyph foo 8 16", 6).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn glyph_header_name_at_end() {
        // col 9 = right after "foo" (col_end), should still match
        let t = find_renameable_at_caret("glyph foo 8 16", 9).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn glyph_header_on_dimensions() {
        assert!(find_renameable_at_caret("glyph foo 8 16", 10).is_none());
    }

    #[test]
    fn glyph_alias_def_name() {
        let t = find_renameable_at_caret("glyph foo = bar", 6).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn glyph_alias_target() {
        let t = find_renameable_at_caret("glyph foo = bar", 12).unwrap();
        assert_eq!(t.name, "bar");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn glyph_alias_target_after_flags() {
        let t = find_renameable_at_caret("glyph foo advance 8 = bar", 23).unwrap();
        assert_eq!(t.name, "bar");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn feature_for_script_links_remap_group() {
        let links = extract_line_links("feature ljmo for hang : hangul-ljmo");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "hangul-ljmo");
        assert!(matches!(links[0].kind, LinkTargetKind::Remap));
    }

    #[test]
    fn ref_name() {
        let t = find_renameable_at_caret("ref latin-a 0 0", 4).unwrap();
        assert_eq!(t.name, "latin-a");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn map_name() {
        let t = find_renameable_at_caret("map A = latin-a", 8).unwrap();
        assert_eq!(t.name, "latin-a");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn name_parts_def() {
        let t = find_renameable_at_caret("name-parts $init = a b c", 11).unwrap();
        assert_eq!(t.name, "$init");
        assert_eq!(t.kind, RenameKind::NameParts);
    }

    #[test]
    fn name_parts_ref_in_values() {
        let t = find_renameable_at_caret("name-parts $combo = $init $final", 20).unwrap();
        assert_eq!(t.name, "$init");
        assert_eq!(t.kind, RenameKind::NameParts);
    }

    #[test]
    fn dollar_var_in_glyph_header() {
        let t = find_renameable_at_caret("glyph hangul-($init)-l 8 16", 14).unwrap();
        assert_eq!(t.name, "$init");
        assert_eq!(t.kind, RenameKind::NameParts);
    }

    #[test]
    fn pattern_glyph_non_var_part() {
        // Caret on the non-$var part of a pattern name — not renameable
        assert!(find_renameable_at_caret("glyph hangul-($init)-l 8 16", 6).is_none());
    }

    #[test]
    fn point_plus() {
        let t = find_renameable_at_caret("point +above 4 1", 6).unwrap();
        assert_eq!(t.name, "above");
        assert_eq!(t.kind, RenameKind::Point);
    }

    #[test]
    fn point_minus() {
        let t = find_renameable_at_caret("point -above 2 1", 7).unwrap();
        assert_eq!(t.name, "above");
        assert_eq!(t.kind, RenameKind::Point);
    }

    #[test]
    fn point_on_coords() {
        assert!(find_renameable_at_caret("point +above 4 1", 14).is_none());
    }

    #[test]
    fn remap_token() {
        let t = find_renameable_at_caret("remap liga : a -> b", 13).unwrap();
        assert_eq!(t.name, "a");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn exclude_from_sample() {
        let t = find_renameable_at_caret("exclude-from-sample foo", 20).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn empty_line() {
        assert!(find_renameable_at_caret("", 0).is_none());
    }

    #[test]
    fn comment_line() {
        assert!(find_renameable_at_caret("# some comment", 2).is_none());
    }

    #[test]
    fn color_def_name() {
        let t = find_renameable_at_caret("color red = #ff0000", 6).unwrap();
        assert_eq!(t.name, "red");
        assert_eq!(t.kind, RenameKind::Color);
    }

    #[test]
    fn color_def_value_hex_is_renameable() {
        // Hex values are not color name references, but they are still renameable
        // (a hex value is not a color name, so no rename target)
        assert!(find_renameable_at_caret("color red = #ff0000", 12).is_none());
    }

    #[test]
    fn color_def_value_name_ref() {
        let t = find_renameable_at_caret("color light-red = red", 18).unwrap();
        assert_eq!(t.name, "red");
        assert_eq!(t.kind, RenameKind::Color);
    }

    #[test]
    fn ref_fill_color_name() {
        let t = find_renameable_at_caret("ref foo 0 0 fill red", 17).unwrap();
        assert_eq!(t.name, "red");
        assert_eq!(t.kind, RenameKind::Color);
    }

    #[test]
    fn ref_fill_fg_not_renameable() {
        assert!(find_renameable_at_caret("ref foo 0 0 fill fg", 17).is_none());
    }

    #[test]
    fn ref_fill_hex_not_renameable() {
        assert!(find_renameable_at_caret("ref foo 0 0 fill #ff0000", 17).is_none());
    }

    #[test]
    fn color_links_value_name() {
        let links = extract_line_links("color light-red = red");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "red");
        assert!(matches!(links[0].kind, LinkTargetKind::Color));
    }

    #[test]
    fn color_links_value_hex_no_link() {
        let links = extract_line_links("color red = #ff0000");
        assert!(links.is_empty());
    }

    #[test]
    fn ref_fill_links_color_name() {
        let links = extract_line_links("ref foo 0 0 fill red");
        // Should have glyph link for 'foo' AND color link for 'red'
        assert!(links.iter().any(|l| l.target == "red" && matches!(l.kind, LinkTargetKind::Color)));
        assert!(links.iter().any(|l| l.target == "foo" && matches!(l.kind, LinkTargetKind::Glyph)));
    }

    #[test]
    fn ref_fill_links_fg_no_color_link() {
        let links = extract_line_links("ref foo 0 0 fill fg");
        assert!(!links.iter().any(|l| matches!(l.kind, LinkTargetKind::Color)));
    }

    #[test]
    fn ref_fill_links_hex_no_color_link() {
        let links = extract_line_links("ref foo 0 0 fill #ff0000");
        assert!(!links.iter().any(|l| matches!(l.kind, LinkTargetKind::Color)));
    }

    #[test]
    fn assert_same_links_glyph_names() {
        let links = extract_line_links("assert same foo bar");
        assert_eq!(links.len(), 2);
        assert!(links.iter().any(|l| l.target == "foo" && matches!(l.kind, LinkTargetKind::Glyph)));
        assert!(links.iter().any(|l| l.target == "bar" && matches!(l.kind, LinkTargetKind::Glyph)));
    }

    #[test]
    fn assert_distinct_links_glyph_names() {
        let links = extract_line_links("assert distinct a b c");
        assert_eq!(links.len(), 3);
        assert!(links.iter().any(|l| l.target == "a"));
        assert!(links.iter().any(|l| l.target == "b"));
        assert!(links.iter().any(|l| l.target == "c"));
    }

    #[test]
    fn assert_same_comment_not_linked() {
        let links = extract_line_links("assert same foo bar // not a glyph");
        let glyph_names: Vec<&str> = links.iter()
            .filter(|l| matches!(l.kind, LinkTargetKind::Glyph))
            .map(|l| l.target.as_str())
            .collect();
        assert_eq!(glyph_names.len(), 2);
        assert!(glyph_names.contains(&"foo"));
        assert!(glyph_names.contains(&"bar"));
    }

    #[test]
    fn assert_shape_links_glyph_names() {
        let links = extract_line_links("assert shape AB : a-upper : b-upper");
        assert!(links.iter().any(|l| l.target == "a-upper" && matches!(l.kind, LinkTargetKind::Glyph)));
        assert!(links.iter().any(|l| l.target == "b-upper" && matches!(l.kind, LinkTargetKind::Glyph)));
    }

    #[test]
    fn assert_same_rename_glyph() {
        let t = find_renameable_at_caret("assert same foo bar", 12).unwrap();
        assert_eq!(t.name, "foo");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn assert_distinct_rename_glyph() {
        let t = find_renameable_at_caret("assert distinct abc def", 16).unwrap();
        assert_eq!(t.name, "abc");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn assert_same_rename_not_on_keyword() {
        assert!(find_renameable_at_caret("assert same foo bar", 0).is_none());
        assert!(find_renameable_at_caret("assert same foo bar", 7).is_none());
    }

    #[test]
    fn assert_shape_rename_glyph() {
        let t = find_renameable_at_caret("assert shape AB : a-upper : b-upper", 18).unwrap();
        assert_eq!(t.name, "a-upper");
        assert_eq!(t.kind, RenameKind::Glyph);
    }

    #[test]
    fn assert_rename_not_in_comment() {
        assert!(find_renameable_at_caret("assert same foo bar // comment", 23).is_none());
    }

    #[test]
    fn exclude_from_sample_links_glyph_name() {
        let links = extract_line_links("exclude-from-sample foo");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "foo");
        assert!(matches!(links[0].kind, LinkTargetKind::Glyph));
    }

    #[test]
    fn assume_unused_links_glyph_names() {
        let links = extract_line_links("assume unused foo bar");
        assert!(links.iter().any(|l| l.target == "foo"));
        assert!(links.iter().any(|l| l.target == "bar"));
    }
}
