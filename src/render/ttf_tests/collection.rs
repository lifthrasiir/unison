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
    assert_eq!(
        families,
        ["A", "B"],
        "declaration order is the output order"
    );
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

/// Blobs are padded going in, so only the last one could leave the file short
/// of a four-byte boundary — and the file length is what a checksum over the
/// whole file walks.
#[test]
fn the_file_ends_on_a_four_byte_boundary() {
    // The last blob is the last face's `name`, so the file length rides on the
    // strings in it — a fixed pair of faces would only catch this by luck.
    for n in 0..4 {
        let designer = "D".repeat(n);
        let src =
            format!("meta designer `{designer}`\nmeta family `A`\nglyph a 1 1\n@@\nmap A = a\n");
        for faces in [vec![font(&src)], vec![font(A), font(&src)]] {
            let bytes = build_collection(&faces).unwrap();
            assert_eq!(
                bytes.len() % 4,
                0,
                "{} face(s), designer {n} long",
                faces.len()
            );
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

/// The size argument for a collection: two faces that differ only in which
/// glyph a few characters reach must share the glyph store outright. That needs
/// the glyph order to be independent of any one face's cmap — with a per-face
/// order the tables differ byte-wise everywhere after the first difference, and
/// content dedup can do nothing.
#[test]
fn faces_differing_only_in_cmap_share_the_glyph_store() {
    let src = "\
slice narrow
slice wide
glyph n 1 1
@@
glyph w 2 1
@@@@
glyph other 1 1
..
map A = n
map B = other
map narrow : ° = n
map wide : ° = w
face narrow : narrow
face wide : wide
";
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let built = crate::render::ttf_builder::build_faces(&[&doc]).unwrap();
    assert_eq!(built.len(), 2);
    let bytes =
        build_collection(&built.iter().map(|(_, b)| b.clone()).collect::<Vec<_>>()).unwrap();

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
    for tag in [b"glyf", b"loca", b"hmtx", b"maxp"] {
        assert_eq!(
            offset_of(0, tag),
            offset_of(1, tag),
            "{}",
            std::str::from_utf8(tag).unwrap()
        );
    }
    assert_ne!(
        offset_of(0, b"cmap"),
        offset_of(1, b"cmap"),
        "the cmaps differ"
    );

    // And each face still maps ° to its own glyph — sharing the store must not
    // have merged the two typefaces.
    let advance_of = |i: u32, ch: char| -> u16 {
        let f = ttc.get(i).unwrap();
        let gid = f.cmap().unwrap().map_codepoint(ch).unwrap();
        f.hmtx().unwrap().advance(gid).unwrap()
    };
    assert_eq!(advance_of(0, '°'), 64, "narrow");
    assert_eq!(advance_of(1, '°'), 128, "wide");
    assert_eq!(advance_of(0, 'A'), 64);
}

/// The single-face path and the multi-face path are separate code, and a source
/// with one face must get the same font from either — otherwise `build` and the
/// editor's preview would slowly drift apart.
#[test]
fn one_face_builds_the_same_font_either_way() {
    let doc = document_io::parse_document_from_str(A, "test.unf".into()).unwrap();
    let single = build_font_from_documents(&[&doc]).unwrap();
    let faces = crate::render::ttf_builder::build_faces(&[&doc]).unwrap();
    assert_eq!(faces.len(), 1);
    assert_eq!(
        faces[0].1, single,
        "the two build paths must agree byte for byte"
    );
}

/// The editor builds one face at a time — never a collection — so which face
/// its preview shows has to be selectable; a two-face source is otherwise stuck
/// on the first one forever. An unknown id falls back to the primary rather
/// than failing the build, since the selection outlives an edit that removes
/// the face it named.
#[test]
#[cfg(feature = "editor")]
fn the_editor_font_pair_follows_the_requested_face() {
    let src = "\
slice narrow
slice wide
glyph n 1 1
@@
glyph w 2 1
@@@@
map narrow : ° = n
map wide : ° = w
face narrow : narrow
face wide : wide
";
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let cache = crate::render::new_contour_cache();
    let advance_of = |face: Option<&str>, ch: char| -> u16 {
        let built = crate::render::build_font_pair_cached_for(
            &[&doc],
            &cache,
            face,
            &crate::cancel::CancelToken::never(),
        )
        .unwrap();
        let f = read_fonts::FontRef::new(&built.vector).unwrap();
        let gid = f.cmap().unwrap().map_codepoint(ch).unwrap();
        f.hmtx().unwrap().advance(gid).unwrap()
    };
    assert_eq!(advance_of(None, '°'), 64, "the primary face");
    assert_eq!(advance_of(Some("narrow"), '°'), 64);
    assert_eq!(advance_of(Some("wide"), '°'), 128);
    assert_eq!(advance_of(Some("gone"), '°'), 64, "unknown id falls back");
}
