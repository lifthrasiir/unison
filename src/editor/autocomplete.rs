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
    Color,
    RemapGroup,
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

pub(crate) fn update_after_edit(lines: &[DocLine], state: &mut super::EditorState) {
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

/// A bare Ctrl chord on a letter key. `ctrl` and not `command`: off the Mac
/// `command` mirrors `ctrl`, so testing `command` would reject every Ctrl
/// chord, and `mac_cmd` is what rules the Cmd variant out on the Mac.
fn ctrl_letter(i: &egui::InputState, key: egui::Key) -> bool {
    i.modifiers.ctrl
        && !i.modifiers.mac_cmd
        && !i.modifiers.alt
        && !i.modifiers.shift
        && i.key_pressed(key)
}

/// Handles autocomplete-specific key events.
///
/// Ctrl+J and Ctrl+K duplicate Down and Up while the popup is open. Ctrl+J is
/// also what opens it (see `document_view::keys`): opening the popup is the
/// step down from a virtual item before the first candidate, and there is no
/// way back onto it — Ctrl+K on the first candidate is a no-op, not a dismissal.
/// Ctrl+K therefore never reaches the code-point popup while this one is up.
pub(crate) fn handle_keys(
    ui: &egui::Ui,
    lines: &mut [DocLine],
    state: &mut super::EditorState,
) -> HandleResult {
    if state.autocomplete.is_none() {
        return HandleResult::NotConsumed;
    }

    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let up = ui.input(|i| {
        (i.key_pressed(egui::Key::ArrowUp) && !i.modifiers.shift && !i.modifiers.command)
            || ctrl_letter(i, egui::Key::K)
    });
    let down = ui.input(|i| {
        (i.key_pressed(egui::Key::ArrowDown) && !i.modifiers.shift && !i.modifiers.command)
            || ctrl_letter(i, egui::Key::J)
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

pub(crate) fn apply_completion(lines: &mut [DocLine], state: &mut super::EditorState) {
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

fn filter_candidates(all: &[CompletionCandidate], prefix: &str) -> Vec<CompletionCandidate> {
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

    // Comment prose names nothing, so nothing completes inside one.
    let (body, comment) = crate::document_io::split_comment(line);
    if comment.is_some() && col >= body.chars().count() {
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

    let ctx = |kind: CompletionKind| {
        Some(CompletionContext {
            kind,
            prefix: word.clone(),
            replace_start: word_start,
        })
    };

    // A caret on a classified name token completes as that field's kind; the
    // shared classification is what keeps completion, links and rename in
    // agreement about which tokens are names.
    for f in crate::editor::line_fields::classify_line(line) {
        if !f.contains_col(col) {
            continue;
        }
        use crate::editor::line_fields::FieldRole;
        return match f.role {
            FieldRole::GlyphRef => ctx(CompletionKind::Glyph),
            FieldRole::RemapGroupRef => ctx(CompletionKind::RemapGroup),
            FieldRole::ColorRef => ctx(CompletionKind::Color),
            FieldRole::PointDef => ctx(CompletionKind::Point),
            FieldRole::NamePartsDef => ctx(CompletionKind::NameParts),
            // Definitions of new names (and name-parts values without a `$`,
            // which the word check above already handled) get no completion.
            // Remap groups and feature tags are declared by being written, so
            // they are definitions too.
            // Face and slice ids are known across the whole directory while
            // this source is one document, so completing them here would offer
            // only the ids that happen to be declared in the file being
            // edited — which is rarely the file that refers to them.
            FieldRole::GlyphDef
            | FieldRole::NamePartsValue
            | FieldRole::ColorDef
            | FieldRole::RemapGroupDef
            | FieldRole::FeatureDef
            | FieldRole::FaceDef
            | FieldRole::FaceRef
            | FieldRole::SliceDef
            | FieldRole::SliceRef => None,
        };
    }

    // The caret is between tokens or past the end: decide what a *new* token
    // here would be.  This zone knowledge is completion-specific.
    let rest_token_idx = find_rest_token_at(rest, adj_col);
    let past_last = rest_token_idx.is_none() && adj_col > rest.last().map_or(0, |s| s.raw_end);

    match keyword {
        "ref" => {
            if let Some(fill_pos) = rest.iter().position(|s| s.value == "fill")
                && past_last
                && rest.len() == fill_pos + 1
            {
                return ctx(CompletionKind::Color);
            }
            if rest_token_idx.is_none() {
                return ctx(CompletionKind::Glyph);
            }
        }
        "exclude-from-sample" => {
            if rest_token_idx.is_none() {
                return ctx(CompletionKind::Glyph);
            }
        }
        "assume" => {
            if rest.first().is_some_and(|s| s.value == "unused") && rest.len() == 1 && past_last {
                return ctx(CompletionKind::Glyph);
            }
        }
        "glyph" => {
            // Trailing alias position: glyph NAME [flags...] = |
            if let Some(eq_pos) = rest.iter().position(|s| s.value == "=")
                && rest_token_idx.is_none()
                && rest.len() == eq_pos + 1
            {
                return ctx(CompletionKind::Glyph);
            }
            // An alias takes no flags, so a line with an `=` gets none offered
            // however it is being edited.
            if rest.iter().any(|s| s.value == "=") {
                return None;
            }
            // After dims, offer glyph flags
            if rest.len() >= 2 {
                let has_dims = rest.iter().any(|s| s.value.parse::<u16>().is_ok());
                if has_dims {
                    if let Some(idx) = rest_token_idx {
                        if idx >= 2 || (idx >= 1 && rest[0].value.parse::<u16>().is_err()) {
                            return ctx(CompletionKind::GlyphFlag);
                        }
                    } else if past_last {
                        return ctx(CompletionKind::GlyphFlag);
                    }
                }
            }
        }
        "map" => {
            // `map generate CHAR = NAME` *defines* NAME, so completing it from
            // the existing glyph names would only ever suggest a collision.
            if let Some(eq_pos) = rest.iter().position(|s| s.value == "=")
                && rest_token_idx.is_none()
                && rest.len() == eq_pos + 1
                && rest.first().is_none_or(|s| s.value != "generate")
            {
                return ctx(CompletionKind::Glyph);
            }
        }
        "anchor" => {
            if rest_token_idx.is_none() {
                return ctx(CompletionKind::Point);
            }
        }
        "remap" => {
            // On a declaration only `after`'s operand is a name at all; the
            // rest of the line is keywords. A rule's operands are glyphs.
            if crate::editor::line_fields::is_remap_group_decl(rest) {
                let prev = rest.iter().rev().find(|s| s.raw_end <= adj_col);
                if rest_token_idx.is_none() && prev.is_some_and(|s| s.value == "after") {
                    return ctx(CompletionKind::RemapGroup);
                }
            } else if rest_token_idx.is_none() {
                return ctx(CompletionKind::Glyph);
            }
        }
        "name-parts" => {
            if rest_token_idx.is_none() {
                return ctx(CompletionKind::NameParts);
            }
        }
        "feature" => {
            if let Some(colon_pos) = rest.iter().position(|s| s.value == ":")
                && rest_token_idx.is_none()
                && rest.len() == colon_pos + 1
            {
                return ctx(CompletionKind::RemapGroup);
            }
        }
        "color" => {
            if let Some(eq_pos) = rest.iter().position(|s| s.value == "=")
                && rest_token_idx.is_none()
                && rest.len() == eq_pos + 1
            {
                return ctx(CompletionKind::Color);
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
fn find_rest_token_at(rest: &[crate::document_io::TokenSpan], adj_col: usize) -> Option<usize> {
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
                "anchor",
                "map",
                "name-parts",
                "remap",
                "feature",
                "meta",
                "face",
                "slice",
                "exclude-from-sample",
                "assume",
                "color",
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
                let name = match item {
                    DocumentItem::Glyph { name, .. } | DocumentItem::GlyphAlias { name, .. } => {
                        name.display()
                    }
                    _ => continue,
                };
                if !source.named_glyphs.contains_key(&name) {
                    candidates.push(CompletionCandidate {
                        label: name,
                        kind: CompletionKind::Glyph,
                    });
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
                for anchor in &glyph.resolved_anchors {
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
        CompletionKind::RemapGroup => {
            // Doc-local, like the color names below: a group could in principle
            // be spread over files, but every one of them is written where its
            // rules are.
            for item in &source.doc.items {
                let name = match item {
                    DocumentItem::Remap { feature, .. } => feature,
                    DocumentItem::RemapGroup { name, .. } => name,
                    _ => continue,
                };
                candidates.push(CompletionCandidate {
                    label: name.clone(),
                    kind: CompletionKind::RemapGroup,
                });
            }
            candidates.sort_by(|a, b| a.label.cmp(&b.label));
            candidates.dedup_by(|a, b| a.label == b.label);
        }
        CompletionKind::Color => {
            candidates.push(CompletionCandidate {
                label: "fg".to_string(),
                kind: CompletionKind::Color,
            });
            for item in &source.doc.items {
                if let DocumentItem::Color { name, .. } = item {
                    candidates.push(CompletionCandidate {
                        label: name.clone(),
                        kind: CompletionKind::Color,
                    });
                }
            }
            candidates.sort_by(|a, b| a.label.cmp(&b.label));
            candidates.dedup_by(|a, b| a.label == b.label);
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

    /// Comment prose names nothing, so completion stays out of it.
    #[test]
    fn no_completion_inside_a_comment() {
        assert!(detect_context("ref foo // lat", 14).is_none());
        assert!(detect_context("ref foo // $na", 14).is_none());
        // Before the marker the line completes as usual.
        assert!(detect_context("ref lat // note", 7).is_some());
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
    fn detect_point_after_anchor() {
        let ctx = detect_context("anchor +ab", 10).unwrap();
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

    #[test]
    fn detect_color_after_color_eq() {
        let ctx = detect_context("color red = ", 12).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Color);
        assert_eq!(ctx.prefix, "");
    }

    #[test]
    fn detect_color_after_color_eq_partial() {
        let ctx = detect_context("color red = blu", 15).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Color);
        assert_eq!(ctx.prefix, "blu");
    }

    #[test]
    fn detect_color_after_ref_fill() {
        let ctx = detect_context("ref foo 0 0 fill ", 17).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Color);
        assert_eq!(ctx.prefix, "");
    }

    #[test]
    fn detect_color_after_ref_fill_partial() {
        let ctx = detect_context("ref foo 0 0 fill re", 19).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Color);
        assert_eq!(ctx.prefix, "re");
    }

    #[test]
    fn detect_color_keyword() {
        let ctx = detect_context("col", 3).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Keyword);
        assert_eq!(ctx.prefix, "col");
    }

    /// Group names and glyph names are different namespaces, and completing one
    /// with the other is worse than not completing at all.
    #[test]
    fn group_names_complete_where_a_group_is_named() {
        // `after`'s operand, mid-token and at a fresh one.
        let ctx = detect_context("remap group a after fl", 22).unwrap();
        assert_eq!(ctx.kind, CompletionKind::RemapGroup);
        assert_eq!(ctx.prefix, "fl");
        assert_eq!(
            detect_context("remap group a after ", 20).unwrap().kind,
            CompletionKind::RemapGroup,
        );
        // And what a `feature` attaches.
        assert_eq!(
            detect_context("feature calt for DFLT : ", 24).unwrap().kind,
            CompletionKind::RemapGroup,
        );
        assert_eq!(
            detect_context("feature calt for DFLT : asc", 27)
                .unwrap()
                .kind,
            CompletionKind::RemapGroup,
        );
    }

    /// The declaration's own name is a definition, and a rule's operands are
    /// still glyphs — including in a group that is named `group`.
    #[test]
    fn a_group_declaration_does_not_complete_glyphs() {
        assert!(detect_context("remap group a reversed ", 23).is_none());
        assert_eq!(
            detect_context("remap group : ", 14).unwrap().kind,
            CompletionKind::Glyph,
        );
        assert_eq!(
            detect_context("remap grp : a -> ", 17).unwrap().kind,
            CompletionKind::Glyph,
        );
    }
}
