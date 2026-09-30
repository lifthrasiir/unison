use super::*;

fn triples(base: &str, sel: &str, glyph: &str) -> Vec<(u32, u32, String)> {
    expand_uvs_map_triples(base, sel, glyph).expect("expected a valid expansion")
}

#[test]
fn a_fixed_pair_expands_to_one_triple() {
    assert_eq!(
        triples("U+0030", "U+FE0F", "num-zero-emoji"),
        vec![(0x30, 0xFE0F, "num-zero-emoji".to_string())],
    );
}

/// The keycap shape: many bases, one selector. The glyph pattern runs in
/// lock-step with the half that varies.
///
/// The target arrives here already through `substitute_name_parts`, so an
/// inline numeric range is written out — `($0..2)` is that step's input,
/// not this one's.
#[test]
fn the_base_may_vary_against_a_fixed_selector() {
    assert_eq!(
        triples("U+0030..0032", "U+FE0F", "num-(0|1|2)-emoji"),
        vec![
            (0x30, 0xFE0F, "num-0-emoji".to_string()),
            (0x31, 0xFE0F, "num-1-emoji".to_string()),
            (0x32, 0xFE0F, "num-2-emoji".to_string()),
        ],
    );
}

/// The ideographic-variant shape: one base, many selectors. Both directions
/// occur in real sources, which is why neither position is privileged.
#[test]
fn the_selector_may_vary_against_a_fixed_base() {
    assert_eq!(
        triples("U+4E00", "U+E0100..E0102", "han-4e00-ivs(1|2|3)"),
        vec![
            (0x4E00, 0xE0100, "han-4e00-ivs1".to_string()),
            (0x4E00, 0xE0101, "han-4e00-ivs2".to_string()),
            (0x4E00, 0xE0102, "han-4e00-ivs3".to_string()),
        ],
    );
}

#[test]
fn a_pipe_list_varies_the_same_way_a_range_does() {
    assert_eq!(
        triples("#|*", "U+FE0F", "keycap-(hash|star)"),
        vec![
            (0x23, 0xFE0F, "keycap-hash".to_string()),
            (0x2A, 0xFE0F, "keycap-star".to_string()),
        ],
    );
}

#[test]
fn both_halves_varying_is_rejected() {
    assert_eq!(
        expand_uvs_map_triples("U+0030..0032", "U+FE0E..FE0F", "x"),
        Err(UvsExpandError::BothVary),
    );
}

/// The halves are not interchangeable: a selector has to be one, and a base
/// has to not be one. Both directions are checked so that a swapped line is
/// caught rather than silently building a sequence nothing will ever match.
#[test]
fn each_half_must_be_the_kind_it_stands_in_for() {
    assert_eq!(
        expand_uvs_map_triples("U+0030", "U+0031", "x"),
        Err(UvsExpandError::NotASelector {
            cp: 0x31,
            selector_half: true,
        }),
    );
    assert_eq!(
        expand_uvs_map_triples("U+FE0F", "U+FE0F", "x"),
        Err(UvsExpandError::NotASelector {
            cp: 0xFE0F,
            selector_half: false,
        }),
    );
}

/// A range that varies must not drag a selector into the base half either.
#[test]
fn a_range_covering_selectors_is_rejected_in_the_base_half() {
    assert!(matches!(
        expand_uvs_map_triples("U+FDFF..FE0F", "U+FE0F", "x"),
        Err(UvsExpandError::NotASelector {
            selector_half: false,
            ..
        }),
    ));
}

#[test]
fn an_unreadable_half_expands_to_nothing() {
    assert_eq!(
        expand_uvs_map_triples("nonsense", "U+FE0F", "x"),
        Err(UvsExpandError::Empty {
            selector_half: false
        }),
    );
    assert_eq!(
        expand_uvs_map_triples("U+0030", "nonsense", "x"),
        Err(UvsExpandError::Empty {
            selector_half: true
        }),
    );
}

/// The Mongolian selectors count, because the shaper's own range list has
/// them — see [`crate::ucd::is_variation_selector`].
#[test]
fn mongolian_free_variation_selectors_count() {
    assert_eq!(
        triples("U+1820", "U+180B", "a-fvs1"),
        vec![(0x1820, 0x180B, "a-fvs1".to_string())],
    );
}
