use std::collections::{HashMap, HashSet};

use crate::document::{DocLine, Document, DocumentItem, NamePartsMap};
use crate::document_io::tokenize_with_spans;
use crate::editor::caret::{Caret, char_to_byte};
use crate::ref_composite::ResolvedGlyph;

pub(crate) const MAX_VISIBLE: usize = 10;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CompletionKind {
    Glyph,
    NameParts,
    Point,
    Keyword,
    GlyphFlag,
}

#[derive(Clone, Debug)]
pub(crate) struct CompletionCandidate {
    pub label: String,
    pub kind: CompletionKind,
}

pub(crate) struct AutocompleteState {
    pub candidates: Vec<CompletionCandidate>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub replace_start: usize,
    pub line: usize,
    all_candidates: Vec<CompletionCandidate>,
}

struct CompletionContext {
    kind: CompletionKind,
    prefix: String,
    replace_start: usize,
}

pub(crate) struct CompletionSource<'a> {
    pub named_glyphs: &'a HashMap<String, ResolvedGlyph>,
    pub name_parts: &'a NamePartsMap,
    pub doc: &'a Document,
}

pub(crate) fn trigger(
    lines: &[DocLine],
    state: &mut super::EditorState,
    source: &CompletionSource,
) {
    let line_text = match lines.get(state.cursor.line) {
        Some(DocLine::Text(t)) => t.as_str(),
        _ => return,
    };

    let ctx = match detect_context(line_text, state.cursor.col) {
        Some(c) => c,
        None => return,
    };

    let all_candidates = collect_candidates(&ctx, source);
    if all_candidates.is_empty() {
        return;
    }

    let candidates = filter_candidates(&all_candidates, &ctx.prefix);
    if candidates.is_empty() {
        return;
    }

    state.autocomplete = Some(AutocompleteState {
        selected: 0,
        scroll_offset: 0,
        replace_start: ctx.replace_start,
        line: state.cursor.line,
        candidates,
        all_candidates,
    });
}

pub(crate) fn update_after_edit(
    lines: &[DocLine],
    state: &mut super::EditorState,
) {
    let ac = match &state.autocomplete {
        Some(ac) => ac,
        None => return,
    };

    if state.cursor.line != ac.line {
        state.autocomplete = None;
        return;
    }

    if state.cursor.col < ac.replace_start {
        state.autocomplete = None;
        return;
    }

    let line_text = match &lines[state.cursor.line] {
        DocLine::Text(t) => t,
        _ => {
            state.autocomplete = None;
            return;
        }
    };

    let prefix: String = line_text
        .chars()
        .skip(ac.replace_start)
        .take(state.cursor.col - ac.replace_start)
        .collect();

    let all = &ac.all_candidates;
    let candidates = filter_candidates(all, &prefix);

    if candidates.is_empty() {
        state.autocomplete = None;
        return;
    }

    let ac = state.autocomplete.as_mut().unwrap();
    ac.candidates = candidates;
    ac.selected = ac.selected.min(ac.candidates.len().saturating_sub(1));
    if ac.scroll_offset > ac.selected {
        ac.scroll_offset = ac.selected;
    }
    if ac.selected >= ac.scroll_offset + MAX_VISIBLE {
        ac.scroll_offset = ac.selected + 1 - MAX_VISIBLE;
    }
}

pub(crate) enum HandleResult {
    NotConsumed,
    Consumed,
    TextChanged,
}

/// Handles autocomplete-specific key events.
pub(crate) fn handle_keys(
    ui: &egui::Ui,
    lines: &mut Vec<DocLine>,
    state: &mut super::EditorState,
) -> HandleResult {
    if state.autocomplete.is_none() {
        return HandleResult::NotConsumed;
    }

    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let up = ui.input(|i| {
        i.key_pressed(egui::Key::ArrowUp) && !i.modifiers.shift && !i.modifiers.command
    });
    let down = ui.input(|i| {
        i.key_pressed(egui::Key::ArrowDown) && !i.modifiers.shift && !i.modifiers.command
    });
    let accept = ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Tab));

    if escape {
        state.autocomplete = None;
        return HandleResult::Consumed;
    }

    if up {
        let ac = state.autocomplete.as_mut().unwrap();
        if ac.selected > 0 {
            ac.selected -= 1;
            if ac.selected < ac.scroll_offset {
                ac.scroll_offset = ac.selected;
            }
        }
        return HandleResult::Consumed;
    }

    if down {
        let ac = state.autocomplete.as_mut().unwrap();
        if ac.selected + 1 < ac.candidates.len() {
            ac.selected += 1;
            if ac.selected >= ac.scroll_offset + MAX_VISIBLE {
                ac.scroll_offset = ac.selected + 1 - MAX_VISIBLE;
            }
        }
        return HandleResult::Consumed;
    }

    if accept {
        apply_completion(lines, state);
        return HandleResult::TextChanged;
    }

    HandleResult::NotConsumed
}

