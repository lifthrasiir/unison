//! Tests for [`crate::render::sample`].
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items.

use super::*;
use crate::document_io;

fn parse(input: &str) -> Document {
    document_io::parse_document_from_str(input, "test.unf".into()).unwrap()
}

#[test]
fn subdivision_flag_is_a_tag_sequence() {
    assert_eq!(
        subdivision_flag_seq("gbsct").as_deref(),
        Some("\u{1f3f4}\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f}")
    );
    // UTS #51 restricts tag_spec to [0-9a-z], 1..=6 characters.
    assert_eq!(subdivision_flag_seq("us-tx"), None);
    assert_eq!(subdivision_flag_seq("GBSCT"), None);
    assert_eq!(subdivision_flag_seq(""), None);
    assert_eq!(subdivision_flag_seq("abcdefg"), None);
}

#[test]
fn sample_includes_map_decomposed_composite_glyph() {
    // `map <precomposed char>` (DocumentItem::MapDecomposed) synthesizes a
    // composite glyph via NFD decomposition; it used to be silently
    // skipped when collecting sample data.
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph a-lower 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 2 0

glyph dia-above mark 3 2
@@@@@@
@@@@@@
anchor -above 1 1

map a = a-lower
map \u{0308} = dia-above
map generate ä
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    assert!(
        data.cmap.contains_key(&('ä' as u32)),
        "'a with combining diaeresis' should be mapped in cmap"
    );
}

/// A character whose glyph never resolved claims no code point in the font,
/// and so none here either: a page listing it would show a notdef box the
/// font never promised.
#[test]
fn a_glyph_that_cannot_resolve_maps_nothing() {
    let d = parse(
        "\
meta height 16
meta ascent 12
meta descent 4

glyph a-lower 2 2
@@@@
@@@@

glyph broken
ref nothing-defines-this

map A = a-lower
map B = broken
",
    );
    let data = collect_sample_data(&[&d]).expect("sample data should build");
    assert!(data.cmap.contains_key(&('A' as u32)));
    assert!(
        !data.cmap.contains_key(&('B' as u32)),
        "the composite never resolved, so it maps nothing"
    );
}

/// The translations offered are the ones the font can draw *and* that add a
/// code point no earlier one drew — five hundred translations of one
/// paragraph are mostly the same letters over again.
#[test]
fn the_udhr_selection_drops_what_the_font_cannot_draw() {
    let dir = std::env::temp_dir().join(format!("uniform-udhr-sel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("udhr-article1.json"),
        r#"[{"lang":"eng","name":"English","text":"AB"},
            {"lang":"dup","name":"Duplicate","text":"BA"},
            {"lang":"non","name":"Unmapped","text":"AZ"}]"#,
    )
    .unwrap();

    let cmap: BTreeMap<u32, String> =
        [('A' as u32, "a".to_string()), ('B' as u32, "b".to_string())]
            .into_iter()
            .collect();
    let selected = udhr_selection(&dir, &cmap).unwrap();
    let langs: Vec<&str> = selected.iter().map(|e| e.lang.as_str()).collect();
    assert_eq!(
        langs,
        ["eng"],
        "`dup` draws nothing new and `non` holds a character the font lacks"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The text a `subdivision-flags` sample stands for: a line per region, and
/// the tag sequence of each of its subdivisions run together. A code that
/// cannot form a well-formed sequence is dropped, and a region left with
/// nothing is no line at all.
#[test]
fn the_subdivision_flags_text_is_a_line_per_region() {
    let dir = std::env::temp_dir().join(format!("uniform-subdiv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cldr-subdivisions-1.2.3.json");
    std::fs::write(
        &path,
        r#"{"subdivisions":{"GB":["gbsct","gbwls"],"US":["us-tx"]}}"#,
    )
    .unwrap();

    assert_eq!(subdivisions_path(&dir).as_deref(), Some(path.as_path()));
    assert_eq!(
        subdivision_flags_text(&path).unwrap(),
        format!(
            "GB {}{}",
            subdivision_flag_seq("gbsct").unwrap(),
            subdivision_flag_seq("gbwls").unwrap()
        ),
        "`us-tx` is no tag sequence, so `US` is left with no line"
    );

    std::fs::remove_dir_all(&dir).ok();
}
