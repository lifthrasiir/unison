use std::collections::{HashMap, HashSet};

use crate::compose::{Direction, IdcOp, VariantSpec, direction_rank, enclosure_rank};
use crate::document::{DocLine, Document, DocumentItem, NamePartsMap};
use crate::document_io::{TokenSpan, tokenize_with_spans};
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
    kind: CompletionKind,
    slot: Option<IdcSlot>,
}

struct CompletionContext {
    kind: CompletionKind,
    prefix: String,
    replace_start: usize,
    /// Which slot of an enclosing IDC line the name being written fills, if it
    /// is on one at all. Only the ordering below reads it.
    slot: Option<IdcSlot>,
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

    // Triggering with the caret inside a name completes the *whole* name, not
    // the part before the caret: the tail is part of the name being written, so
    // leaving it in place would splice a candidate onto a leftover suffix. The
    // caret moves to the name's end first, and only when the popup does open —
    // a trigger that finds nothing to offer must not move it.
    let col = word_end(line_text, state.cursor.col);

    let ctx = match detect_context(line_text, col) {
        Some(c) => c,
        None => return,
    };

    let all_candidates = collect_candidates(
        &ctx,
        source,
        &at_context(lines, state.cursor.line),
        idc_slot_fit(line_text, col, lines, state.cursor.line),
    );
    if all_candidates.is_empty() {
        return;
    }

    let candidates = filter_candidates(&all_candidates, &ctx.kind, &ctx.prefix, ctx.slot);
    if candidates.is_empty() {
        return;
    }

    state.cursor.col = col;
    state.selection_anchor = None;
    state.autocomplete = Some(AutocompleteState {
        selected: 0,
        scroll_offset: 0,
        replace_start: ctx.replace_start,
        line: state.cursor.line,
        candidates,
        all_candidates,
        kind: ctx.kind,
        slot: ctx.slot,
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

    let candidates = filter_candidates(&ac.all_candidates, &ac.kind, &prefix, ac.slot);

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

    // The caret moves freely while the popup is open (arrows, Home, a click)
    // and only an *edit* re-checks it against the popup; accepting with the
    // caret off the popup's line or before the prefix would splice the
    // candidate into text it never completed, so it just closes the popup.
    if state.cursor.line != ac.line || state.cursor.col < ac.replace_start {
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

/// The `@` base in force where the caret is, for the glyph completion below.
///
/// Kept as an `Option<String>` rather than looked up per candidate so the whole
/// list is rewritten from one answer: a mid-list disagreement would offer two
/// spellings of the same glyph.
fn at_context(lines: &[DocLine], line: usize) -> Option<String> {
    crate::document::at_base_at_line(lines, line)
}

/// Restate a glyph candidate list in the `@` spelling the author is typing.
///
/// Typing `@` is typing the base glyph's name, so from that keystroke on the
/// popup behaves as though the base had been spelled out: only names in the
/// base's family survive, and each is offered as the `@` form — so accepting
/// one writes `@-bar`, not `foo-bar`, and the helper keeps following its base.
/// The base itself becomes a bare `@`.
fn rewrite_as_at_names(
    candidates: Vec<CompletionCandidate>,
    base: &str,
) -> Vec<CompletionCandidate> {
    candidates
        .into_iter()
        .filter_map(|c| {
            let rest = c.label.strip_prefix(base)?;
            Some(CompletionCandidate {
                label: format!("@{rest}"),
                kind: c.kind,
            })
        })
        .collect()
}

/// End of the whitespace-delimited word the caret sits *inside*, in chars.
///
/// The same word `detect_context` reads the prefix from, so a trigger from the
/// middle of one ends up completing all of it. A caret that is not inside a
/// word — in whitespace, at a line's start, right before a word — stays where
/// it is: the word ahead is not one the author is in the middle of writing.
fn word_end(line: &str, col: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    if col > chars.len() || col == 0 || chars[col - 1].is_whitespace() {
        return col;
    }
    let mut end = col;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }
    end
}

/// What a candidate list is actually narrowed by.
///
/// A glyph name's `:` opens its variant suffix, and the variants of one name are
/// exactly the choice the author is making when the caret sits in one — so a
/// glyph prefix stops filtering at its last `:` and the whole family stays
/// listed however much of a suffix is already written. Otherwise `foo:4x` would
/// have to be backspaced down to `foo:` to see what else there is, which is the
/// one moment the list is worth reading. Every other kind filters by what is
/// typed, `:` or no `:`.
fn effective_prefix<'a>(kind: &CompletionKind, prefix: &'a str) -> &'a str {
    match (kind, prefix.rfind(':')) {
        (CompletionKind::Glyph, Some(colon)) => &prefix[..colon + 1],
        _ => prefix,
    }
}

/// The candidates a prefix leaves, in the order they are offered.
///
/// Lexicographic (the order [`collect_candidates`] built) unless this is a
/// variant listing for a slot of an IDC line, in which case the choice has a
/// right answer and D1's tie-break says which: the names claiming this slot's
/// own direction first, unmarked ones next, and the ones marked for another
/// direction last. `sort_by_key` is stable, so each rank keeps its
/// lexicographic order. `slot` is `None` off an IDC line, which is what makes
/// every other completion order alike.
fn filter_candidates(
    all: &[CompletionCandidate],
    kind: &CompletionKind,
    prefix: &str,
    slot: Option<IdcSlot>,
) -> Vec<CompletionCandidate> {
    let prefix = effective_prefix(kind, prefix);
    let mut out: Vec<CompletionCandidate> = all
        .iter()
        .filter(|c| c.label.starts_with(prefix))
        .cloned()
        .collect();
    if *kind == CompletionKind::Glyph
        && let Some(slot) = slot
        && prefix.contains(':')
    {
        out.sort_by_key(|c| slot.rank(&c.label));
    }
    out
}

/// Which slot of an IDC line the caret is writing, and what a name is ranked
/// by for it. The two kinds of line claim different things of a name — a share
/// of an axis says which side it was drawn for, an enclosure's outer part says
/// what it can hold — so the ranking is the slot's to answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdcSlot {
    /// A share of a split's axis, named by the direction it stands for.
    Split(Direction),
    /// One of an enclosure's two; `outer` is the part that does the enclosing.
    Enclosure { outer: bool },
}