pub(crate) fn handle_accept(lines: &mut Vec<DocLine>, state: &mut super::EditorState) {
    apply_completion(lines, state);
}

fn apply_completion(lines: &mut Vec<DocLine>, state: &mut super::EditorState) {
    let ac = match state.autocomplete.take() {
        Some(ac) => ac,
        None => return,
    };
    if ac.candidates.is_empty() {
        return;
    }

    let candidate = &ac.candidates[ac.selected].label;
    let line_idx = state.cursor.line;
    let DocLine::Text(text) = &lines[line_idx] else {
        return;
    };

    let old_line = text.clone();
    let prefix_bytes = char_to_byte(&old_line, ac.replace_start);
    let cursor_bytes = char_to_byte(&old_line, state.cursor.col);

    let new_line = format!(
        "{}{}{}",
        &old_line[..prefix_bytes],
        candidate,
        &old_line[cursor_bytes..]
    );
    let new_col = ac.replace_start + candidate.chars().count();
    let new_cursor = Caret::new(line_idx, new_col);

    state.undo.break_coalesce();
    state.undo.push_lines(
        line_idx,
        vec![DocLine::Text(old_line)],
        vec![DocLine::Text(new_line.clone())],
        state.cursor,
        new_cursor,
    );
    lines[line_idx] = DocLine::Text(new_line);
    state.cursor = new_cursor;
    state.selection_anchor = None;
}

fn filter_candidates(
    all: &[CompletionCandidate],
    prefix: &str,
) -> Vec<CompletionCandidate> {
    all.iter()
        .filter(|c| c.label.starts_with(prefix))
        .cloned()
        .collect()
}

