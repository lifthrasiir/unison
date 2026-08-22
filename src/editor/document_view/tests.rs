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

    assert!(state.apply_edit_action(
        EditAction::Delete,
        &doc,
        &mut lines,
        &egui::Context::default(),
    ));
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

/// Flattening a `ref` writes the target's pixels where the *ref* drew them,
/// and a ref offset names the target's declared box corner — so a target that
/// declares one has that box taken out of the offset first. Getting it wrong
/// lands the pixels somewhere the composite never drew.
#[test]
fn inline_flatten_places_the_pixels_by_the_targets_box() {
    let flattened = |flags: &str, offset: &str| {
        let src = format!(
            "glyph stem 2 2 {flags}\n\
             @@..\n\
             ..@@\n\
             \n\
             glyph comp 4 2\n\
             ........\n\
             ........\n\
             ref stem {offset}\n"
        );
        let mut lines = parse_doclines(&src);
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
        lines
            .iter()
            .filter_map(|l| match l {
                DocLine::Grid(g) => Some(g.clone()),
                _ => None,
            })
            .next_back()
            .expect("the composite kept its grid")
    };

    // The same drawing in the same place, spelled two ways.
    let plain = flattened("", "1 0");
    let boxed = flattened("origin 1 0", "2 0");
    assert_eq!(
        (0..plain.height)
            .flat_map(|r| (0..plain.width).map(move |c| (r, c)))
            .filter(|&(r, c)| plain.get(r, c).is_bitmap_filled())
            .collect::<Vec<_>>(),
        (0..boxed.height)
            .flat_map(|r| (0..boxed.width).map(move |c| (r, c)))
            .filter(|&(r, c)| boxed.get(r, c).is_bitmap_filled())
            .collect::<Vec<_>>(),
        "the box a ref names must not move the pixels it flattens"
    );
    assert!(
        plain.get(0, 1).is_bitmap_filled(),
        "and they landed where the ref drew them"
    );
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

/// Everything "Inline once" needs from a source: the parsed document, the
/// resolution the editor draws from, and the index of a glyph by name.
fn inline_fixture(source: &str) -> (Vec<DocLine>, Document, InlineEnv) {
    let lines = parse_doclines(source);
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    let name_parts = crate::document::collect_name_parts(&[&doc]);
    let (named, alt_index) = ref_composite::resolve_named_glyphs_with_parts(&[&doc], &name_parts);
    (
        lines,
        doc,
        InlineEnv {
            named,
            name_parts,
            alt_index,
        },
    )
}

struct InlineEnv {
    named: HashMap<String, ref_composite::ResolvedGlyph>,
    name_parts: crate::document::NamePartsMap,
    alt_index: ref_composite::AlternativesIndex,
}

#[track_caller]
fn glyph_idx(doc: &Document, want: &str) -> usize {
    doc.items
        .iter()
        .position(|i| matches!(i, DocumentItem::Glyph { name, .. } if name.display() == want))
        .unwrap_or_else(|| panic!("no glyph named {want}"))
}

fn text_lines(lines: &[DocLine]) -> Vec<&str> {
    lines
        .iter()
        .filter_map(|l| match l {
            DocLine::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

/// The composite of `edit_idx`, as the view computes it — "Inline once" reads
/// the offsets it derived for anchor-positioned refs from there.
fn composite_of(
    doc: &Document,
    env: &InlineEnv,
    edit_idx: usize,
) -> Option<ref_composite::GlyphComposite> {
    let DocumentItem::Glyph { body, .. } = &doc.items[edit_idx] else {
        return None;
    };
    ref_composite::compute_composite(
        body,
        &env.named,
        &env.name_parts,
        &env.alt_index,
        &Default::default(),
    )
}

/// "Inline once" expands a `ref` by one level: the target's own refs take its
/// place, rebased onto where the ref sat. What it refers to stays referred to.
#[test]
fn inline_once_replaces_a_ref_with_the_targets_own_refs() {
    let (mut lines, doc, env) = inline_fixture(
        "glyph stem 2 2\n\
         @@..\n\
         ..@@\n\
         \n\
         glyph mid\n\
         ref stem 1 0\n\
         ref stem 0 2\n\
         \n\
         glyph top\n\
         ref mid 2 1\n",
    );
    let top = glyph_idx(&doc, "top");
    let comp = composite_of(&doc, &env, top);
    let mut state = EditorState::new();

    assert!(changes::inline_ref_once(
        &mut lines,
        &doc,
        &mut state,
        top,
        0,
        comp.as_ref(),
        &env.named,
        &env.name_parts,
    ));

    let texts = text_lines(&lines);
    assert!(
        texts.contains(&"ref stem 3 1") && texts.contains(&"ref stem 2 3"),
        "the target's refs should have been rebased onto the ref's offset: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.starts_with("ref mid")),
        "the expanded ref line survived: {texts:?}"
    );
    let top_start = lines
        .iter()
        .position(|l| matches!(l, DocLine::Text(t) if t == "glyph top"))
        .unwrap();
    assert!(
        !lines[top_start..]
            .iter()
            .any(|l| matches!(l, DocLine::Grid(_))),
        "a target that draws no pixels of its own must not grow a grid: {lines:?}"
    );
}

/// The target states its refs in *its* subcells: inlining a `scale 1` glyph
/// into a `scale 2` one doubles every offset, and the other way round halves
/// it onto the coarser lattice.
#[test]
fn inline_once_rebases_offsets_across_a_scale_difference() {
    let (lines, doc, env) = inline_fixture(
        "glyph stem 1 1\n\
         @@\n\
         \n\
         glyph coarse\n\
         ref stem 1 0\n\
         ref stem 0 2\n\
         \n\
         glyph fine scale 2\n\
         ref stem 1 0\n\
         ref stem 0 2\n\
         \n\
         glyph up 4 2 scale 2\n\
         ................\n\
         ................\n\
         ................\n\
         ................\n\
         ref coarse 3 1\n\
         \n\
         glyph down 4 2\n\
         ........\n\
         ........\n\
         ref fine 3 1\n",
    );

    for (parent, want) in [
        ("up", ["ref stem 5 1", "ref stem 3 5"]),
        ("down", ["ref stem 4 1", "ref stem 3 2"]),
    ] {
        let idx = glyph_idx(&doc, parent);
        let comp = composite_of(&doc, &env, idx);
        let mut state = EditorState::new();
        let mut lines_of = lines.clone();
        assert!(changes::inline_ref_once(
            &mut lines_of,
            &doc,
            &mut state,
            idx,
            0,
            comp.as_ref(),
            &env.named,
            &env.name_parts,
        ));
        let texts = text_lines(&lines_of);
        for line in want {
            assert!(
                texts.contains(&line),
                "inlining into `{parent}` should have written `{line}`: {texts:?}"
            );
        }
    }
}

/// A target that draws pixels *and* refs gives up both: the pixels land in the
/// parent's grid exactly as flattening would put them, the refs stay refs.
#[test]
fn inline_once_merges_the_targets_own_pixels() {
    let (mut lines, doc, env) = inline_fixture(
        "glyph dot 2 2\n\
         @@..\n\
         ....\n\
         \n\
         glyph mid 4 2\n\
         ..@@....\n\
         ........\n\
         ref dot 2 0\n\
         \n\
         glyph top 8 2\n\
         ................\n\
         ................\n\
         ref mid 1 0\n",
    );
    let top = glyph_idx(&doc, "top");
    let comp = composite_of(&doc, &env, top);
    let mut state = EditorState::new();

    assert!(changes::inline_ref_once(
        &mut lines,
        &doc,
        &mut state,
        top,
        0,
        comp.as_ref(),
        &env.named,
        &env.name_parts,
    ));

    let grid = lines
        .iter()
        .filter_map(|l| match l {
            DocLine::Grid(g) => Some(g),
            _ => None,
        })
        .next_back()
        .expect("top keeps its grid");
    assert!(
        grid.get(0, 2).is_bitmap_filled(),
        "the target's own pixel should land at its column plus the ref's offset"
    );
    assert!(
        !grid.get(0, 3).is_bitmap_filled(),
        "the target's *ref* must not be flattened too"
    );
    let texts = text_lines(&lines);
    assert!(
        texts.contains(&"ref dot 3 0"),
        "the target's ref should survive, rebased: {texts:?}"
    );
}

/// A target with no refs has no declaration to expand — it is its pixels — so
/// "Inline once" is the flatten there.
#[test]
fn inline_once_of_a_pixel_only_target_flattens_it() {
    let (mut lines, doc, env) = inline_fixture(
        "glyph stem 2 2\n\
         @@..\n\
         ..@@\n\
         \n\
         glyph top 4 2\n\
         ........\n\
         ........\n\
         ref stem 1 0\n",
    );
    let top = glyph_idx(&doc, "top");
    let comp = composite_of(&doc, &env, top);
    let mut state = EditorState::new();

    assert!(changes::inline_ref_once(
        &mut lines,
        &doc,
        &mut state,
        top,
        0,
        comp.as_ref(),
        &env.named,
        &env.name_parts,
    ));

    let texts = text_lines(&lines);
    assert!(
        !texts.iter().any(|t| t.starts_with("ref ")),
        "the ref line should be gone: {texts:?}"
    );
    let grid = lines
        .iter()
        .filter_map(|l| match l {
            DocLine::Grid(g) => Some(g),
            _ => None,
        })
        .next_back()
        .unwrap();
    assert!(grid.get(0, 1).is_bitmap_filled() && grid.get(1, 2).is_bitmap_filled());
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
                | DocumentItem::Heading { .. }
                | DocumentItem::Directive(_)
                | DocumentItem::Face { .. }
                | DocumentItem::Slice { .. }
                | DocumentItem::Meta(_)
                | DocumentItem::Audit(_)
                | DocumentItem::Exists { .. }
                | DocumentItem::Map { .. }
                | DocumentItem::NameParts { .. }
                | DocumentItem::Remap { .. }
                | DocumentItem::RemapGroup { .. }
                | DocumentItem::Feature { .. }
                | DocumentItem::FeatureAnchor { .. }
                | DocumentItem::MapDecomposed { .. }
                | DocumentItem::Color { .. }
                | DocumentItem::PropBlock { .. }
                | DocumentItem::PropChar { .. }
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

glyph keep-empty keep advance 0

map A = stem
map B = wide
remap set1 : stem -> wide
feature liga for latn : set1
exclude-from-sample stem
",
    );
}

/// A heading drawn above the bitmap face's own size has to be painted with the
/// *vector* face, or the enlarged bitmap shows its pixels. At 1× the body text
/// is 16 px and a `#` line is 48 px, so the switch has to happen on the heading
/// even though the base font is the bitmap one.
#[test]
fn a_heading_larger_than_the_bitmap_size_draws_with_the_vector_face() {
    let bitmap = egui::FontFamily::Name("UniformBitmap".into());
    let vector = egui::FontFamily::Name("UniformVector".into());
    let base = egui::FontId::new(EDITOR_FONT_SIZE, bitmap.clone());

    let heading = |level: u8| {
        let font_size = heading_font_size(base.size, level);
        VisualLine {
            kind: VLineKind::Text(String::new()),
            doc_line: 0,
            color: egui::Color32::WHITE,
            comment_col: None,
            annotations: Vec::new(),
            error_spans: Vec::new(),
            col_offset: 0,
            heading: Some(HeadingLine {
                level,
                font_size,
                row_height: font_size,
            }),
        }
        .text_font(&base)
    };

    assert_eq!(heading(1).size, 48.0);
    assert_eq!(heading(1).family, vector, "`#` must use the vector face");
    assert_eq!(heading(2).size, 32.0);
    assert_eq!(heading(2).family, vector, "`##` must use the vector face");
    // `###` is body size, so it stays on the bitmap face like the rest.
    assert_eq!(heading(3).size, EDITOR_FONT_SIZE);
    assert_eq!(heading(3).family, bitmap);
}
