//! Renaming a glyph, name-parts variable or point across every open document.

use super::*;
use super::docs::{load_open_document, shadowed_by_open};

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
    for (i, line) in lines.iter_mut().enumerate() {
        let DocLine::Text(s) = line else { continue };
        if let Some((t, spans)) = rename_in_line(s, old_name, new_name, kind) {
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
                if gn.0 == name { return true; }
                if body.refs.iter().any(|r| r.name == name) { return true; }
            }
            (RenameKind::Glyph, DocumentItem::Map { glyph, .. }) => {
                if glyph == name { return true; }
            }
            (RenameKind::Glyph, DocumentItem::Remap { .. }) => {
                let mut all = item.remap_operands();
                if all.any(|s| s == name) { return true; }
            }
            (RenameKind::Glyph, DocumentItem::Directive(s)) => {
                if s.contains(name) { return true; }
            }
            (RenameKind::NameParts, DocumentItem::NameParts { name: n, values, .. }) => {
                if n == name || values.iter().any(|v| v == name) { return true; }
            }
            (RenameKind::NameParts, DocumentItem::Glyph { name: gn, body }) => {
                if gn.0.contains(name) { return true; }
                if body.refs.iter().any(|r| r.name.contains(name)) { return true; }
            }
            (RenameKind::Point, DocumentItem::Glyph { body, .. }) => {
                let stripped = name.trim_start_matches(['+', '-']);
                if body.points.iter().any(|p| {
                    let ps = p.position.trim_start_matches(['+', '-']);
                    ps == stripped
                }) { return true; }
            }
            (RenameKind::Color, DocumentItem::Color { name: n, .. }) => {
                if n == name { return true; }
            }
            (RenameKind::Color, DocumentItem::Glyph { body, .. }) => {
                if body.refs.iter().any(|r| r.fill.as_ref().is_some_and(|f| f.color == name)) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
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
) -> Option<(String, Vec<(usize, usize, usize)>)> {
    use crate::editor::doc_links::RenameKind;
    use crate::editor::line_fields::{FieldRole, classify_line};

    // (char col start, char col end, replacement)
    let mut reps: Vec<(usize, usize, String)> = Vec::new();
    for f in classify_line(full) {
        let rep = match (kind, f.role) {
            (RenameKind::Glyph, FieldRole::GlyphDef | FieldRole::GlyphRef)
                if f.token == old_name =>
            {
                Some(crate::document_io::quote_token(new_name))
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
            _ => None,
        };
        if let Some(r) = rep {
            reps.push((f.col_start, f.col_end, r));
        }
    }

    if reps.is_empty() {
        return None;
    }

    // Renaming an anchor also migrates the legacy `point` keyword.
    if matches!(kind, RenameKind::Point) {
        let trimmed = full.trim_start();
        if trimmed.starts_with("point ") {
            let leading = full.chars().count() - trimmed.chars().count();
            reps.push((leading, leading + "point".len(), "anchor".to_string()));
        }
    }

    use crate::editor::caret::char_to_byte;
    reps.sort_by_key(|&(start, _, _)| std::cmp::Reverse(start));
    let mut out = full.to_string();
    let mut spans = Vec::with_capacity(reps.len());
    for (start, end, replacement) in reps {
        let byte_start = char_to_byte(&out, start);
        let byte_end = char_to_byte(&out, end);
        spans.push((start, end, replacement.chars().count()));
        out.replace_range(byte_start..byte_end, &replacement);
    }
    spans.reverse(); // back to ascending order
    Some((out, spans))
}

/// Maps a caret column across the replacements one `rename_in_line` made,
/// given as ascending `(char start, char end, new char length)` spans.  A
/// caret anywhere on a rewritten token lands just *after* the new text, so
/// the user sees the symbol they renamed; everything further right shifts
/// with it.
fn shift_caret_col(col: usize, spans: &[(usize, usize, usize)]) -> usize {
    let mut delta: isize = 0;
    for &(start, end, new_len) in spans {
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
                    || !(chars[next_idx].is_alphanumeric() || chars[next_idx] == '-' || chars[next_idx] == '_');
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

    fn t(s: &str) -> DocLine { DocLine::Text(s.to_string()) }

    fn do_rename(lines: &[DocLine], old: &str, new: &str, kind: &RenameKind) -> Vec<String> {
        let mut lines = lines.to_vec();
        rename_in_place(&mut lines, old, new, kind, None);
        lines.into_iter()
            .filter_map(|l| if let DocLine::Text(s) = l { Some(s) } else { None })
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
    fn rename_glyph_alias_after_flags() {
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
    fn rename_point_plus() {
        let lines = vec![t("point +above 4 1")];
        let result = do_rename(&lines, "above", "top", &RenameKind::Point);
        assert_eq!(result, vec!["anchor +top 4 1"]);
    }

    #[test]
    fn rename_point_minus() {
        let lines = vec![t("point -above 2 1")];
        let result = do_rename(&lines, "above", "top", &RenameKind::Point);
        assert_eq!(result, vec!["anchor -top 2 1"]);
    }

    #[test]
    fn rename_point_both_variants() {
        let lines = vec![t("point +above 4 1"), t("point -above 2 1")];
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

    #[test]
    fn rename_preserves_irregular_spacing() {
        let lines = vec![t("  remap liga :  foo   ->  bar")];
        let result = do_rename(&lines, "foo", "quux", &RenameKind::Glyph);
        assert_eq!(result, vec!["  remap liga :  quux   ->  bar"]);
    }

    #[test]
    fn rename_leaves_unrelated_lines() {
        let lines = vec![
            t("glyph foo 8 16"),
            t("ref baz 0 0"),
            t("map X = foo"),
        ];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph bar 8 16", "ref baz 0 0", "map X = bar"]);
    }
}

#[cfg(test)]
mod rename_caret_tests {
    use super::*;
    use crate::document::DocLine;
    use crate::editor::caret::Caret;
    use crate::editor::doc_links::RenameKind;

    fn t(s: &str) -> DocLine { DocLine::Text(s.to_string()) }

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
        let to_open: Vec<PathBuf> = self.font_base_docs.iter()
            .filter(|base| {
                !shadowed_by_open(&self.open_documents, &base.path)
                    && doc_may_reference(&base.items, &action.old_name, &action.kind)
            })
            .map(|base| base.path.clone())
            .collect();

        if !to_open.is_empty() {
            let base_docs = &self.font_base_docs;
            let loaded: Vec<_> = std::thread::scope(|s| {
                let handles: Vec<_> = to_open.iter().map(|path| {
                    let path = path.clone();
                    let base_gen = base_docs.iter().find(|b| b.path == path)
                        .map(|b| (b.edit_gen, b.content_gen));
                    s.spawn(move || load_open_document(path, base_gen).ok())
                }).collect();
                handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
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
                let ops: Vec<_> = changed_text.iter().map(|(idx, old_text)| {
                    let new_text = match &doc.lines[*idx] {
                        DocLine::Text(s) => s.clone(),
                        _ => unreachable!(),
                    };
                    crate::editor::undo::UndoOp::Lines {
                        at: *idx,
                        old: vec![DocLine::Text(old_text.clone())],
                        new: vec![DocLine::Text(new_text)],
                    }
                }).collect();
                doc.editor_state.undo.push_compound(
                    ops,
                    cursor_before,
                    doc.editor_state.cursor,
                );
                match crate::document_io::derive_document(&doc.lines, doc.document.path.clone()) {
                    Ok((new_doc, _)) => {
                        let items_changed = !doc.document.items.iter().filter(|i| i.affects_font())
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
