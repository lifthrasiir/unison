//! Tests for [`super::super::collection`]: the TTC writer.

use read_fonts::TableProvider;

use super::*;
use crate::document_io;
use crate::render::ttf_builder::build_font_from_documents;

fn font(src: &str) -> Vec<u8> {
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    build_font_from_documents(&[&doc]).expect("expected a font")
}

const A: &str = "meta family `A`\nglyph a 1 1\n@@\nmap A = a\n";
const B: &str = "meta family `B`\nglyph a 1 1\n@@\nmap A = a\n";

/// The whole point of a collection is that `read-fonts` — and therefore every
/// real consumer — sees several fonts in one file, in the order given.
#[test]
fn a_collection_reads_back_as_its_faces_in_order() {
    let bytes = build_collection(&[font(A), font(B)]).unwrap();
    let read_fonts::FileRef::Collection(ttc) = read_fonts::FileRef::new(&bytes).unwrap() else {
        panic!("expected a collection");
    };
    assert_eq!(ttc.len(), 2);
    let families: Vec<String> = (0..2)
        .map(|i| {
            let f = ttc.get(i).unwrap();
            let name = f.name().unwrap();
            name.name_record()
                .iter()
                .find(|r| r.name_id.get().to_u16() == 1)
                .map(|r| r.string(name.string_data()).unwrap().chars().collect())
                .unwrap()
        })
        .collect();
    assert_eq!(families, ["A", "B"], "declaration order is the output order");
}

/// Identical tables are stored once and pointed at twice. That is what sharing
/// *is* in a TTC — there is no other mechanism.
#[test]
fn identical_tables_are_stored_once() {
    let bytes = build_collection(&[font(A), font(B)]).unwrap();
    let read_fonts::FileRef::Collection(ttc) = read_fonts::FileRef::new(&bytes).unwrap() else {
        panic!("expected a collection");
    };
    let offset_of = |i: u32, tag: &[u8; 4]| -> u32 {
        ttc.get(i)
            .unwrap()
            .table_directory()
            .table_records()
            .iter()
            .find(|r| r.tag() == read_fonts::types::Tag::new(tag))
            .unwrap()
            .offset()
    };
    // The two sources differ only in family name, so everything but `name`
    // is byte-identical and must land on one offset.
    assert_eq!(offset_of(0, b"glyf"), offset_of(1, b"glyf"), "glyf");
    assert_eq!(offset_of(0, b"head"), offset_of(1, b"head"), "head");
    assert_ne!(offset_of(0, b"name"), offset_of(1, b"name"), "name differs");

    // And the file is meaningfully smaller than the two fonts concatenated.
    let separate = font(A).len() + font(B).len();
    assert!(
        bytes.len() < separate,
        "collection {} should be smaller than {separate}",
        bytes.len(),
    );
}

/// `head.checkSumAdjustment` covers "the whole font file", which a face inside
/// a collection does not have. Left non-zero it would also differ between two
/// otherwise identical `head` tables and block sharing.
#[test]
fn head_checksum_adjustment_is_zeroed() {
    let bytes = build_collection(&[font(A), font(B)]).unwrap();
    let read_fonts::FileRef::Collection(ttc) = read_fonts::FileRef::new(&bytes).unwrap() else {
        panic!("expected a collection");
    };
    for i in 0..2 {
        let f = ttc.get(i).unwrap();
        let head = f.table_data(read_fonts::types::Tag::new(b"head")).unwrap();
        let raw = head.as_bytes();
        assert_eq!(&raw[8..12], &[0, 0, 0, 0], "face {i}");
    }
}

/// Every table blob starts on a four-byte boundary, as the format requires.
#[test]
fn table_offsets_are_four_byte_aligned() {
    let bytes = build_collection(&[font(A), font(B)]).unwrap();
    let read_fonts::FileRef::Collection(ttc) = read_fonts::FileRef::new(&bytes).unwrap() else {
        panic!("expected a collection");
    };
    for i in 0..ttc.len() {
        for rec in ttc.get(i).unwrap().table_directory().table_records() {
            assert_eq!(rec.offset() % 4, 0, "{} in face {i}", rec.tag());
        }
    }
}

#[test]
fn a_single_face_collection_is_still_a_collection() {
    let bytes = build_collection(&[font(A)]).unwrap();
    let read_fonts::FileRef::Collection(ttc) = read_fonts::FileRef::new(&bytes).unwrap() else {
        panic!("expected a collection");
    };
    assert_eq!(ttc.len(), 1);
    assert!(build_collection(&[]).is_err());
}
