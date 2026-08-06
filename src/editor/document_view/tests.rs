//! Unit tests for the document view's non-interactive helpers.
//!
//! Scenario-level GUI tests live in [`crate::editor::view_tests`].

use super::changes::{self, defer_document_changes, flush_document_changes};
use super::*;
use crate::document_io::{derive_document, parse_doclines};
use crate::edit_menu::EditAction;

#[test]
fn deferred_change_is_dirty_without_advancing_generation_and_is_per_editor() {
    let lines = vec![DocLine::Text("glyph foo 2 2".into())];
    let (mut doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    doc.edit_gen = 7;

    let mut first = EditorState::new();
    first.cursor = Caret::new(0, 4);
    first.undo.push_text(
        0,
        4,
        "a".into(),
        "b".into(),
        Caret::new(0, 4),
        Caret::new(0, 5),
    );
    let second = EditorState::new();

    defer_document_changes(&mut doc, &mut first);

    assert!(doc.dirty);
    assert_eq!(doc.edit_gen, 7);
    assert_eq!(first.pending_reparse_line, Some(0));
    assert_eq!(second.pending_reparse_line, None);
}

#[test]
fn external_edit_action_can_be_flushed_immediately() {
    let mut lines = vec![DocLine::Text("//abc".into())];
    let (mut doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    let mut state = EditorState::new();
    state.selection_anchor = Some(Caret::new(0, 2));
    state.cursor = Caret::new(0, 3);

    assert!(state.apply_edit_action(EditAction::Delete, &mut lines, &egui::Context::default(),));
    flush_document_changes(&mut lines, &mut doc, &mut state);

    assert_eq!(lines, vec![DocLine::Text("//bc".into())]);
    assert!(matches!(
        doc.items.first(),
        Some(crate::document::DocumentItem::Comment(text)) if text == "bc"
    ));
    assert!(doc.dirty);
    assert_eq!(doc.edit_gen, 1);
    assert_eq!(state.pending_reparse_line, None);
    assert!(!state.take_document_sync_request());
}

/// The parser accepts `ref` and `anchor` lines in any order, so a body's
/// layer-to-line mapping cannot assume refs come first: flattening ref 0 of a
/// glyph whose source states an anchor first must remove the *ref* line.
#[test]
fn inline_flatten_removes_the_ref_line_not_an_interleaved_anchor() {
    let mut lines = parse_doclines(
        "glyph stem 2 2\n\
         @@..\n\
         ..@@\n\
         \n\
         glyph comp 2 2\n\
         ....\n\
         ....\n\
         anchor -a 0 0\n\
         ref stem 0 0\n",
    );
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    let name_parts = crate::document::collect_name_parts(&[&doc]);
    let (named, _alt) = ref_composite::resolve_named_glyphs_with_parts(&[&doc], &name_parts);
    let mut state = EditorState::new();
    let comp_idx = doc
        .items
        .iter()
        .position(|i| matches!(i, DocumentItem::Glyph { name, .. } if name.display() == "comp"))
        .unwrap();

    assert!(changes::inline_ref_to_pixels(
        &mut lines,
        &doc,
        &mut state,
        comp_idx,
        0,
        &named,
        &name_parts,
    ));

    let texts: Vec<&str> = lines
        .iter()
        .filter_map(|l| match l {
            DocLine::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        texts.contains(&"anchor -a 0 0"),
        "the anchor line was removed instead of the ref line: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.trim_start().starts_with("ref stem")),
        "the ref line survived flattening: {texts:?}"
    );
}

fn assert_all_doc_lines_covered(input: &str) {
    let lines = parse_doclines(input);
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();

    let last_item_end = doc
        .items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let start = doc.item_line_starts[idx];
            use crate::document::DocumentItem;
            match item {
                DocumentItem::BlankLine
                | DocumentItem::Comment(_)
                | DocumentItem::Directive(_)
                | DocumentItem::Face { .. }
                | DocumentItem::Slice { .. }
                | DocumentItem::Meta(_)
                | DocumentItem::Map { .. }
                | DocumentItem::NameParts { .. }
                | DocumentItem::Remap { .. }
                | DocumentItem::RemapGroup { .. }
                | DocumentItem::Feature { .. }
                | DocumentItem::FeatureAnchor { .. }
                | DocumentItem::MapDecomposed { .. }
                | DocumentItem::Color { .. }
                | DocumentItem::AssertShape { .. }
                | DocumentItem::AssertSame { .. }
                | DocumentItem::AssertDistinct { .. }
                | DocumentItem::GlyphAlias { .. } => start + 1,
                DocumentItem::Glyph { body, .. } => {
                    // One past the glyph's last layer line.
                    pixel_interaction::layer_doc_line(
                        &lines,
                        body,
                        start,
                        body.refs.len() + body.points.len(),
                    )
                }
            }
        })
        .max()
        .unwrap_or(0);

    assert_eq!(
        last_item_end,
        lines.len(),
        "item_line_starts don't cover all {n} DocLines (last item ends at {last_item_end})",
        n = lines.len(),
    );

    // Check that starts are monotonically increasing and match
    for i in 1..doc.item_line_starts.len() {
        assert!(
            doc.item_line_starts[i] > doc.item_line_starts[i - 1],
            "item_line_starts not strictly increasing at {i}: {:?}",
            &doc.item_line_starts[i - 1..=i]
        );
    }
}

#[test]
fn all_lines_covered_alias_then_blank() {
    assert_all_doc_lines_covered(
        "glyph minus = dash\n\
         \n\
         glyph plusminus 8 16\n\
         ................\n\
         ................\n\
         ................\n\
         ................\n\
         ......@@@@......\n\
         ......@@@@......\n\
         ......@@@@......\n\
         ..@@@@@@@@@@@@..\n\
         ......@@@@......\n\
         ......@@@@......\n\
         ......@@@@......\n\
         ................\n\
         ..@@@@@@@@@@@@..\n\
         ................\n\
         ................\n\
         ................\n",
    );
}

#[test]
fn all_lines_covered_consecutive_aliases() {
    assert_all_doc_lines_covered(
        "glyph U+002B = plus\n\
         glyph U+2212 = minus\n\
         glyph U+00B1 = plusminus\n\
         glyph U+2213 = minusplus\n\
         glyph U+00D7 = times\n\
         glyph U+00F7 = div\n",
    );
}

#[test]
fn all_lines_covered_glyph_with_ref_then_alias() {
    assert_all_doc_lines_covered(
        "glyph div 8 16\n\
         ................\n\
         ................\n\
         ................\n\
         ................\n\
         ................\n\
         ......@@@@......\n\
         ......@@@@......\n\
         ................\n\
         ................\n\
         ................\n\
         ......@@@@......\n\
         ......@@@@......\n\
         ................\n\
         ................\n\
         ................\n\
         ................\n\
         ref hyphen 0 0\n\
         \n\
         glyph U+002B = plus\n",
    );
}

#[test]
fn all_lines_covered_ref_only_single_ref_at_origin() {
    assert_all_doc_lines_covered(
        "glyph composite\n\
         ref other 0 0\n\
         \n\
         glyph next 2 2\n\
         ..@@\n\
         @@..\n",
    );
}

#[test]
fn all_lines_covered_all_directive_types() {
    assert_all_doc_lines_covered(
        "\
meta height 16
meta ascent 12
meta descent 4

// comment
name-parts $base = stem wide

glyph stem 2 2
@@@@
..@@

glyph wide 3 1
@@..@@

glyph alias = stem

glyph comp
ref stem
ref wide 1 0
anchor -join 0 0
anchor +join 2 0

glyph sticky-empty sticky advance 0

map A = stem
map B = wide
remap set1 : stem -> wide
feature liga for latn : set1
exclude-from-sample stem
",
    );
}
