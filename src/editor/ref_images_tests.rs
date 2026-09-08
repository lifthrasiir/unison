//! What a strip row is derived from: the parts of a line that can name a code
//! point, and the file the strip is read from.

use super::*;

fn cps(text: &str) -> Vec<u32> {
    candidate_codepoints(text)
        .into_iter()
        .map(|(_, _, cp)| cp)
        .collect()
}

#[test]
fn a_candidate_is_a_whole_part_of_four_to_six_hex_digits() {
    assert_eq!(cps("glyph han-4e00"), vec![0x4e00]);
    assert_eq!(cps("glyph han-2a6d6:8x16"), vec![0x2a6d6]);
    // Three digits and seven are both out, and so is a part that is only
    // partly hexadecimal.
    assert_eq!(cps("glyph han-4e0"), Vec::<u32>::new());
    assert_eq!(cps("glyph han-4e000001"), Vec::<u32>::new());
    assert_eq!(cps("glyph zebra4e00"), Vec::<u32>::new());
    // Uppercase is outside the delimiter set, so `U+4E00` is several parts,
    // none of them a candidate. That is what keeps a `map` line's own
    // spelling of a code point from being read as one.
    assert_eq!(cps("map U+4E00 han-4e00"), vec![0x4e00]);
}

#[test]
fn a_candidate_has_to_be_a_code_point() {
    // Above the Unicode range, and so nothing a chart could hold.
    assert_eq!(cps("glyph a-ffffff"), Vec::<u32>::new());
    // A surrogate is not a `char`.
    assert_eq!(cps("glyph a-d800"), Vec::<u32>::new());
}

#[test]
fn only_the_name_a_glyph_line_declares_carries_a_strip() {
    let found = |line: &str| {
        let c = candidate_codepoints(line);
        in_glyph_definitions(line, &c)
            .into_iter()
            .map(|(_, _, cp)| cp)
            .collect::<Vec<_>>()
    };
    assert_eq!(found("glyph han-4e00"), vec![0x4e00]);
    // The alias form declares its left name and refers to its right one.
    assert_eq!(found("glyph han-4e00 = han-4e01"), vec![0x4e00]);
    // A component line names a glyph but declares none.
    assert_eq!(found("  ref han-4e00"), Vec::<u32>::new());
}

#[test]
fn a_strip_is_shown_once_per_code_point_and_only_over_a_glyph_line() {
    let store = RefImages::for_test([0x4e00, 0x4e01].into_iter().collect());
    let lines: Vec<DocLine> = [
        "// han-4e00 is drawn from the chart",
        "  ref han-4e00",
        "glyph han-4e00",
        "glyph han-4e00:8x16",
        "map U+4E01 han-4e01",
        "glyph han-4e01",
        // No strip in the index for this one.
        "glyph han-4e02",
    ]
    .iter()
    .map(|s| DocLine::Text(s.to_string()))
    .collect();
    assert_eq!(store.rows_for(&lines), vec![(2, 0x4e00), (5, 0x4e01)]);
}

#[test]
fn nothing_is_shown_until_the_directory_has_been_read() {
    let store = RefImages {
        root: Arc::new(PathBuf::from("/nonexistent")),
        inner: Arc::new(Mutex::new(Inner {
            index: None,
            generation: 0,
            entries: HashMap::default(),
        })),
        requests: None,
    };
    let lines = vec![DocLine::Text("glyph han-4e00".to_string())];
    assert!(store.rows_for(&lines).is_empty());
}

/// The path rule is shared with `scripts/extract_ref_charts.py`; if one moves
/// the other has to.
#[test]
fn a_strip_lives_under_its_code_points_prefix() {
    let root = Path::new("/font/../data/ref");
    assert_eq!(image_path(root, 0x4e00), root.join("4").join("4e00.png"));
    assert_eq!(image_path(root, 0x2a6d6), root.join("2a").join("2a6d6.png"));
    // Four digits is the shortest name, so the prefix is one character even
    // when the code point is not.
    assert_eq!(image_path(root, 0x41), root.join("0").join("0041.png"));
    assert_eq!(image_path(root, 0x10000), root.join("10").join("10000.png"));
}
