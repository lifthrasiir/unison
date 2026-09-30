//! Tests of alternative-glyph selection by anchor size, ordering and lookahead.

use super::*;

#[test]
fn anchor_range_parsing_and_size_match() {
    use crate::document_io;

    let input = "\
glyph target-wide 4 2
@@@@@@@@
@@@@@@@@
anchor -join 1..2 0..1

glyph target-narrow 2 2
@@@@
@@@@
anchor -join 0 0

glyph container 6 2
............
............
anchor +join 3..4 0..1
ref target-wide
ref target-narrow
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::default());

    let container = resolved.get("container").unwrap();
    // target-wide has 2x2 anchor matching +join (2x2): offset = 3-1 = 2, placed at col 2.
    // target-narrow has 1x1 anchor, doesn't match +join (2x2), falls back to (0,0).
    // container own 6px + target-wide 4px at col 2 → max(6, 2+4) = 6.
    // target-narrow 2px at col 0 → still within bounds.
    assert_eq!(container.grid.width, 6);
}

#[test]
fn alternative_glyph_selected_on_size_mismatch() {
    use crate::document_io;

    let input = "\
glyph stem 2 2
@@@@
@@@@
anchor -join 0 0

glyph stem:wide 4 2
@@@@@@@@
@@@@@@@@
anchor -join 0..1 0

glyph container 6 2
............
............
anchor +join 3..4 0
ref stem
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let name_parts = NamePartsMap::default();
    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    // stem has 1x1 anchor, +join is 2x1. stem:wide has 2x1 anchor → matches.
    let container = resolved.get("container").unwrap();
    // stem:wide is 4 wide, placed at col 3-0=3? No: +join col=3..4, -join col=0..1
    // offset = plus.col - minus.col = 3 - 0 = 3
    // container pixels: 6 wide, stem:wide at col 3 → extends to col 7.
    // total width = max(6, 3+4) = 7
    assert_eq!(container.grid.width, 7);

    // Verify via compute_composite that resolved_name is the alternative.
    let container_body = doc
        .items
        .iter()
        .find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "container" => Some(body),
            _ => None,
        })
        .unwrap();
    let composite = compute_composite(
        container_body,
        &resolved,
        &name_parts,
        &_alt_idx,
        &Default::default(),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(composite.layers[0].resolved_name, "stem:wide");
}

#[test]
fn alternative_glyph_alphabetical_priority() {
    use crate::document_io;

    let input = "\
glyph base 1 1
@@
anchor -a 0 0

glyph base:zzz 2 2
@@@@
@@@@
anchor -a 0..1 0..1

glyph base:aaa 2 2
@@@@
@@@@
anchor -a 0..1 0..1

glyph host 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +a 1..2 1..2
ref base
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let name_parts = NamePartsMap::default();
    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    let host_body = doc
        .items
        .iter()
        .find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "host" => Some(body),
            _ => None,
        })
        .unwrap();
    let composite = compute_composite(
        host_body,
        &resolved,
        &name_parts,
        &_alt_idx,
        &Default::default(),
        &Default::default(),
    )
    .unwrap();
    // base:aaa comes before base:zzz alphabetically.
    assert_eq!(composite.layers[0].resolved_name, "base:aaa");
}

#[test]
fn pattern_ref_selects_alternative_by_anchor_size() {
    use crate::document_io;

    let input = "\
name-parts $ab = a b

glyph enclosing 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +center 2 1..2

glyph a-inner 2 4
@@@@
@@@@
@@@@
@@@@
anchor -center 1 2

glyph b-inner 2 4
@@@@
@@@@
@@@@
@@@@
anchor -center 1 2

glyph b-inner:compressed 2 4
@@@@
@@@@
@@@@
@@@@
anchor -center 1 1..2

glyph ($ab)-combo
ref enclosing
ref ($ab)-inner
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    let b_refs = vec![
        GlyphRef {
            raw_name: None,
            comment: None,
            name: "enclosing".to_string(),
            offset: None,
            negated: false,
            inherit: false,
            goto: false,
            fill: None,
            visibility: None,
        },
        GlyphRef {
            raw_name: None,
            comment: None,
            name: "b-inner".to_string(),
            offset: None,
            negated: false,
            inherit: false,
            goto: false,
            fill: None,
            visibility: None,
        },
    ];
    let (effective, _, _) = derive_ref_offsets_with(
        &[],
        &b_refs,
        1,
        &AnchorAligns::default(),
        |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
        |name| alt_idx.get(name).to_vec(),
        |name| resolved.get(name).map(|r| r.declared_anchors.clone()),
        |_: &str| (0, 0),
    );
    assert_eq!(
        effective[1].name, "b-inner:compressed",
        "b-inner:compressed should be selected because its -center (1x2) matches +center (1x2)"
    );
}