impl IdcSlot {
    fn rank(self, name: &str) -> u8 {
        match self {
            Self::Split(d) => direction_rank(name, Some(d)),
            Self::Enclosure { outer } => enclosure_rank(name, outer),
        }
    }
}

/// Which slot of an IDC line the caret is writing in, as the direction that slot
/// stands for — `None` when the line is not an IDC line at all.
///
/// The parts are told from the gaps exactly as the parser and
/// [`crate::editor::line_fields`] tell them apart (a gap is a number), and the
/// slot is how many parts stand before the caret. A caret past the last token is
/// therefore writing the next slot, which is the one an unwritten component
/// would fill.
fn idc_slot(line: &str, col: usize) -> Option<IdcSlot> {
    let (op, before) = idc_op_and_slot(line, col)?;
    match op.walls() {
        None => op.slot_direction(before).map(IdcSlot::Split),
        // Past the second component the caret is writing an offset, not a
        // name, and the popup does not open on a number anyway.
        Some(_) => (before < 2).then_some(IdcSlot::Enclosure { outer: before == 0 }),
    }
}

/// The operator an IDC line writes and how many components stand before the
/// caret. Shared by the slot's ranking and the slot's size filter, which have
/// to agree about which slot is being written.
fn idc_op_and_slot(line: &str, col: usize) -> Option<(IdcOp, usize)> {
    let trimmed = line.trim_start();
    let leading = line.chars().count() - trimmed.chars().count();
    let spans = tokenize_with_spans(trimmed).ok()?;
    let (keyword, rest) = spans.split_first()?;
    let op = IdcOp::from_token(&keyword.value)?;
    let adj_col = col.saturating_sub(leading);
    let before = rest
        .iter()
        .filter(|s| s.raw_end < adj_col && is_idc_part(s))
        .count();
    Some((op, before))
}

fn is_idc_part(span: &TokenSpan) -> bool {
    !span.value.is_empty() && span.value.parse::<i16>().is_err()
}