fn detect_context(line: &str, col: usize) -> Option<CompletionContext> {
    let chars: Vec<char> = line.chars().collect();
    if col > chars.len() {
        return None;
    }

    // Find the current word boundaries (whitespace-delimited)
    let mut word_start = col;
    while word_start > 0 && !chars[word_start - 1].is_whitespace() {
        word_start -= 1;
    }
    let word: String = chars[word_start..col].iter().collect();

    // Check for $ in the current word -> name-parts completion
    if let Some(dp) = word.rfind('$') {
        let dollar_char_offset = word[..dp].chars().count();
        let replace_start = word_start + dollar_char_offset;
        let prefix: String = word[dp..].to_string();
        return Some(CompletionContext {
            kind: CompletionKind::NameParts,
            prefix,
            replace_start,
        });
    }

    let trimmed = line.trim_start();
    let leading = chars.len() - trimmed.chars().count();

    // Empty line or no keyword yet
    if trimmed.is_empty() || col <= leading + keyword_len(trimmed) {
        return Some(CompletionContext {
            kind: CompletionKind::Keyword,
            prefix: word.clone(),
            replace_start: word_start,
        });
    }

    let spans = tokenize_with_spans(trimmed).ok()?;
    if spans.is_empty() {
        return Some(CompletionContext {
            kind: CompletionKind::Keyword,
            prefix: word,
            replace_start: word_start,
        });
    }

    let keyword = spans[0].value.as_str();
    let rest = &spans[1..];
    let adj_col = col.saturating_sub(leading);

    // Find which token index (in rest) the cursor is in or after
    let rest_token_idx = find_rest_token_at(rest, adj_col);

    match keyword {
        "ref" | "exclude-from-sample" => {
            if rest_token_idx <= Some(0) || (rest.is_empty() && adj_col > spans[0].raw_end) {
                return Some(CompletionContext {
                    kind: CompletionKind::Glyph,
                    prefix: word,
                    replace_start: word_start,
                });
            }
        }
        "glyph" => {
            // Check for alias form: glyph NAME [flags...] = ALIAS
            if let Some(eq_pos) = rest.iter().position(|s| s.value == "=") {
                let after_eq = eq_pos + 1;
                match rest_token_idx {
                    Some(idx) if idx >= after_eq => {
                        return Some(CompletionContext {
                            kind: CompletionKind::Glyph,
                            prefix: word,
                            replace_start: word_start,
                        });
                    }
                    None if rest.len() == after_eq => {
                        return Some(CompletionContext {
                            kind: CompletionKind::Glyph,
                            prefix: word,
                            replace_start: word_start,
                        });
                    }
                    _ => {}
                }
            }
            // After dims, offer glyph flags
            if rest.len() >= 2 {
                let has_dims = rest[0].value.parse::<u16>().is_ok()
                    || (rest.len() > 1
                        && rest.iter().any(|s| s.value.parse::<u16>().is_ok()));
                if has_dims {
                    if let Some(idx) = rest_token_idx {
                        if idx >= 2 || (idx >= 1 && rest[0].value.parse::<u16>().is_err()) {
                            return Some(CompletionContext {
                                kind: CompletionKind::GlyphFlag,
                                prefix: word,
                                replace_start: word_start,
                            });
                        }
                    } else if adj_col > rest.last().map_or(0, |s| s.raw_end) {
                        return Some(CompletionContext {
                            kind: CompletionKind::GlyphFlag,
                            prefix: word,
                            replace_start: word_start,
                        });
                    }
                }
            }
        }
        "map" => {
            // map CHAR = GLYPH
            if let Some(eq_pos) = rest.iter().position(|s| s.value == "=") {
                let after_eq = eq_pos + 1;
                match rest_token_idx {
                    Some(idx) if idx >= after_eq => {
                        return Some(CompletionContext {
                            kind: CompletionKind::Glyph,
                            prefix: word,
                            replace_start: word_start,
                        });
                    }
                    None if rest.len() == after_eq => {
                        return Some(CompletionContext {
                            kind: CompletionKind::Glyph,
                            prefix: word,
                            replace_start: word_start,
                        });
                    }
                    _ => {}
                }
            }
        }
        "point" | "anchor" => {
            if rest_token_idx <= Some(0) || (rest.is_empty() && adj_col > spans[0].raw_end) {
                return Some(CompletionContext {
                    kind: CompletionKind::Point,
                    prefix: word,
                    replace_start: word_start,
                });
            }
        }
        "remap" => {
            // All non-structural tokens can be glyph names
            if let Some(idx) = rest_token_idx {
                let val = &rest[idx].value;
                if val != ":" && val != "->" {
                    return Some(CompletionContext {
                        kind: CompletionKind::Glyph,
                        prefix: word,
                        replace_start: word_start,
                    });
                }
            } else if adj_col > spans[0].raw_end {
                return Some(CompletionContext {
                    kind: CompletionKind::Glyph,
                    prefix: word,
                    replace_start: word_start,
                });
            }
        }
        "name-parts" => {
            if rest_token_idx <= Some(0) || (rest.is_empty() && adj_col > spans[0].raw_end) {
                return Some(CompletionContext {
                    kind: CompletionKind::NameParts,
                    prefix: word,
                    replace_start: word_start,
                });
            }
        }
        "feature" => {
            // feature NAME for SCRIPT... : REMAP_GROUP
            if let Some(colon_pos) = rest.iter().position(|s| s.value == ":") {
                let after_colon = colon_pos + 1;
                match rest_token_idx {
                    Some(idx) if idx >= after_colon => {
                        return Some(CompletionContext {
                            kind: CompletionKind::Glyph,
                            prefix: word,
                            replace_start: word_start,
                        });
                    }
                    None if rest.len() == after_colon => {
                        return Some(CompletionContext {
                            kind: CompletionKind::Glyph,
                            prefix: word,
                            replace_start: word_start,
                        });
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }

    None
}

fn keyword_len(trimmed: &str) -> usize {
    trimmed
        .find(|c: char| c.is_whitespace())
        .unwrap_or(trimmed.len())
}

/// Find which rest-token index the cursor falls within.
/// Returns None if the cursor is between/after tokens (not inside any).
fn find_rest_token_at(
    rest: &[crate::document_io::TokenSpan],
    adj_col: usize,
) -> Option<usize> {
    for (i, span) in rest.iter().enumerate() {
        if adj_col >= span.raw_start && adj_col <= span.raw_end {
            return Some(i);
        }
    }
    None
}

fn collect_candidates(
    ctx: &CompletionContext,
    source: &CompletionSource,
) -> Vec<CompletionCandidate> {
    let mut candidates = Vec::new();

    match ctx.kind {
        CompletionKind::Keyword => {
            let keywords = [
                "glyph",
                "ref",
                "point",
                "anchor",
                "map",
                "name-parts",
                "remap",
                "feature",
                "font-meta",
                "exclude-from-sample",
            ];
            for kw in &keywords {
                candidates.push(CompletionCandidate {
                    label: kw.to_string(),
                    kind: CompletionKind::Keyword,
                });
            }
        }
        CompletionKind::Glyph => {
            for name in source.named_glyphs.keys() {
                candidates.push(CompletionCandidate {
                    label: name.clone(),
                    kind: CompletionKind::Glyph,
                });
            }
            // Also add raw glyph names from current document that may not be
            // resolved yet (e.g. pattern names).
            for item in &source.doc.items {
                if let DocumentItem::Glyph { name, .. } = item {
                    let n = name.display();
                    if !source.named_glyphs.contains_key(&n) {
                        candidates.push(CompletionCandidate {
                            label: n,
                            kind: CompletionKind::Glyph,
                        });
                    }
                }
            }
            candidates.sort_by(|a, b| a.label.cmp(&b.label));
            candidates.dedup_by(|a, b| a.label == b.label);
        }
        CompletionKind::NameParts => {
            for name in source.name_parts.keys() {
                candidates.push(CompletionCandidate {
                    label: name.clone(),
                    kind: CompletionKind::NameParts,
                });
            }
            candidates.sort_by(|a, b| a.label.cmp(&b.label));
        }
        CompletionKind::Point => {
            let mut seen = HashSet::new();
            for glyph in source.named_glyphs.values() {
                for anchor in &glyph.anchors {
                    if seen.insert(anchor.position.clone()) {
                        candidates.push(CompletionCandidate {
                            label: anchor.position.clone(),
                            kind: CompletionKind::Point,
                        });
                    }
                }
            }
            candidates.sort_by(|a, b| a.label.cmp(&b.label));
        }
        CompletionKind::GlyphFlag => {
            for flag in &["sticky", "inline", "advance", "left"] {
                candidates.push(CompletionCandidate {
                    label: flag.to_string(),
                    kind: CompletionKind::GlyphFlag,
                });
            }
        }
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_keyword_on_empty_line() {
        let ctx = detect_context("", 0).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Keyword);
        assert_eq!(ctx.prefix, "");
    }

    #[test]
    fn detect_keyword_partial() {
        let ctx = detect_context("re", 2).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Keyword);
        assert_eq!(ctx.prefix, "re");
    }

    #[test]
    fn detect_glyph_after_ref() {
        let ctx = detect_context("ref ", 4).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Glyph);
        assert_eq!(ctx.prefix, "");
    }

    #[test]
    fn detect_glyph_after_ref_partial() {
        let ctx = detect_context("ref lat", 7).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Glyph);
        assert_eq!(ctx.prefix, "lat");
    }

    #[test]
    fn detect_glyph_after_map_eq() {
        let ctx = detect_context("map A = lat", 11).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Glyph);
        assert_eq!(ctx.prefix, "lat");
    }

    #[test]
    fn detect_glyph_after_map_eq_empty() {
        let ctx = detect_context("map A = ", 8).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Glyph);
        assert_eq!(ctx.prefix, "");
    }

    #[test]
    fn detect_name_parts_dollar() {
        let ctx = detect_context("name-parts $in", 14).unwrap();
        assert_eq!(ctx.kind, CompletionKind::NameParts);
        assert_eq!(ctx.prefix, "$in");
    }

    #[test]
    fn detect_name_parts_dollar_in_glyph() {
        let ctx = detect_context("glyph hangul-($in", 17).unwrap();
        assert_eq!(ctx.kind, CompletionKind::NameParts);
        assert_eq!(ctx.prefix, "$in");
    }

    #[test]
    fn detect_point_after_point() {
        let ctx = detect_context("point +ab", 9).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Point);
        assert_eq!(ctx.prefix, "+ab");
    }

    #[test]
    fn detect_glyph_alias() {
        let ctx = detect_context("glyph foo = ba", 14).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Glyph);
        assert_eq!(ctx.prefix, "ba");
    }

    #[test]
    fn detect_exclude_from_sample() {
        let ctx = detect_context("exclude-from-sample fo", 22).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Glyph);
        assert_eq!(ctx.prefix, "fo");
    }

    #[test]
    fn detect_remap_glyph() {
        let ctx = detect_context("remap liga : a -> b", 19).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Glyph);
        assert_eq!(ctx.prefix, "b");
    }

    #[test]
    fn no_context_on_comment() {
        assert!(detect_context("# comment", 5).is_none());
    }
}