/// An alternative that is itself a composite only enters the alternatives
/// index once it has been resolved. If that merge is deferred to the end of the
/// fixpoint round, every composite resolved later in the *same* round sees an
/// index without it, and a ref whose anchors only size-match that alternative
/// falls back to offset (0, 0) instead — which is what `i-upper` + `acute-above`
/// used to do, silently, in the shipped font.
#[test]
fn alternative_resolved_in_the_same_round_is_visible_to_later_composites() {
    use crate::document_io;

    let input = "\
glyph stroke 3 1 inline
@@@@@@

glyph mark-above mark
ref stroke
anchor -above 1 0

glyph mark-above:wide mark
ref stroke
anchor -above 0..1 0

glyph base 5 3
..........
..........
@@@@@@@@@@
anchor +above 2..3 1

glyph combo
ref base
ref mark-above

glyph combo-expected
ref base
ref mark-above:wide 2 1
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (resolved, _) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::default());

    // `mark-above` is one cell wide against `base`'s two-cell `+above`, so only
    // `mark-above:wide` can attach; both are composites, so both resolve in the
    // same round as `combo` itself.
    assert_eq!(
        resolved["combo"].grid, resolved["combo-expected"].grid,
        "the wide alternative should have been picked and anchored"
    );
    assert_ne!(
        resolved["combo"].grid, resolved["base"].grid,
        "the mark must not have collapsed onto the base at (0, 0)"
    );
}

/// The same, when the base name is a plain glyph and only its alternative is a
/// composite: nothing holds `combo` back a round, so the alternative it picks
/// is derived in the very wave `combo` is — and has to be drawn, not skipped
/// for not having been flattened yet.
#[test]
fn alternative_derived_in_the_same_wave_is_drawn_by_later_composites() {
    use crate::document_io;

    let input = "\
glyph stroke 3 1 inline
@@@@@@

glyph mark-above 1 1 mark
@@
anchor -above 0 0

glyph mark-above:wide mark
ref stroke
anchor -above 0..1 0

glyph base 5 3
..........
..........
@@@@@@@@@@
anchor +above 2..3 1

glyph combo
ref base
ref mark-above

glyph combo-expected
ref base
ref mark-above:wide 2 1
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (resolved, _) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::default());
    assert_eq!(
        resolved["combo"].grid, resolved["combo-expected"].grid,
        "the wide alternative should have been picked, anchored and drawn"
    );
}

#[test]
fn lookahead_selects_alternative_when_later_ref_consumes_forwarded_anchor() {
    use crate::document_io;

    let input = "\
glyph base:alt 2 2
@@@@
@@@@
anchor +above 1 0

glyph base 2 4
@@@@
@@@@
....
....
ref base:alt 0 2 inherit
anchor +below 1 3

glyph mark-above 2 1 mark
@@@@
anchor -above 1 0

glyph mark-below 2 1 mark
@@@@
anchor -below 1 0

glyph combo-above
ref base
ref mark-above

glyph combo-below
ref base
ref mark-below
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (resolved, alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::default());

    let mut decl_anchors: HashMap<String, Vec<GlyphPoint>> = HashMap::default();
    for item in &doc.items {
        if let DocumentItem::Glyph { name, body } = item {
            decl_anchors
                .entry(name.display())
                .or_insert_with(|| body.points.clone());
        }
    }

    // combo-above: base + mark-above → should substitute base:alt
    // because base's own points lack +above (forwarded from ref base:alt)
    let above_body = doc
        .items
        .iter()
        .find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "combo-above" => Some(body),
            _ => None,
        })
        .unwrap();
    let (effective, _, _) = derive_ref_offsets_with(
        &above_body.points,
        &above_body.refs,
        1,
        &AnchorAligns::default(),
        |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
        |name| alt_idx.get(name).to_vec(),
        |name| decl_anchors.get(name).cloned(),
        |_: &str| (0, 0),
    );
    assert_eq!(
        effective[0].name, "base:alt",
        "should select base:alt for mark-above (base's own points lack +above)"
    );

    // combo-below: base + mark-below → should NOT substitute
    // because base's own points include +below
    let below_body = doc
        .items
        .iter()
        .find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "combo-below" => Some(body),
            _ => None,
        })
        .unwrap();
    let (effective, _, _) = derive_ref_offsets_with(
        &below_body.points,
        &below_body.refs,
        1,
        &AnchorAligns::default(),
        |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
        |name| alt_idx.get(name).to_vec(),
        |name| decl_anchors.get(name).cloned(),
        |_: &str| (0, 0),
    );
    assert_eq!(
        effective[0].name, "base",
        "should keep base for mark-below (base's own points have +below)"
    );
}

