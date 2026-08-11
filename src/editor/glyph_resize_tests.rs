//! What a resize does to the file: the glyph's own block, and the `ref`s
//! elsewhere that have to move with it.

use super::*;
use crate::document_io::{derive_document, encode_grid_row, parse_doclines};

/// A document set of one file, resolved the way the editor resolves it.
struct Fixture {
    doc: Document,
    lines: Vec<DocLine>,
    named_glyphs: HashMap<String, ResolvedGlyph>,
    alt_index: AlternativesIndex,
    name_parts: NamePartsMap,
}

impl Fixture {
    fn new(source: &str) -> Self {
        let lines = parse_doclines(source);
        let (doc, _) = derive_document(&lines, "test.unf".into()).expect("derive");
        let mut f = Fixture {
            doc,
            lines,
            named_glyphs: HashMap::new(),
            alt_index: AlternativesIndex::default(),
            name_parts: NamePartsMap::new(),
        };
        f.resolve();
        f
    }

    fn resolve(&mut self) {
        let docs: Vec<&Document> = vec![&self.doc];
        self.name_parts = crate::document::collect_name_parts(&docs);
        let (named, alts) =
            crate::ref_composite::resolve_named_glyphs_with_parts(&docs, &self.name_parts);
        self.named_glyphs = named;
        self.alt_index = alts;
    }

    fn env(&self) -> ResolveEnv<'_> {
        ResolveEnv {
            named_glyphs: &self.named_glyphs,
            name_parts: &self.name_parts,
            alt_index: &self.alt_index,
        }
    }

    fn item_of(&self, glyph: &str) -> usize {
        self.doc
            .items
            .iter()
            .position(|i| matches!(i, DocumentItem::Glyph { name, .. } if name.0 == glyph))
            .expect("glyph")
    }

    /// The whole resize, exactly as the host performs it.
    fn resize(&mut self, glyph: &str, deltas: ResizeDeltas) {
        let docs: Vec<&Document> = vec![&self.doc];
        let names = target_names(&docs, &self.name_parts, glyph);
        let define_item = Some(self.item_of(glyph));
        let plan = plan_document_resize(
            &self.doc,
            &self.lines,
            &names,
            deltas,
            define_item,
            self.env(),
        );
        apply_plan(&mut self.lines, plan);
        let (doc, _) = derive_document(&self.lines, "test.unf".into()).expect("re-derive");
        self.doc = doc;
        self.resolve();
    }

    fn rendered(&self) -> Vec<String> {
        let mut out = Vec::new();
        for line in &self.lines {
            match line {
                DocLine::Text(t) => out.push(t.clone()),
                DocLine::Grid(g) => {
                    for row in 0..g.height {
                        out.push(encode_grid_row(g, row));
                    }
                }
            }
        }
        out
    }
}

const DOT: &str = "\
glyph dot 2 2
@@..
..@@
";

#[test]
fn growing_left_and_down_moves_the_ink_and_the_header() {
    let mut f = Fixture::new(DOT);
    f.resize(
        "dot",
        ResizeDeltas {
            left: 2,
            bottom: 1,
            ..Default::default()
        },
    );
    assert_eq!(
        f.rendered(),
        vec!["glyph dot 4 3", "....@@..", "......@@", "........",],
    );
}

#[test]
fn shrinking_crops_what_falls_outside() {
    let mut f = Fixture::new(DOT);
    f.resize(
        "dot",
        ResizeDeltas {
            left: -1,
            ..Default::default()
        },
    );
    assert_eq!(f.rendered(), vec!["glyph dot 1 2", "..", "@@"]);
}

#[test]
fn the_glyph_never_shrinks_past_its_last_pixel() {
    let block = parse_doclines(DOT);
    let (doc, _) = derive_document(&block, "t.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        unreachable!()
    };
    // Two columns wide: taking three off both edges leaves nothing to write.
    assert!(
        resize_block(
            &block,
            body,
            ResizeDeltas {
                left: -2,
                right: -1,
                ..Default::default()
            },
            &[],
        )
        .is_none()
    );
}

/// The example from the module docs: `foo` grows two columns to the left and
/// one row down, and a `ref foo 1 2` elsewhere becomes `ref foo -1 2`. Only
/// the left edge enters the offset — growing downwards adds cells past the
/// ink and moves nothing.
#[test]
fn a_ref_to_the_glyph_takes_the_left_growth_as_a_bearing() {
    let mut f = Fixture::new(
        "\
glyph foo 2 2
@@..
..@@

glyph bar 8 16
ref foo 1 2
",
    );
    f.resize(
        "foo",
        ResizeDeltas {
            left: 2,
            bottom: 1,
            ..Default::default()
        },
    );
    assert!(f.rendered().contains(&"ref foo -1 2".to_string()));
}