/// The size test a name has to pass to be offered for an IDC slot: its
/// declared box must be exactly this many cells *across* the split axis.
///
/// This is the one part of the composition rule that is not a matter of taste.
/// A ⿰ line splits the parent's width and hands each part the parent's full
/// height, so a part that is not that tall does not fill the slot it is put in
/// and [`crate::compose`] reports it as an *error* — unlike a name drawn for
/// the other side of the glyph, which is a warning and so is merely ordered
/// last (see [`filter_candidates`]). A listing must not offer what the build
/// would refuse, so these names are dropped outright.
#[derive(Clone, Copy)]
enum SlotFit {
    /// A share of a split's axis: the box must be exactly this many cells
    /// across it.
    Across {
        /// The parent's extent across the axis, in declared cells.
        cells: u16,
        /// Whether the split is horizontal, i.e. which side of a box to read.
        horizontal: bool,
    },
    /// A slot of an enclosure: the outer part is the glyph exactly and the
    /// inner one fits inside it
    /// ([`fits_enclosure_slot`](crate::compose::fits_enclosure_slot)). Both are
    /// errors on the line when they fail, so both are dropped here.
    Enclosure { parent: (u16, u16), outer: bool },
}

impl SlotFit {
    /// Whether `name`, whose header declares `declared`, may fill the slot.
    ///
    /// A name whose box nothing states — a family name, a pattern, a glyph
    /// that resolves to whatever it places — passes: the listing may only drop
    /// what it can show is wrong. A name that states a size in its own `:WxH`
    /// suffix is measured by that when it has no box of its own, since that
    /// suffix is exactly the claim the author is choosing between.
    fn admits(self, name: &str, declared: Option<(u16, u16)>) -> bool {
        let Some(size) = declared.or_else(|| VariantSpec::parse(name).size) else {
            return true;
        };
        match self {
            Self::Across { cells, horizontal } => cells == if horizontal { size.1 } else { size.0 },
            Self::Enclosure { parent, outer } => {
                crate::compose::fits_enclosure_slot(size, parent, outer)
            }
        }
    }
}

/// What the slot the caret is writing demands across the axis, or `None` when
/// the line is not an IDC line or the enclosing glyph declares no box (which
/// `compose` reports on its own — there is nothing to filter by).
fn idc_slot_fit(line: &str, col: usize, lines: &[DocLine], line_idx: usize) -> Option<SlotFit> {
    let (op, before) = idc_op_and_slot(line, col)?;
    let (w, h) = enclosing_glyph_box(lines, line_idx)?;
    Some(match op.walls() {
        None => SlotFit::Across {
            cells: if op.horizontal() { h } else { w },
            horizontal: op.horizontal(),
        },
        Some(_) => SlotFit::Enclosure {
            parent: (w, h),
            outer: before == 0,
        },
    })
}