/// `inherit` decides what a composite *exposes*; it must not decide *which
/// form* of a glyph gets picked when a sibling ref needs an anchor. Choosing
/// between `base` and `base:alt` is a question about `base:alt` — does it
/// declare `+above`? — and `ttf_builder/gpos.rs` already answers it from
/// `declared_anchors` plus the alternative index, never from the flag. The
/// look-ahead used to read the primary's *exposed* set instead, so dropping
/// `inherit` from `glyph i-lower`'s `ref i-lower:dotless` silently made every
/// generated `ï í ì ī ǐ î ĭ ĩ` compose over the dotted form.
#[test]
fn lookahead_alternative_does_not_depend_on_inherit() {
    use crate::document_io;

    // Same font twice, differing only in the flag on `base`'s own ref.
    let source = |inherit: &str| {
        format!(
            "\
glyph base:alt 2 2
@@@@
@@@@
anchor +above 1 0

glyph base 2 4
@@@@
@@@@
....
....
ref base:alt 0 2{inherit}
anchor +below 1 3

glyph mark-above 2 1 mark
@@@@
anchor -above 1 0

glyph combo-above
ref base
ref mark-above
"
        )
    };

    let resolve = |inherit: &str| {
        let doc =
            document_io::parse_document_from_str(&source(inherit), "test.unf".into()).unwrap();
        let (resolved, alt_idx) =
            resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::default());
        let mut decl_anchors: HashMap<String, Vec<GlyphPoint>> = HashMap::default();
        for item in &doc.items {
            if let DocumentItem::Glyph { name, body } = item {
                decl_anchors
                    .entry(name.display())
                    .or_insert_with(|| body.points.clone());
            }
        }
        let body = doc
            .items
            .iter()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == "combo-above" => Some(body),
                _ => None,
            })
            .unwrap()
            .clone();
        let (effective, _, _) = derive_ref_offsets_with(
            &body.points,
            &body.refs,
            1,
            &AnchorAligns::default(),
            |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
            |name| alt_idx.get(name).to_vec(),
            |name| decl_anchors.get(name).cloned(),
            |_: &str| (0, 0),
        );
        let base_exposes_above = resolved["base"]
            .resolved_anchors
            .iter()
            .any(|p| p.position == "+above");
        (
            effective,
            resolved["combo-above"].grid.clone(),
            base_exposes_above,
        )
    };

    let (with_refs, with_grid, with_above) = resolve(" inherit");
    let (without_refs, without_grid, without_above) = resolve("");

    // The flag does its one job: only the `inherit` form forwards `+above`.
    assert!(with_above, "`ref base:alt inherit` should expose +above");
    assert!(!without_above, "a non-inherit ref should expose nothing");

    // And nothing else. Both pick the alternative that declares `+above`.
    for (label, effective) in [("inherit", &with_refs), ("no inherit", &without_refs)] {
        assert_eq!(
            effective[0].name, "base:alt",
            "{label}: the mark's -above must pick the form declaring +above",
        );
        assert_eq!(
            effective[1].name, "mark-above",
            "{label}: the mark itself has no alternative to pick",
        );
    }
    assert_eq!(
        with_refs.iter().map(|r| r.offset).collect::<Vec<_>>(),
        without_refs.iter().map(|r| r.offset).collect::<Vec<_>>(),
        "attachment offsets must not depend on the flag",
    );
    assert_eq!(
        with_grid, without_grid,
        "the composed grid must not depend on the flag"
    );
}