/// A resize that only moved the right or bottom edge leaves every reference
/// to the glyph exactly as written — including one with no offset, which must
/// not be materialized into `0 0` for nothing.
#[test]
fn growing_rightwards_touches_no_reference() {
    let mut f = Fixture::new(
        "\
glyph foo 2 2
@@..
..@@

glyph bar 8 16
ref foo
ref foo 3 0
",
    );
    f.resize(
        "foo",
        ResizeDeltas {
            right: 4,
            bottom: 2,
            ..Default::default()
        },
    );
    let rendered = f.rendered();
    assert!(rendered.contains(&"ref foo".to_string()));
    assert!(rendered.contains(&"ref foo 3 0".to_string()));
}

/// An offset-less ref that no anchor placed is a (0, 0) fallback, so the
/// compensation has to be written out for it.
#[test]
fn an_unplaced_offsetless_ref_gets_the_offset_spelled_out() {
    let mut f = Fixture::new(
        "\
glyph foo 2 2
@@..
..@@

glyph bar 8 16
ref foo
",
    );
    f.resize(
        "foo",
        ResizeDeltas {
            left: 1,
            top: 1,
            ..Default::default()
        },
    );
    assert!(f.rendered().contains(&"ref foo -1 -1".to_string()));
}

/// An anchor-placed ref follows its target's anchors, which moved with the
/// resize, so its line is left alone. Writing an offset onto it would freeze
/// it where it happened to be.
#[test]
fn an_anchor_placed_ref_is_left_alone() {
    let mut f = Fixture::new(
        "\
glyph base 4 4
@@@@....
........
........
........
anchor +above 0 0

glyph mark 2 2
@@..
..@@
anchor -above 0 0

glyph both 8 16
ref base 0 0
ref mark
",
    );
    let before = f.rendered();
    assert!(before.contains(&"ref mark".to_string()));
    f.resize(
        "mark",
        ResizeDeltas {
            left: 1,
            top: 1,
            ..Default::default()
        },
    );
    let after = f.rendered();
    assert!(
        after.contains(&"ref mark".to_string()),
        "the anchored ref must keep its auto placement: {after:?}"
    );
    // The mark's own anchor moved with its ink, which is what keeps the
    // attachment pointing at the same place.
    assert!(
        after.contains(&"anchor -above 1 1".to_string()),
        "{after:?}"
    );
}

/// A `ref` reaching the glyph through an alias names the same glyph id, so it
/// is compensated the same way.
#[test]
fn a_ref_through_an_alias_moves_too() {
    let mut f = Fixture::new(
        "\
glyph foo 2 2
@@..
..@@

glyph foo-alt = foo

glyph bar 8 16
ref foo-alt 1 0
",
    );
    f.resize(
        "foo",
        ResizeDeltas {
            left: 1,
            ..Default::default()
        },
    );
    assert!(f.rendered().contains(&"ref foo-alt 0 0".to_string()));
}

/// Everything is stated in logical pixels: the header counts them directly,
/// while the grid, the `anchor` lines and a referring glyph's `ref` offsets
/// are all in that glyph's own subcells.
#[test]
fn scale_is_counted_on_both_sides() {
    let mut f = Fixture::new(
        "\
glyph foo 2 2 scale 2
@@@@....
@@@@....
....@@@@
....@@@@
anchor +above 0 0

glyph bar 4 4 scale 2
ref foo 2 0
",
    );
    f.resize(
        "foo",
        ResizeDeltas {
            left: 1,
            ..Default::default()
        },
    );
    let rendered = f.rendered();
    // Three logical columns, i.e. six subcells, with the ink two subcells in.
    assert_eq!(rendered[0], "glyph foo 3 2 scale 2");
    assert_eq!(rendered[1], "....@@@@....");
    // The glyph's own anchor moved by the same two subcells...
    assert!(
        rendered.contains(&"anchor +above 2 0".to_string()),
        "{rendered:?}"
    );
    // ...and `bar`, itself at scale 2, moves its reference by two of *its*
    // subcells, which is the same one logical pixel.
    assert!(
        rendered.contains(&"ref foo 0 0".to_string()),
        "{rendered:?}"
    );
}

/// A resize is one edit per file however many lines it moved, so the whole of
/// it undoes at once.
#[test]
fn a_resize_is_a_single_undo_entry() {
    let mut f = Fixture::new(
        "\
glyph foo 2 2
@@..
..@@
anchor +above 0 0

glyph bar 8 16
ref foo 1 0
",
    );
    let before = f.rendered();
    let docs: Vec<&Document> = vec![&f.doc];
    let names = target_names(&docs, &f.name_parts, "foo");
    let define_item = Some(f.item_of("foo"));
    let plan = plan_document_resize(
        &f.doc,
        &f.lines,
        &names,
        ResizeDeltas {
            left: 1,
            ..Default::default()
        },
        define_item,
        f.env(),
    );
    let ops = apply_plan(&mut f.lines, plan);
    assert!(ops.len() > 1, "the glyph's block and the ref both moved");

    let mut undo = crate::editor::undo::UndoStack::new();
    let caret = crate::editor::caret::Caret::zero();
    undo.push_compound(ops, caret, caret);
    assert_ne!(f.rendered(), before);
    undo.undo(&mut f.lines).expect("one entry");
    assert_eq!(f.rendered(), before, "one undo takes the whole resize back");
    assert!(!undo.can_undo(), "and there is nothing else to take back");
}
