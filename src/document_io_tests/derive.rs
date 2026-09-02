//! `derive_document` equivalence: deriving a document from its lines has to
//! agree with parsing the same text.

use super::*;

fn assert_derive_equivalent(input: &str) {
    let old_doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let lines = parse_doclines(input);
    let (new_doc, starts) = derive_document(&lines, "test.unf".into()).unwrap();

    assert_eq!(
        old_doc.items.len(),
        new_doc.items.len(),
        "item count mismatch"
    );
    assert_eq!(starts.len(), new_doc.items.len());

    for (idx, (old_item, new_item)) in old_doc.items.iter().zip(new_doc.items.iter()).enumerate() {
        match (old_item, new_item) {
            (DocumentItem::BlankLine, DocumentItem::BlankLine) => {}
            (DocumentItem::Comment(a), DocumentItem::Comment(b)) => {
                assert_eq!(a, b, "comment mismatch at item {idx}");
            }
            (DocumentItem::Meta(a), DocumentItem::Meta(b)) => {
                assert_eq!(a, b, "meta mismatch at item {idx}");
            }
            (DocumentItem::Audit(a), DocumentItem::Audit(b)) => {
                assert_eq!(a, b, "audit mismatch at item {idx}");
            }
            (DocumentItem::Directive(a), DocumentItem::Directive(b)) => {
                assert_eq!(a, b, "directive mismatch at item {idx}");
            }
            (
                DocumentItem::NameParts {
                    name: n1,
                    values: v1,
                    ..
                },
                DocumentItem::NameParts {
                    name: n2,
                    values: v2,
                    ..
                },
            ) => {
                assert_eq!(n1, n2, "name-parts name mismatch at item {idx}");
                assert_eq!(v1, v2, "name-parts values mismatch at item {idx}");
            }
            (
                DocumentItem::Remap {
                    feature: f1,
                    lookbehind: lb1,
                    source: s1,
                    target: t1,
                    lookahead: la1,
                    ..
                },
                DocumentItem::Remap {
                    feature: f2,
                    lookbehind: lb2,
                    source: s2,
                    target: t2,
                    lookahead: la2,
                    ..
                },
            ) => {
                assert_eq!(f1, f2, "remap feature mismatch at item {idx}");
                assert_eq!(lb1, lb2, "remap lookbehind mismatch at item {idx}");
                assert_eq!(s1, s2, "remap source mismatch at item {idx}");
                assert_eq!(t1, t2, "remap target mismatch at item {idx}");
                assert_eq!(la1, la2, "remap lookahead mismatch at item {idx}");
            }
            (
                DocumentItem::Feature {
                    name: n1,
                    scripts: s1,
                    remap_group: r1,
                    ..
                },
                DocumentItem::Feature {
                    name: n2,
                    scripts: s2,
                    remap_group: r2,
                    ..
                },
            ) => {
                assert_eq!(n1, n2, "feature name mismatch at item {idx}");
                assert_eq!(s1, s2, "feature scripts mismatch at item {idx}");
                assert_eq!(r1, r2, "feature remap_group mismatch at item {idx}");
            }
            (
                DocumentItem::Glyph { name: n1, body: b1 },
                DocumentItem::Glyph { name: n2, body: b2 },
            ) => {
                assert_eq!(n1.display(), n2.display(), "name mismatch at item {idx}");
                assert_eq!(b1.pixels, b2.pixels, "pixels mismatch at item {idx}");
                assert_eq!(
                    b1.refs.len(),
                    b2.refs.len(),
                    "ref count mismatch at item {idx}"
                );
                for (ri, (r1, r2)) in b1.refs.iter().zip(b2.refs.iter()).enumerate() {
                    assert_eq!(r1.name, r2.name, "ref name mismatch at item {idx} ref {ri}");
                    assert_eq!(
                        r1.offset, r2.offset,
                        "ref offset mismatch at item {idx} ref {ri}"
                    );
                    assert_eq!(
                        r1.negated, r2.negated,
                        "ref negation mismatch at item {idx} ref {ri}"
                    );
                }
            }
            (
                DocumentItem::GlyphAlias {
                    name: n1,
                    target: t1,
                    ..
                },
                DocumentItem::GlyphAlias {
                    name: n2,
                    target: t2,
                    ..
                },
            ) => {
                assert_eq!(n1.display(), n2.display(), "name mismatch at item {idx}");
                assert_eq!(t1, t2, "alias target mismatch at item {idx}");
            }
            _ => panic!(
                "item kind mismatch at item {idx}: {:?} vs {:?}",
                std::mem::discriminant(old_item),
                std::mem::discriminant(new_item),
            ),
        }
    }
}

#[test]
fn derive_equivalent_simple() {
    assert_derive_equivalent(
        "\
// test comment
meta height 16
meta ascent 14
meta descent 2

glyph test-glyph 4 3
....@@..
..@@@@..
@@@@@@@@

glyph uni0041
ref test-glyph 2 0

assume unused test-glyph
",
    );
}

#[test]
fn derive_equivalent_alias() {
    assert_derive_equivalent("glyph uni0041 = test-glyph\n");
}

#[test]
fn derive_equivalent_mixed_refs() {
    assert_derive_equivalent(
        "\
glyph mixed 2 2
..@@
@@..
ref other 1 1
",
    );
}

#[test]
fn derive_item_line_starts() {
    let input = "\
// comment
glyph foo 2 1
..@@
ref bar 0 0
";
    let lines = parse_doclines(input);
    let (doc, starts) = derive_document(&lines, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 2);
    assert_eq!(starts, vec![0, 1]); // comment at line 0, glyph header at line 1
}

// -----------------------------------------------------------------------
// Intermediate editing states (derive_document tolerance)
// -----------------------------------------------------------------------
