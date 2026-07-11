use crate::document::DocLine;
use crate::document_io::{tokenize_tokens, tokenize_with_spans, TokenSpan};

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
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenameKind {
    Glyph,
    NameParts,
    Point,
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
    if spans.first().map_or(true, |t| t.value != prefix.trim()) {
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
    let trimmed = line.trim_start();
    let leading = line.chars().count() - trimmed.chars().count();

    let spans = match tokenize_with_spans(trimmed) {
        Ok(s) if !s.is_empty() => s,
        _ => return Vec::new(),
    };

    let keyword = spans[0].value.as_str();
    let rest = &spans[1..];

    match keyword {
        "ref" => {
            let name_span = match rest.first() {
                Some(s) if !s.value.is_empty() => s,
                _ => return Vec::new(),
            };
            extract_glyph_and_parts_links(&name_span.value, leading + name_span.raw_start)
        }
        "glyph" => {
            if let Some(eq_pos) = rest.iter().position(|s| s.value == "=") {
                if let Some(alias_span) = rest.get(eq_pos + 1) {
                    let alias_base = leading + alias_span.raw_start;
                    let mut links = extract_glyph_and_parts_links(&alias_span.value, alias_base);
                    if let Some(name_span) = rest.first() {
                        links.extend(extract_name_parts_vars(&name_span.value, leading + name_span.raw_start));
                    }
                    return links;
                }
            }
            let name_span = match rest.first() {
                Some(s) if !s.value.is_empty() => s,
                _ => return Vec::new(),
            };
            extract_name_parts_vars(&name_span.value, leading + name_span.raw_start)
        }
        "name-parts" => {
            if rest.len() >= 3 && rest[1].value == "=" {
                let mut links = Vec::new();
                for span in &rest[2..] {
                    scan_dollar_refs(&span.value, leading + span.raw_start, &mut links);
                }
                return links;
            }
            Vec::new()
        }
        "map" => {
            if rest.len() == 3 && rest[1].value == "=" {
                let glyph_span = &rest[2];
                return extract_glyph_and_parts_links(&glyph_span.value, leading + glyph_span.raw_start);
            }
            Vec::new()
        }
        "remap" => {
            let mut links = Vec::new();
            for span in rest {
                let clean = span.value.trim_end_matches(':');
                let clean_chars = clean.chars().count();
                if clean_chars > 0 && clean != "->" && clean != ":" {
                    links.extend(extract_glyph_and_parts_links(clean, leading + span.raw_start));
                }
            }
            links
        }
        "feature" => {
            if let Some(colon_pos) = rest.iter().position(|s| s.value == ":") {
                if let Some(remap_span) = rest.get(colon_pos + 1) {
                    return vec![LinkSpan {
                        col_start: leading + remap_span.raw_start,
                        col_end: leading + remap_span.raw_end,
                        target: remap_span.value.clone(),
                        kind: LinkTargetKind::Remap,
                    }];
                }
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
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

fn simple_glyph_rename(span: &TokenSpan, leading: usize, col: usize) -> Option<RenameTarget> {
    let base = leading + span.raw_start;
    let end = leading + span.raw_end;
    if let Some(t) = scan_dollar_ref_at(&span.value, base, col) {
        return Some(t);
    }
    if col >= base && col <= end && !span.value.contains('$') && !span.value.contains('(') {
        return Some(RenameTarget {
            name: span.value.clone(),
            kind: RenameKind::Glyph,
            col_start: base,
            col_end: end,
        });
    }
    None
}

pub(crate) fn find_renameable_at_caret(line: &str, col: usize) -> Option<RenameTarget> {
    let trimmed = line.trim_start();
    let leading = line.chars().count() - trimmed.chars().count();

    let spans = tokenize_with_spans(trimmed).ok()?;
    if spans.is_empty() {
        return None;
    }

    let keyword = spans[0].value.as_str();
    let rest = &spans[1..];

    match keyword {
        "point" => {
            let token_span = rest.first()?;
            if token_span.value.is_empty() {
                return None;
            }
            let name_start = leading + token_span.raw_start;
            let name_end = leading + token_span.raw_end;
            if col >= name_start && col <= name_end {
                let stripped = token_span.value
                    .strip_prefix('+')
                    .or_else(|| token_span.value.strip_prefix('-'))
                    .unwrap_or(&token_span.value);
                return Some(RenameTarget {
                    name: stripped.to_string(),
                    kind: RenameKind::Point,
                    col_start: name_start,
                    col_end: name_end,
                });
            }
            None
        }
        "glyph" => {
            if let Some(eq_pos) = rest.iter().position(|s| s.value == "=") {
                if let Some(alias_span) = rest.get(eq_pos + 1) {
                    // glyph NAME ... = ALIAS form
                    if let Some(def_span) = rest.first() {
                        let def_start = leading + def_span.raw_start;
                        let def_end = leading + def_span.raw_end;
                        if let Some(t) = scan_dollar_ref_at(&def_span.value, def_start, col) {
                            return Some(t);
                        }
                        if col >= def_start && col <= def_end
                            && !def_span.value.contains('$')
                            && !def_span.value.contains('(')
                        {
                            return Some(RenameTarget {
                                name: def_span.value.clone(),
                                kind: RenameKind::Glyph,
                                col_start: def_start,
                                col_end: def_end,
                            });
                        }
                    }
                    return simple_glyph_rename(alias_span, leading, col);
                }
                return None;
            }
            let name_span = rest.first()?;
            if name_span.value.is_empty() {
                return None;
            }
            let base_col = leading + name_span.raw_start;
            let name_end = leading + name_span.raw_end;
            if let Some(t) = scan_dollar_ref_at(&name_span.value, base_col, col) {
                return Some(t);
            }
            if col >= base_col && col <= name_end
                && !name_span.value.contains('$')
                && !name_span.value.contains('(')
            {
                return Some(RenameTarget {
                    name: name_span.value.clone(),
                    kind: RenameKind::Glyph,
                    col_start: base_col,
                    col_end: name_end,
                });
            }
            None
        }
        "ref" => {
            let name_span = rest.first()?;
            if name_span.value.is_empty() {
                return None;
            }
            simple_glyph_rename(name_span, leading, col)
        }
        "name-parts" => {
            if rest.len() >= 3 && rest[1].value == "=" {
                let def_span = &rest[0];
                let def_start = leading + def_span.raw_start;
                let def_end = leading + def_span.raw_end;
                if col >= def_start && col <= def_end {
                    return Some(RenameTarget {
                        name: def_span.value.clone(),
                        kind: RenameKind::NameParts,
                        col_start: def_start,
                        col_end: def_end,
                    });
                }
                // $var in values
                for span in &rest[2..] {
                    if let Some(t) = scan_dollar_ref_at(&span.value, leading + span.raw_start, col) {
                        return Some(t);
                    }
                }
            }
            None
        }
        "map" => {
            if rest.len() == 3 && rest[1].value == "=" {
                let glyph_span = &rest[2];
                return simple_glyph_rename(glyph_span, leading, col);
            }
            None
        }
        "remap" => {
            for span in rest {
                let clean = span.value.trim_end_matches(':');
                let clean_chars = clean.chars().count();
                if clean_chars > 0 && clean != "->" && clean != ":" {
                    let token_col = leading + span.raw_start;
                    let token_end = token_col + clean_chars;
                    if col >= token_col && col <= token_end {
                        if let Some(t) = scan_dollar_ref_at(clean, token_col, col) {
                            return Some(t);
                        }
                        if !clean.contains('$') && !clean.contains('(') {
                            return Some(RenameTarget {
                                name: clean.to_string(),
                                kind: RenameKind::Glyph,
                                col_start: token_col,
                                col_end: token_end,
                            });
                        }
                    }
                }
            }
            None
        }
        "exclude-from-sample" => {
            let name_span = rest.first()?;
            if name_span.value.is_empty() {
                return None;
            }
            simple_glyph_rename(name_span, leading, col)
        }
        _ => None,
    }
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
                    if let Ok(tokens) = tokenize_tokens(trimmed) {
                        if tokens.first().is_some_and(|t| t == "glyph")
                            && tokens.get(1).is_some_and(|t| t == name)
                        {
                            return Some(i);
                        }
                    }
                }
            }
            None
        }
        LinkTargetKind::NameParts => {
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed) {
                        if tokens.first().is_some_and(|t| t == "name-parts")
                            && tokens.get(1).is_some_and(|t| t == name)
                        {
                            return Some(i);
                        }
                    }
                }
            }
            None
        }
        LinkTargetKind::Remap => {
            for (i, line) in lines.iter().enumerate() {
                if let DocLine::Text(s) = line {
                    let trimmed = s.trim();
                    if let Ok(tokens) = tokenize_tokens(trimmed) {
                        if tokens.first().is_some_and(|t| t == "remap") {
                            if let Some(first) = tokens.get(1) {
                                if first == name || first.trim_end_matches(':') == name {
                                    return Some(i);
                                }
                            }
                        }
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
}