/// The box the `glyph` header above `line` declares, in declared cells.
///
/// Read off the header's own text rather than the parsed document because the
/// popup opens on a buffer that may be mid-edit; the header is the nearest one
/// above, exactly as [`at_context`] finds the `@` base. An IDC glyph is
/// required to state its `W H` there, so nothing else has to be consulted.
fn enclosing_glyph_box(lines: &[DocLine], line_idx: usize) -> Option<(u16, u16)> {
    let header = lines[..line_idx.min(lines.len())]
        .iter()
        .rev()
        .filter_map(|l| l.as_text())
        .find_map(|t| {
            let tokens = crate::document_io::tokenize_tokens(t.trim()).ok()?;
            (tokens.first()? == "glyph").then_some(tokens)
        })?;
    if header.iter().any(|t| t == "=") {
        return None; // an alias declares nothing of its own
    }
    let flags = crate::document_io::parse_glyph_flag_parts(header.get(2..)?);
    flags
        .extent
        .or_else(|| flags.advance.or(flags.width).zip(flags.height))
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
    let slot = idc_slot(line, col);

    // Check for $ in the current word -> name-parts completion
    if let Some(dp) = word.rfind('$') {
        let dollar_char_offset = word[..dp].chars().count();
        let replace_start = word_start + dollar_char_offset;
        let prefix: String = word[dp..].to_string();
        return Some(CompletionContext {
            kind: CompletionKind::NameParts,
            prefix,
            replace_start,
            slot,
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
            slot,
        });
    }

    let spans = tokenize_with_spans(trimmed).ok()?;
    if spans.is_empty() {
        return Some(CompletionContext {
            kind: CompletionKind::Keyword,
            prefix: word,
            replace_start: word_start,
            slot,
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
            slot,
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
        // A fresh token on an IDC line is the next component; a gap is a number,
        // and a number matches no glyph name, so the popup simply does not open
        // on one.
        kw if IdcOp::from_token(kw).is_some() => {
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

/// `cross` narrows the glyph listing to the names that fit the IDC slot the
/// caret is in; see [`SlotFit`]. It is applied here rather than in
/// [`filter_candidates`] because it does not depend on what is typed, so the
/// popup's stored list is already the admissible one and every keystroke after
/// it filters by prefix alone.
fn collect_candidates(
    ctx: &CompletionContext,
    source: &CompletionSource,
    at_base: &Option<String>,
    cross: Option<SlotFit>,
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
                "audit",
                "exists",
                "face",
                "slice",
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
            let admits =
                |name: &str, declared| cross.is_none_or(|cross| cross.admits(name, declared));
            // The names this document declares outright, so a source that does
            // write a shape-shaped name keeps it below.
            let declared_here: HashSet<String> = source
                .doc
                .items
                .iter()
                .filter_map(|item| match item {
                    DocumentItem::Glyph { name, .. } | DocumentItem::GlyphAlias { name, .. } => {
                        Some(name.display())
                    }
                    _ => None,
                })
                .collect();
            // An on-demand *shape* (`3x10`, `2x1-circle`, `4x8-poly5`) is not a
            // name anyone wrote: it is resolved because some `ref` spelled the
            // geometry out, and there are as many of them as a source cares to
            // spell. Offering them buries the declared names — they lead with a
            // digit, so they sort to the very top — and completing one saves no
            // typing, since the name *is* the shape. The color/mono pair is not
            // one of these: its halves are declared and
            // `parse_on_demand_glyph` never matches it (see
            // `on_demand::detect_color_mono_glyph`).
            let synthesized = |name: &str| {
                crate::on_demand::parse_on_demand_glyph(name).is_some()
                    && !declared_here.contains(name)
            };
            for (name, glyph) in source.named_glyphs {
                if admits(name, glyph.declared_box) && !synthesized(name) {
                    candidates.push(CompletionCandidate {
                        label: name.clone(),
                        kind: CompletionKind::Glyph,
                    });
                }
            }
            // Also add raw glyph names from current document that may not be
            // resolved yet (e.g. pattern names).
            for item in &source.doc.items {
                let (name, declared) = match item {
                    DocumentItem::Glyph { name, body } => (name.display(), body.declared_extent()),
                    DocumentItem::GlyphAlias { name, .. } => (name.display(), None),
                    _ => continue,
                };
                if !source.named_glyphs.contains_key(&name) && admits(&name, declared) {
                    candidates.push(CompletionCandidate {
                        label: name,
                        kind: CompletionKind::Glyph,
                    });
                }
            }
            candidates.sort_by(|a, b| a.label.cmp(&b.label));
            candidates.dedup_by(|a, b| a.label == b.label);
            if ctx.prefix.starts_with('@')
                && let Some(base) = at_base
            {
                candidates = rewrite_as_at_names(candidates, base);
            }
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
            for flag in crate::document_io::GLYPH_FLAG_KEYWORDS {
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

    /// The caret can move while the popup is open (arrows, Home, a click) —
    /// only an edit re-checks it. Accepting with the caret before the prefix
    /// (or on another line) must close the popup untouched, not splice the
    /// candidate around the moved caret.
    #[test]
    fn accepting_with_the_caret_moved_away_only_closes_the_popup() {
        let popup = |line: usize, replace_start: usize| AutocompleteState {
            candidates: vec![CompletionCandidate {
                label: "latin-a".into(),
                kind: CompletionKind::Glyph,
            }],
            selected: 0,
            scroll_offset: 0,
            replace_start,
            line,
            all_candidates: Vec::new(),
            kind: CompletionKind::Glyph,
            slot: None,
        };

        // Caret moved back before the prefix being completed.
        let mut lines = vec![DocLine::Text("ref lat".into())];
        let mut state = crate::editor::EditorState::new();
        state.cursor = Caret::new(0, 2);
        state.autocomplete = Some(popup(0, 4));
        apply_completion(&mut lines, &mut state);
        assert_eq!(lines[0], DocLine::Text("ref lat".into()));
        assert!(state.autocomplete.is_none());
        assert_eq!(state.cursor, Caret::new(0, 2));

        // Caret moved to a different line.
        let mut lines = vec![DocLine::Text("ref lat".into()), DocLine::Text("x".into())];
        let mut state = crate::editor::EditorState::new();
        state.cursor = Caret::new(1, 0);
        state.autocomplete = Some(popup(0, 4));
        apply_completion(&mut lines, &mut state);
        assert_eq!(lines[0], DocLine::Text("ref lat".into()));
        assert_eq!(lines[1], DocLine::Text("x".into()));
        assert!(state.autocomplete.is_none());
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

    /// Typing `@` is typing the base glyph's name: the popup narrows to that
    /// glyph's family and offers each of them in the `@` spelling, so accepting
    /// one keeps the helper following its base instead of freezing today's
    /// name into the file.
    #[test]
    fn typing_at_completes_the_base_glyphs_family() {
        let src = "glyph foo\nref @-b\nglyph @-bar\nglyph @-baz\nglyph other\n";
        let doc = crate::document_io::parse_document_from_str(src, "t.unf".into()).unwrap();
        let lines = crate::document_io::parse_doclines(src);
        let named_glyphs = HashMap::new();
        let name_parts = NamePartsMap::new();
        let source = CompletionSource {
            named_glyphs: &named_glyphs,
            name_parts: &name_parts,
            doc: &doc,
        };

        // The `ref` line sits inside `glyph foo`, so that is what `@` means.
        let at_base = at_context(&lines, 1);
        assert_eq!(at_base.as_deref(), Some("foo"));

        let ctx = detect_context("ref @-b", 7).unwrap();
        assert_eq!(ctx.kind, CompletionKind::Glyph);
        assert_eq!(ctx.prefix, "@-b");
        let all = collect_candidates(&ctx, &source, &at_base, None);
        let labels: Vec<&str> = all.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["@", "@-bar", "@-baz"]);
        let shown: Vec<String> = filter_candidates(&all, &ctx.kind, &ctx.prefix, ctx.slot)
            .into_iter()
            .map(|c| c.label)
            .collect();
        assert_eq!(shown, vec!["@-bar", "@-baz"]);

        // Without the `@` the same context offers the full names, unchanged.
        let ctx = detect_context("ref fo", 6).unwrap();
        let all = collect_candidates(&ctx, &source, &at_base, None);
        let labels: Vec<&str> = all.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["foo", "foo-bar", "foo-baz", "other"]);
    }

    /// A glyph name's variant suffix does not narrow the list: the caret sitting
    /// in one is the author choosing among the family, so the whole family stays
    /// listed instead of having to be backspaced back into view.
    #[test]
    fn a_variant_suffix_lists_the_whole_family() {
        let all: Vec<CompletionCandidate> =
            ["han-53ef", "han-53ef:11x16", "han-53ef:12x16-r", "han-5b50"]
                .iter()
                .map(|n| CompletionCandidate {
                    label: (*n).to_string(),
                    kind: CompletionKind::Glyph,
                })
                .collect();

        let shown = |prefix: &str| -> Vec<String> {
            filter_candidates(&all, &CompletionKind::Glyph, prefix, None)
                .into_iter()
                .map(|c| c.label)
                .collect()
        };
        assert_eq!(
            shown("han-53ef:11x"),
            vec!["han-53ef:11x16", "han-53ef:12x16-r"],
        );
        assert_eq!(shown("han-53ef:"), shown("han-53ef:11x"));
        // Before the `:` the prefix still narrows as it always did.
        assert_eq!(
            shown("han-53"),
            vec!["han-53ef", "han-53ef:11x16", "han-53ef:12x16-r"],
        );

        // And only a glyph name is read this way; every other kind takes the
        // prefix whole.
        let parts = [CompletionCandidate {
            label: "$a:b".to_string(),
            kind: CompletionKind::NameParts,
        }];
        assert!(filter_candidates(&parts, &CompletionKind::NameParts, "$a:c", None).is_empty());
    }

    /// A variant listing for a slot of an IDC line is a choice with a right
    /// answer, so D1's tie-break orders it: this slot's direction first, then
    /// the unmarked names, then the ones marked for another slot. Off an IDC
    /// line — and for a name with no variant suffix — the order stays
    /// lexicographic.
    #[test]
    fn an_idc_slot_orders_its_variant_listing_by_direction() {
        let all: Vec<CompletionCandidate> = ["p:4x16", "p:4x16-l", "p:5x16-c", "p:5x16-r"]
            .iter()
            .map(|n| CompletionCandidate {
                label: (*n).to_string(),
                kind: CompletionKind::Glyph,
            })
            .collect();

        let order = |line: &str, col: usize| -> Vec<String> {
            let ctx = detect_context(line, col).unwrap();
            assert_eq!(ctx.kind, CompletionKind::Glyph);
            filter_candidates(&all, &ctx.kind, &ctx.prefix, ctx.slot)
                .into_iter()
                .map(|c| c.label)
                .collect()
        };

        // ⿰'s first slot is the left one, its last the right one.
        assert_eq!(
            order("⿰ p:4 q:8x16", 5),
            vec!["p:4x16-l", "p:4x16", "p:5x16-c", "p:5x16-r"],
        );
        assert_eq!(
            order("⿰ q:8x16 p:4", 12),
            vec!["p:5x16-r", "p:4x16", "p:4x16-l", "p:5x16-c"],
        );
        // A gap is not a slot: the component after it is still the right one.
        assert_eq!(order("⿰ q:8x16 -1 p:4", 15)[0], "p:5x16-r".to_string());
        // ⿱ names its slots up and down, so no `l`/`r`/`c` name suits it and
        // only the unmarked one is promoted.
        assert_eq!(
            order("⿱ p:4 q:8x16", 5),
            vec!["p:4x16", "p:4x16-l", "p:5x16-c", "p:5x16-r"],
        );

        // Off an IDC line the same listing is lexicographic.
        assert_eq!(
            order("ref p:4", 7),
            vec!["p:4x16", "p:4x16-l", "p:5x16-c", "p:5x16-r"],
        );
        // …as is a listing that is not a variant listing at all.
        let ctx = detect_context("⿰ p q:8x16", 3).unwrap();
        assert_eq!(
            filter_candidates(&all, &ctx.kind, &ctx.prefix, ctx.slot)
                .into_iter()
                .map(|c| c.label)
                .collect::<Vec<_>>(),
            vec!["p:4x16", "p:4x16-l", "p:5x16-c", "p:5x16-r"],
        );
    }

    /// A component fills its slot across the whole width (or height) of the
    /// parent, so a name whose box is the wrong size *across* the split axis
    /// cannot go there at all — `compose` calls it an error, not a warning —
    /// and the listing drops it rather than offering it last the way a name
    /// drawn for the other side is offered.
    #[test]
    fn an_idc_slot_lists_only_the_names_that_fit_across_the_axis() {
        let src = "\
glyph p:5x16 5 16
glyph p:5x10 5 10
glyph p:16x5 16 5
glyph parent 15 16
⿰ p:5 q:10x16
";
        let doc = crate::document_io::parse_document_from_str(src, "t.unf".into()).unwrap();
        let lines = crate::document_io::parse_doclines(src);
        let named_glyphs = HashMap::new();
        let name_parts = NamePartsMap::new();
        let source = CompletionSource {
            named_glyphs: &named_glyphs,
            name_parts: &name_parts,
            doc: &doc,
        };

        // Every `W H` header owns a grid line of its own, so the IDC line is
        // not at its source line number.
        let idc = lines
            .iter()
            .position(|l| l.as_text().is_some_and(|t| t.starts_with('⿰')))
            .unwrap();

        let shown = |line_text: &str, col: usize, line: usize| -> Vec<String> {
            let ctx = detect_context(line_text, col).unwrap();
            assert_eq!(ctx.kind, CompletionKind::Glyph);
            let cross = idc_slot_fit(line_text, col, &lines, line);
            let all = collect_candidates(&ctx, &source, &None, cross);
            filter_candidates(&all, &ctx.kind, &ctx.prefix, ctx.slot)
                .into_iter()
                .map(|c| c.label)
                .collect()
        };

        // The parent is 15x16 and the split is horizontal, so only the 16-tall
        // variant is a candidate at all.
        assert_eq!(shown("⿰ p:5 q:10x16", 5, idc), vec!["p:5x16"]);

        // Off an IDC line there is no slot to fit, so the family lists whole.
        assert_eq!(shown("ref p:5", 7, idc), vec!["p:16x5", "p:5x10", "p:5x16"],);
    }

    /// An enclosure's two slots want opposite things of a name: the outer one
    /// is the glyph exactly and promises a cavity, the inner one fits inside it
    /// and promises none. Both are errors on the line when they fail, so the
    /// listing drops what the build would refuse and orders the rest by which
    /// slot the drawing was made for.
    #[test]
    fn an_enclosure_slot_lists_the_names_made_for_it() {
        let src = "\
glyph p:15x16.9x10 15 16
glyph p:15x16 15 16
glyph p:9x10 9 10
glyph p:16x16 16 16
glyph parent 15 16
\u{2FF4} p:1 q
";
        let doc = crate::document_io::parse_document_from_str(src, "t.unf".into()).unwrap();
        let lines = crate::document_io::parse_doclines(src);
        let named_glyphs = HashMap::new();
        let name_parts = NamePartsMap::new();
        let source = CompletionSource {
            named_glyphs: &named_glyphs,
            name_parts: &name_parts,
            doc: &doc,
        };
        let idc = lines
            .iter()
            .position(|l| l.as_text().is_some_and(|t| t.starts_with('\u{2FF4}')))
            .unwrap();
        let shown = |line_text: &str, col: usize| -> Vec<String> {
            let ctx = detect_context(line_text, col).unwrap();
            let cross = idc_slot_fit(line_text, col, &lines, idc);
            let all = collect_candidates(&ctx, &source, &None, cross);
            filter_candidates(&all, &ctx.kind, &ctx.prefix, ctx.slot)
                .into_iter()
                .map(|c| c.label)
                .collect()
        };

        // The outer slot: only a drawing that is the glyph exactly, and the one
        // promising a cavity first.
        assert_eq!(shown("\u{2FF4} p:1 q", 5), vec!["p:15x16.9x10", "p:15x16"]);
        // The inner slot: anything that fits inside the glyph, and the one
        // promising a cavity last — it was drawn to hold something, not to be
        // held.
        assert_eq!(
            shown("\u{2FF4} p:15x16.9x10 p:1", 18),
            vec!["p:15x16", "p:9x10", "p:15x16.9x10"],
        );
    }

    /// A header's own `@` stands for the base that was already in force, so the
    /// line being edited never counts as its own base — the same rule the
    /// parser walks the file by.
    #[test]
    fn a_helper_header_is_not_its_own_base() {
        let lines = crate::document_io::parse_doclines("glyph foo\nglyph @-bar\nref @-baz\n");
        assert_eq!(at_context(&lines, 1).as_deref(), Some("foo"));
        assert_eq!(at_context(&lines, 2).as_deref(), Some("foo"));
        assert_eq!(at_context(&lines, 0), None);

        // And the completion agrees with the parser about the `:variant`
        // suffix not being part of the base.
        let lines = crate::document_io::parse_doclines("glyph foo:mono\nref @-bar\n");
        assert_eq!(at_context(&lines, 1).as_deref(), Some("foo"));
    }

    /// An on-demand shape (`3x10`, `2x1-circle`, …) is a name the source never
    /// declares: it exists only because something referred to it, and being
    /// digits it sorts to the very top of the list. The popup offers the names
    /// the source actually writes instead — and still offers a shape-shaped
    /// name that a `glyph` block does declare.
    #[test]
    fn on_demand_shapes_are_not_offered() {
        let labels = |src: &str| -> Vec<String> {
            let doc = crate::document_io::parse_document_from_str(src, "t.unf".into()).unwrap();
            let name_parts = NamePartsMap::new();
            let (named_glyphs, _) =
                crate::ref_composite::resolve_named_glyphs_with_parts(&[&doc], &name_parts);
            let source = CompletionSource {
                named_glyphs: &named_glyphs,
                name_parts: &name_parts,
                doc: &doc,
            };
            let ctx = detect_context("ref ", 4).unwrap();
            collect_candidates(&ctx, &source, &None, None)
                .into_iter()
                .map(|c| c.label)
                .collect()
        };

        // `3x10` is resolved (the `ref` names it) but never declared.
        let shown = labels("glyph foo 3 10\nref 3x10\n");
        assert!(shown.contains(&"foo".to_string()));
        assert!(!shown.contains(&"3x10".to_string()), "{shown:?}");

        // Declared outright, it is an ordinary glyph again.
        let shown = labels("glyph 3x10 3 10\n...\nglyph foo 3 10\nref 3x10\n");
        assert!(shown.contains(&"3x10".to_string()), "{shown:?}");
    }
}
