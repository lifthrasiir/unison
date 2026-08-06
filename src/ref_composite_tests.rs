//! Tests for [`super::ref_composite`].
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items while keeping the source at a readable size.

use super::*;
use crate::on_demand::detect_color_mono_glyph;
use crate::pixel::PixelShape;

fn filled_grid(w: u16, h: u16) -> PixelGrid {
    let mut g = PixelGrid::new(w, h);
    for r in 0..h {
        for c in 0..w {
            g.set(r, c, PixelShape::new(0, true));
        }
    }
    g
}

/// Ref names resolve via `resolve_ref_name_with_parts`, which falls back
/// to `parse_ref_pattern` when a direct cache lookup misses (e.g. a ref
/// pointing at a pattern name like "digit(0|1)" whose expansions, not
/// the raw pattern string, are the cache keys). `composite_to_grid` used
/// to do a bare `cache.get(&gref.name)` with no such fallback, so the
/// same ref would render live via `compute_composite` but silently drop
/// out of the flattened grid. Both now share `resolve_composite_layout`,
/// which this test pins down.
#[test]
fn composite_to_grid_resolves_pattern_refs_like_compute_composite() {
    let mut cache: HashMap<String, ResolvedGlyph> = HashMap::new();
    cache.insert(
        "digit0".to_string(),
        ResolvedGlyph {
            grid: filled_grid(2, 2),
            origin_row: 0,
            origin_col: 0,
            resolved_anchors: Vec::new(),
            declared_anchors: Vec::new(),
            scale: 1,
        },
    );

    let refs = vec![GlyphRef {
        comment: None,
        name: "digit(0|1)".to_string(),
        offset: None,
        negated: false,
        inherit: false,
        fill: None,
        visibility: None,
    }];

    // compute_composite resolves the pattern ref via the shared layout's
    // fallback and includes the layer.
    let body = GlyphBody {
        refs: refs.clone(),
        ..GlyphBody::new()
    };
    let empty_parts = NamePartsMap::new();
    let composite = compute_composite(
        &body,
        &cache,
        &empty_parts,
        &AlternativesIndex::default(),
        &Default::default(),
    )
    .expect("has refs");
    assert_eq!(
        composite.layers.len(),
        1,
        "compute_composite should include the pattern-resolved layer"
    );

    // composite_to_grid must resolve the same ref the same way, and thus
    // produce a non-empty grid with the layer's pixels present.
    let grid = composite_to_grid(&None, &refs, &cache, &empty_parts, 1);
    assert_eq!(
        grid.get(0, 0),
        PixelShape::new(0, true),
        "composite_to_grid should include the pattern-resolved layer's pixels"
    );
}

#[test]
fn adjoin_resolves_offset_from_points() {
    use crate::document_io;

    let input = "\
glyph target 10 10
....................
....................
....................
....................
....................
....................
....................
....................
....................
....................
anchor -blah 5 5

glyph container 12 12
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
anchor +blah 3 3
ref target
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();

    let docs = vec![&doc];
    let name_parts = NamePartsMap::new();
    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    let container = resolved
        .get("container")
        .expect("container should be resolved");
    // target placed at offset (col=3-5, row=3-5) = (-2, -2).
    // Container own pixels 12×12 at (0,0), target 10×10 at (-2,-2).
    // Bounding box: min=-2, max=12 → total 14×14.
    assert_eq!(
        container.grid.width, 14,
        "width should be 14 (12 + 2 for negative offset)"
    );
    assert_eq!(
        container.grid.height, 14,
        "height should be 14 (12 + 2 for negative offset)"
    );
}

#[test]
fn auto_offsets_are_rederived_without_mutating_source_refs() {
    use crate::document_io;

    let input = "\
glyph target 1 1
@@
anchor -join 0 0

glyph container 1 1
..
anchor +join 3 0
ref target
";
    let mut doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let name_parts = NamePartsMap::new();

    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);
    assert_eq!(resolved["container"].grid.width, 4);
    let container_body = doc
        .items
        .iter()
        .find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "container" => Some(body),
            _ => None,
        })
        .unwrap();
    assert_eq!(container_body.refs[0].offset, None);
    let composite = compute_composite(
        container_body,
        &resolved,
        &name_parts,
        &_alt_idx,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        (
            composite.layers[0].offset_row - composite.own_offset_row,
            composite.layers[0].offset_col - composite.own_offset_col,
        ),
        (0, 3)
    );

    let target_body = doc
        .items
        .iter_mut()
        .find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "target" => Some(body),
            _ => None,
        })
        .unwrap();
    target_body.points[0].col = 2;
    target_body.points[0].col_end = 2;

    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);
    assert_eq!(resolved["container"].grid.width, 2);
    let container_body = doc
        .items
        .iter()
        .find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "container" => Some(body),
            _ => None,
        })
        .unwrap();
    assert_eq!(container_body.refs[0].offset, None);
    let composite = compute_composite(
        container_body,
        &resolved,
        &name_parts,
        &_alt_idx,
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        (
            composite.layers[0].offset_row - composite.own_offset_row,
            composite.layers[0].offset_col - composite.own_offset_col,
        ),
        (0, 1)
    );
}

#[test]
fn anchors_are_forwarded_transitively_and_publish_after_consume() {
    use crate::document_io;

    let input = "\
glyph link 1 1
@@
anchor -join 0 0
anchor +join 2 0

glyph wrapped
ref link inherit

glyph chain 1 1
..
anchor +join 0 0
ref wrapped
ref wrapped
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());
    assert_eq!(resolved["chain"].grid.width, 3);
    assert!(resolved["chain"].grid.get(0, 0).is_filled());
    assert!(resolved["chain"].grid.get(0, 2).is_filled());
}

#[test]
fn substituted_and_pattern_refs_resolve_in_all_container_shapes() {
    use crate::document_io;

    let input = "\
name-parts $base = stem

glyph stem 1 1
@@

glyph stem-a 1 1
@@

glyph stem-b 1 1
@@

glyph via-parts
ref $base

glyph via-pattern
ref stem-(a|b)

glyph pair-(a|b)
ref $base

glyph uni(2800|2801)
ref $base

glyph pipe-a|pipe-b
ref $base
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    for name in [
        "via-parts",
        "via-pattern",
        "pair-a",
        "pair-b",
        "uni2800",
        "uni2801",
        "pipe-a",
        "pipe-b",
    ] {
        assert!(
            resolved
                .get(name)
                .is_some_and(|g| g.grid.get(0, 0).is_filled()),
            "{name} did not resolve"
        );
    }
    assert!(is_ref_valid("$base", &resolved, &name_parts));
    assert!(is_ref_valid("stem-(a|b)", &resolved, &name_parts));
}

#[test]
fn adjoin_resolves_minus_before_plus_ref_order() {
    use crate::document_io;

    let input = "\
glyph inner 8 8
................
................
................
................
................
................
................
................
anchor +center 4 4

glyph outer 12 12
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
........................
anchor -center 6 6

glyph combo-plus-first
ref inner
ref outer

glyph combo-minus-first
ref outer
ref inner
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());

    let pf = resolved.get("combo-plus-first").unwrap();
    let mf = resolved.get("combo-minus-first").unwrap();
    assert_eq!(
        (pf.grid.width, pf.grid.height),
        (mf.grid.width, mf.grid.height),
        "ref order should not affect resolved dimensions"
    );
}

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
    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());

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
    let name_parts = NamePartsMap::new();
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
    let name_parts = NamePartsMap::new();
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
            comment: None,
            name: "enclosing".to_string(),
            offset: None,
            negated: false,
            inherit: false,
            fill: None,
            visibility: None,
        },
        GlyphRef {
            comment: None,
            name: "b-inner".to_string(),
            offset: None,
            negated: false,
            inherit: false,
            fill: None,
            visibility: None,
        },
    ];
    let (effective, _, _) = derive_ref_offsets_with(
        &[],
        &b_refs,
        |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
        |name| alt_idx.get(name).to_vec(),
        |name| resolved.get(name).map(|r| r.declared_anchors.clone()),
    );
    assert_eq!(
        effective[1].name, "b-inner:compressed",
        "b-inner:compressed should be selected because its -center (1x2) matches +center (1x2)"
    );
}

#[test]
fn overlapping_subpixel_contours_are_correct() {
    use crate::document_io;
    use crate::pixel::PX_SUBPIXEL;
    use crate::render::contour::track_contour_multi;

    // HALF1 (1\, bottom-left triangle) + HALF2 (\1, top-right triangle) = full
    let input = "\
glyph base 1 1
1\\

glyph overlay 1 1
\\1

glyph combined
ref base
ref overlay
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (resolved, _) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());
    let base = &resolved["base"].grid;
    let overlay = &resolved["overlay"].grid;
    let contours = track_contour_multi(&[(base, 0, 0), (overlay, 0, 0)], PX_SUBPIXEL);
    assert_eq!(
        contours.len(),
        1,
        "complement halves should form one full-pixel contour"
    );
    let path = &contours[0];
    assert!(path.contains(&(0.0, 0.0)));
    assert!(path.contains(&(1.0, 0.0)));
    assert!(path.contains(&(1.0, 1.0)));
    assert!(path.contains(&(0.0, 1.0)));
}

#[test]
fn own_grid_plus_ref_contours_are_unioned() {
    use crate::document_io;
    use crate::pixel::PX_SUBPIXEL;
    use crate::render::contour::track_contour_multi;

    let input = "\
glyph part 1 1
\\1

glyph host 1 1
1\\
ref part
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (resolved, _) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());
    let host_grid = &resolved["host"].grid;
    let part_grid = &resolved["part"].grid;

    // The own grid has HALF1 at (0,0), the ref has HALF2 at (0,0).
    // track_contour_multi should trace the union as a full pixel.
    let contours = track_contour_multi(&[(host_grid, 0, 0), (part_grid, 0, 0)], PX_SUBPIXEL);
    assert_eq!(contours.len(), 1);
    let path = &contours[0];
    assert!(path.contains(&(0.0, 0.0)));
    assert!(path.contains(&(1.0, 1.0)));
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
    let (resolved, _) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());

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
    let (resolved, alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());

    let mut decl_anchors: HashMap<String, Vec<GlyphPoint>> = HashMap::new();
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
        |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
        |name| alt_idx.get(name).to_vec(),
        |name| decl_anchors.get(name).cloned(),
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
        |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
        |name| alt_idx.get(name).to_vec(),
        |name| decl_anchors.get(name).cloned(),
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
        let (resolved, alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::new());
        let mut decl_anchors: HashMap<String, Vec<GlyphPoint>> = HashMap::new();
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
            |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
            |name| alt_idx.get(name).to_vec(),
            |name| decl_anchors.get(name).cloned(),
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

#[test]
fn on_demand_fractional_rect_resolved() {
    // 1p2r3x4 → scale 3, grid 6×12, rect (0,0)-(5,12)
    let doc = make_doc("glyph container\n  ref 1p2r3x4\n");
    let name_parts = NamePartsMap::new();
    let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    assert!(cache.contains_key("1p2r3x4"));
    let resolved = &cache["1p2r3x4"];
    assert_eq!(resolved.scale, 3);
    assert_eq!(resolved.grid.width, 6);
    assert_eq!(resolved.grid.height, 12);
    // Geometry stops at subcolumn 5, but the ink flag is decided per
    // logical pixel: the second one is covered ⅔, which rounds up, so the
    // bitmap is a full 2 columns wide.
    for r in 0..12 {
        for c in 0..6 {
            assert_eq!(
                resolved.grid.get(r, c).shape_id() != crate::pixel::PX_EMPTY,
                c < 5,
                "pixel ({r},{c}) geometry"
            );
            assert!(
                resolved.grid.get(r, c).is_filled(),
                "pixel ({r},{c}) should be inked"
            );
        }
    }
}

#[test]
fn on_demand_fractional_rect_neg_anchoring() {
    // -1p2r3x-1p1r3 → scale 3, grid 6×3
    // rect 5×4, right-aligned → off_c=1, bottom-aligned → off_r=−1
    // Wait: extent_w = ceil(5/3) = 2, grid_w = 6, off_c = 6-5 = 1
    //        extent_h = ceil(4/3) = 2, grid_h = 6, off_r = 6-4 = 2
    let doc = make_doc("glyph container\n  ref -1p2r3x-1p1r3\n");
    let name_parts = NamePartsMap::new();
    let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    let resolved = &cache["-1p2r3x-1p1r3"];
    assert_eq!(resolved.scale, 3);
    assert_eq!(resolved.grid.width, 6);
    assert_eq!(resolved.grid.height, 6);
    // geometry: cols 1..6, rows 2..6
    for r in 0..6 {
        for c in 0..6 {
            assert_eq!(
                resolved.grid.get(r, c).shape_id() != crate::pixel::PX_EMPTY,
                c >= 1 && r >= 2,
                "pixel ({r},{c}) geometry"
            );
            // Ink, per logical pixel: both columns are covered ⅔ or more
            // and round up, but logical row 0 holds only subrow 2 — ⅓ —
            // and stays dark.
            assert_eq!(
                resolved.grid.get(r, c).is_filled(),
                r >= 3,
                "pixel ({r},{c}) fill={} expected={}",
                resolved.grid.get(r, c).is_filled(),
                r >= 3,
            );
        }
    }
}

fn make_doc(text: &str) -> Document {
    use crate::document_io::{derive_document, parse_doclines};
    let lines = parse_doclines(text);
    let (doc, _) = derive_document(&lines, std::path::PathBuf::new()).unwrap();
    doc
}

#[test]
fn on_demand_glyph_injected_for_ref() {
    let doc = make_doc("glyph test 3 5\n......\n......\n......\n......\n......\n  ref 2x3\n");
    let name_parts = NamePartsMap::new();
    let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    assert!(cache.contains_key("2x3"));
    let resolved = &cache["2x3"];
    assert_eq!(resolved.grid.width, 2);
    assert_eq!(resolved.grid.height, 3);
    for r in 0..3 {
        for c in 0..2 {
            assert!(resolved.grid.get(r, c).is_filled());
        }
    }
}

#[test]
fn on_demand_glyph_composite_resolves() {
    let doc = make_doc("glyph composite\n  ref 3x2\n");
    let name_parts = NamePartsMap::new();
    let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    assert!(
        cache.contains_key("3x2"),
        "on-demand glyph 3x2 missing from cache"
    );
    assert!(
        cache.contains_key("composite"),
        "composite glyph missing from cache"
    );
    let comp = &cache["composite"];
    assert_eq!(comp.grid.width, 3);
    assert_eq!(comp.grid.height, 2);
    for r in 0..2 {
        for c in 0..3 {
            assert!(
                comp.grid.get(r, c).is_filled(),
                "composite pixel ({r},{c}) should be filled"
            );
        }
    }
}

#[test]
fn on_demand_glyph_resolves_in_multi_ref_composite() {
    let doc = make_doc(concat!(
        "glyph base 2 2\n@@@@\n@@@@\n",
        "glyph comp\n",
        "  ref base\n",
        "  ref 3x2 2 0\n",
    ));
    let name_parts = NamePartsMap::new();
    let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    assert!(
        cache.contains_key("3x2"),
        "on-demand 3x2 should be in cache"
    );
    assert!(cache.contains_key("comp"), "comp should resolve");
    let comp = &cache["comp"];
    assert!(
        comp.grid.width >= 5,
        "composite width should span base(2) + 3x2 at col 2"
    );
}

#[test]
fn on_demand_glyph_not_injected_when_defined() {
    let doc = make_doc("glyph 2x3 2 3\n....\n....\n....\n");
    let name_parts = NamePartsMap::new();
    let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    let resolved = &cache["2x3"];
    for r in 0..3 {
        for c in 0..2 {
            assert!(!resolved.grid.get(r, c).is_filled());
        }
    }
}

#[test]
fn color_mono_on_demand_glyph_created() {
    let doc = make_doc(concat!(
        "glyph part-a 2 2\n@@@@\n@@@@\n",
        "glyph part-b 2 2\n@@@@\n@@@@\n",
        "glyph test:mono\n  ref part-a\n",
        "glyph test:color\n  ref part-b\n",
        "glyph container\n  ref test\n",
    ));
    let name_parts = NamePartsMap::new();
    let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    assert!(
        cache.contains_key("test"),
        "color/mono on-demand glyph 'test' should be synthesized"
    );
    let resolved = &cache["test"];
    assert_eq!(resolved.grid.width, 2);
    assert_eq!(resolved.grid.height, 2);
}

#[test]
fn color_mono_on_demand_not_created_when_name_contains_mono_or_color() {
    assert_eq!(detect_color_mono_glyph("foo:mono", |_| true), None);
    assert_eq!(detect_color_mono_glyph("foo:color", |_| true), None);
    assert_eq!(detect_color_mono_glyph("foo:mono:bar", |_| true), None);
}

#[test]
fn color_mono_on_demand_not_created_when_defined() {
    let doc = make_doc(concat!(
        "glyph part 2 2\n@@@@\n@@@@\n",
        "glyph test:mono\n  ref part\n",
        "glyph test:color\n  ref part\n",
        "glyph test\n  ref part\n",
        "glyph container\n  ref test\n",
    ));
    let name_parts = NamePartsMap::new();
    let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    assert!(cache.contains_key("test"));
}

#[test]
fn color_mono_on_demand_not_created_when_only_mono_exists() {
    let doc = make_doc(concat!(
        "glyph part 2 2\n@@@@\n@@@@\n",
        "glyph test:mono\n  ref part\n",
        "glyph container\n  ref test\n",
    ));
    let name_parts = NamePartsMap::new();
    let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    assert!(
        !cache.contains_key("test"),
        "should not synthesize when only :mono exists"
    );
}

/// Manual profiling harness:
/// `cargo test -r profile_resolve_name_expansion -- --ignored --nocapture`
/// Loads the real font sources and times a cold resolve (the derived
/// rebuild stage that includes name expansion) plus a full font build.
/// `UNIFORM_PROFILE_RUNS=N` controls the resolve repeat count (useful
/// for attaching a sampling profiler).
#[test]
#[ignore]
fn profile_resolve_name_expansion() {
    let docs =
        crate::render::ttf_builder::load_docs_from_directory_checked(std::path::Path::new("font"))
            .0;
    assert!(!docs.is_empty(), "font/ not found; run from repo root");
    let refs: Vec<&Document> = docs.iter().collect();
    let name_parts = crate::document::collect_name_parts(&refs);
    let runs: usize = std::env::var("UNIFORM_PROFILE_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    for run in 0..runs {
        let t0 = std::time::Instant::now();
        let (resolved, _alt) = resolve_named_glyphs_with_parts(&refs, &name_parts);
        eprintln!(
            "run {run}: resolve {:?}, {} glyphs",
            t0.elapsed(),
            resolved.len()
        );
    }
    let t0 = std::time::Instant::now();
    let built = crate::render::build_font_from_documents(&refs);
    eprintln!("font build: {:?}, ok={}", t0.elapsed(), built.is_some());
}

/// Anchor inheritance is opt-in: a composite exposes only its own declared
/// anchors plus the surviving anchors of refs marked `inherit`. Attachment
/// *inside* the composite works regardless of the flag.
#[test]
fn anchor_exposure_requires_inherit() {
    let input = "\
glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 1 0
anchor +below 1 3
glyph mark 2 1 mark
@@@@
anchor -above 0 0
anchor +above 0 -1
glyph opaque
ref base
ref mark
glyph transparent
ref base inherit
ref mark inherit
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, _alt) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    // The mark attached in both composites (its -above consumed base's +above).
    for name in ["opaque", "transparent"] {
        let g = &resolved[name];
        assert!(
            !g.resolved_anchors.iter().any(|p| p.position == "-above"),
            "{name}: consumed -above must not be exposed: {:?}",
            g.resolved_anchors,
        );
    }

    let opaque = &resolved["opaque"];
    assert!(
        opaque.resolved_anchors.is_empty(),
        "no inherit, no declared anchors: nothing exposed, got {:?}",
        opaque.resolved_anchors,
    );

    let transparent = &resolved["transparent"];
    let positions: Vec<&str> = transparent
        .resolved_anchors
        .iter()
        .map(|p| p.position.as_str())
        .collect();
    assert!(
        positions.contains(&"+below"),
        "base's +below survives: {positions:?}"
    );
    assert!(
        positions.contains(&"+above"),
        "mark's republished +above survives: {positions:?}"
    );
    let above = transparent
        .resolved_anchors
        .iter()
        .find(|p| p.position == "+above")
        .unwrap();
    // mark's own +above (0, -1) translated by the attachment offset (1, 0).
    assert_eq!(
        (above.col, above.row),
        (1, -1),
        "the surviving +above is the mark's, moved"
    );
}

/// Two inherit refs surviving with the same anchor name is an error, and the
/// fallback acts as if that anchor did not exist at all — a digraph must not
/// pick one side's attachment point silently.
#[test]
fn duplicate_exposed_anchors_are_dropped() {
    let input = "\
glyph half 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 1 0
anchor +below 1 3
glyph digraph
ref half 0 0 inherit
ref half 4 0 inherit
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, _alt) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    let digraph = &resolved["digraph"];
    assert!(
        digraph.resolved_anchors.is_empty(),
        "both +above and both +below collide; all must be dropped, got {:?}",
        digraph.resolved_anchors,
    );
}

/// A minus anchor no remaining ref can ever satisfy must not defer its ref:
/// deferral would let explicit-offset sibling refs commit first, miss their
/// consumption, and leave the base's occupied anchor exposed.
#[test]
fn unsatisfiable_minus_does_not_defer_commit_order() {
    let input = "\
glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor -center 1 1
anchor +below 1 3
glyph dot 1 1 mark
@@
anchor -below 0 0
anchor +below 0 1
glyph comp
ref base inherit
ref dot 1 3
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, _alt) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    let comp = &resolved["comp"];
    let positions: Vec<&str> = comp
        .resolved_anchors
        .iter()
        .map(|p| p.position.as_str())
        .collect();
    assert!(
        positions.contains(&"-center"),
        "base's unsatisfiable -center is forwarded through inherit: {positions:?}"
    );
    assert!(
        !positions.contains(&"+below"),
        "base must commit before the explicit-offset dot so the dot consumes \
         +below; it must not linger exposed: {positions:?}"
    );
}

/// `map generate` composites stand in for their decomposition, so their
/// synthesized refs inherit implicitly: the generated glyph exposes the
/// surviving anchors exactly as the hand-written equivalent with `inherit`.
#[test]
fn map_generate_refs_inherit_implicitly() {
    let input = "\
glyph a-upper 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 1 0
anchor +below 1 3
glyph grave 2 1 mark
@@@@
anchor -above 0 0
anchor +above 0 -1
map A = a-upper
map U+0300 = grave
map generate \u{c0}
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, _alt) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    let generated = resolved.get("uni00C0").expect("generated composite");
    let positions: Vec<&str> = generated
        .resolved_anchors
        .iter()
        .map(|p| p.position.as_str())
        .collect();
    assert!(positions.contains(&"+above"), "{positions:?}");
    assert!(positions.contains(&"+below"), "{positions:?}");
    assert!(!positions.contains(&"-above"), "{positions:?}");
}

/// The derive-level diagnostics carry which anchor collided and which ref
/// attached ambiguously, so issues.rs can report them per glyph.
#[test]
fn derive_reports_duplicates_and_ambiguity() {
    let anchored = |position: &str, col: i16, row: i16| GlyphPoint {
        comment: None,
        position: position.to_string(),
        col,
        row,
        col_end: col,
        row_end: row,
    };
    let lookup = |name: &str| -> Option<Vec<GlyphPoint>> {
        match name {
            "half" => Some(vec![anchored("+above", 1, 0)]),
            "mark" => Some(vec![anchored("-above", 0, 0)]),
            _ => None,
        }
    };
    let inherit_ref = |name: &str, col: i16| GlyphRef {
        comment: None,
        name: name.to_string(),
        offset: Some((col, 0)),
        negated: false,
        inherit: true,
        fill: None,
        visibility: None,
    };

    // Two inherited +above survive → both dropped, one issue.
    let refs = vec![inherit_ref("half", 0), inherit_ref("half", 4)];
    let (_, exposed, issues) = derive_ref_offsets_with(&[], &refs, lookup, |_| Vec::new(), lookup);
    assert!(exposed.is_empty(), "{exposed:?}");
    assert_eq!(
        issues,
        vec![DeriveIssue::DuplicateExposed {
            position: "+above".into()
        }],
    );

    // A mark whose -above finds two +above candidates attaches to neither.
    let refs = vec![
        inherit_ref("half", 0),
        inherit_ref("half", 4),
        GlyphRef {
            offset: None,
            inherit: false,
            ..inherit_ref("mark", 0)
        },
    ];
    let (effective, _, issues) =
        derive_ref_offsets_with(&[], &refs, lookup, |_| Vec::new(), lookup);
    assert_eq!(effective[2].offset, Some((0, 0)), "unattached fallback");
    assert!(
        issues.contains(&DeriveIssue::AmbiguousAttachment {
            position: "-above".into(),
            ref_name: "mark".into(),
        }),
        "{issues:?}",
    );
}

/// Manual migration helper: compares the current opt-in anchor exposure over
/// `font/` against forward-everything (every ref forced `inherit`), listing
/// the glyphs whose `+above`/`+below` disappeared.
/// `cargo test -r probe_migration_worklist -- --ignored --nocapture`
#[test]
#[ignore]
fn probe_migration_worklist() {
    let docs =
        crate::render::ttf_builder::load_docs_from_directory_checked(std::path::Path::new("font"))
            .0;
    let refs: Vec<&Document> = docs.iter().collect();
    let name_parts = crate::document::collect_name_parts(&refs);
    let (resolved, _alt) = resolve_named_glyphs_with_parts(&refs, &name_parts);

    // The same font with every ref forced to inherit approximates the old
    // forward-everything behavior.
    let mut all_inherit: Vec<Document> = docs.clone();
    for doc in &mut all_inherit {
        for item in &mut doc.items {
            if let DocumentItem::Glyph { body, .. } = item {
                for r in &mut body.refs {
                    r.inherit = true;
                }
            }
        }
    }
    let refs2: Vec<&Document> = all_inherit.iter().collect();
    let (resolved_old, _alt2) = resolve_named_glyphs_with_parts(&refs2, &name_parts);

    for pos in ["-above", "-below", "+above", "+below"] {
        let now = resolved
            .values()
            .filter(|g| g.resolved_anchors.iter().any(|p| p.position == pos))
            .count();
        let old = resolved_old
            .values()
            .filter(|g| g.resolved_anchors.iter().any(|p| p.position == pos))
            .count();
        eprintln!("{pos}: exposed by {now} glyphs (forward-everything: {old})");
    }

    let mut lost: Vec<&str> = Vec::new();
    for (name, old) in &resolved_old {
        let Some(new) = resolved.get(name) else {
            continue;
        };
        for pos in ["+above", "+below"] {
            let had = old.resolved_anchors.iter().any(|p| p.position == pos);
            let has = new.resolved_anchors.iter().any(|p| p.position == pos)
                || new.declared_anchors.iter().any(|p| p.position == pos);
            if had && !has {
                lost.push(name);
                break;
            }
        }
    }
    lost.sort();
    lost.dedup();
    eprintln!(
        "== {} glyphs no longer expose a +above/+below they used to:",
        lost.len()
    );
    for n in &lost {
        eprintln!("  {n}");
    }
}

/// A `-` anchor that finds a same-name `+` of a *different size* attaches to
/// nothing, and that near-miss is reported: it almost always means the wrong
/// `:narrow`/`:wide` variant was picked. A minus with no same-name `+` at all
/// stays quiet — that is ordinary alias forwarding.
#[test]
fn derive_reports_size_mismatched_attachment() {
    let anchored = |position: &str, col: i16, row: i16, w: i16| GlyphPoint {
        comment: None,
        position: position.to_string(),
        col,
        row,
        col_end: col + w - 1,
        row_end: row,
    };
    let lookup = |name: &str| -> Option<Vec<GlyphPoint>> {
        match name {
            "base" => Some(vec![anchored("+above", 1, 0, 2)]),
            "mark" => Some(vec![anchored("-above", 0, 0, 1)]),
            _ => None,
        }
    };
    let gref = |name: &str, offset: Option<(i16, i16)>| GlyphRef {
        comment: None,
        name: name.to_string(),
        offset,
        negated: false,
        inherit: false,
        fill: None,
        visibility: None,
    };

    // Explicit offset: the mark cannot consume the 2-cell +above.
    let refs = vec![gref("base", None), gref("mark", Some((1, 2)))];
    let (_, _, issues) = derive_ref_offsets_with(&[], &refs, lookup, |_| Vec::new(), lookup);
    assert!(
        issues.contains(&DeriveIssue::SizeMismatchedAttachment {
            position: "-above".into(),
            ref_name: "mark".into(),
            minus: (1, 1),
            plus: (2, 1),
        }),
        "{issues:?}",
    );

    // No same-name + anywhere: plain forwarding, no warning.
    let refs = vec![gref("mark", None)];
    let (_, _, issues) = derive_ref_offsets_with(&[], &refs, lookup, |_| Vec::new(), lookup);
    assert!(issues.is_empty(), "{issues:?}");
}

/// Several `-` anchors on one ref target are *alternatives*, not several
/// attachments: one combining mark that can adjoin to more than one anchor
/// system. Attaching through one retires the rest — they neither survive as
/// exposed anchors nor warn about a same-name `+` of another size — and their
/// `+` partners go with them, so the mark publishes only the system it
/// actually joined.
///
/// `gr-psili` over a Greek capital is the case: it declares `-gr-above` and
/// forwards `-above` from `com-above:narrow`, while the capital publishes a
/// 1-cell `+gr-above` and a 2-cell `+above`. It joins the Greek system; the
/// leftover 1-cell `-above` used to survive, warn against the capital's
/// 2-cell `+above`, and publish a second `+above` that collided with it.
#[test]
fn attaching_through_one_minus_retires_the_other_alternatives() {
    let anchored = |position: &str, col: i16, row: i16, w: i16| GlyphPoint {
        comment: None,
        position: position.to_string(),
        col,
        row,
        col_end: col + w - 1,
        row_end: row,
    };
    let lookup = |name: &str| -> Option<Vec<GlyphPoint>> {
        match name {
            "cap" => Some(vec![
                anchored("+above", 3, 1, 2),
                anchored("+gr-above", 0, 3, 1),
            ]),
            "psili" => Some(vec![
                anchored("-gr-above", 1, 1, 1),
                anchored("+gr-above", 1, -1, 1),
                anchored("-above", 1, 2, 1),
                anchored("+above", 1, -1, 1),
            ]),
            _ => None,
        }
    };
    let gref = |name: &str| GlyphRef {
        comment: None,
        name: name.to_string(),
        offset: None,
        negated: false,
        inherit: true,
        fill: None,
        visibility: None,
    };

    let refs = vec![gref("cap"), gref("psili")];
    let (effective, exposed, issues) =
        derive_ref_offsets_with(&[], &refs, lookup, |_| Vec::new(), lookup);

    // Joined through -gr-above: offset = plus(0,3) - minus(1,1).
    assert_eq!(effective[1].offset, Some((-1, 2)));
    assert!(issues.is_empty(), "{issues:?}");

    let mut positions: Vec<&str> = exposed.iter().map(|(p, _)| p.position.as_str()).collect();
    positions.sort_unstable();
    // The capital's untouched +above, and the Greek system psili published.
    // Not psili's -above (retired), and so not a second +above either.
    assert_eq!(positions, vec!["+above", "+gr-above"]);
}

/// Size-based alternative selection still runs for offset-less refs — and it
/// is exactly what the size-mismatch warning defers to: the uni1E2E shape
/// (a narrow mark stacked on a wide mark's 2-cell `+above`) picks the
/// `:wide` alternative and stays quiet, while the same refs pinned by
/// explicit offsets cannot substitute and warn instead.
#[test]
fn offsetless_stacked_mark_picks_wide_alternative_without_warning() {
    let input = "\
glyph i-compressed 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 1..2 0

glyph dia 4 1 mark
@@@@@@@@
anchor -above 1..2 0
anchor +above 1..2 -1

glyph acute 2 1 mark
@@@@
anchor -above 0 0
anchor +above 0 -1

glyph acute:wide 2 1 mark
@@@@
anchor -above 0..1 0
anchor +above 0..1 -1

glyph stacked
ref i-compressed
ref dia
ref acute
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    let body = doc
        .items
        .iter()
        .find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "stacked" => Some(body),
            _ => None,
        })
        .unwrap();
    let derive = |refs: &[GlyphRef]| {
        derive_ref_offsets_with(
            &body.points,
            refs,
            |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
            |name| alt_idx.get(name).to_vec(),
            |name| resolved.get(name).map(|r| r.declared_anchors.clone()),
        )
    };

    // Offset-less: the narrow acute cannot consume dia's 2-cell +above, so
    // the 2-cell `acute:wide` is substituted; everything attaches, no issue.
    let (effective, _, issues) = derive(&body.refs);
    assert_eq!(effective[2].name, "acute:wide");
    assert!(issues.is_empty(), "{issues:?}");

    // The same refs pinned by explicit offsets: no substitution is possible,
    // and the near-miss is reported instead of passing in silence.
    let pinned: Vec<GlyphRef> = body
        .refs
        .iter()
        .enumerate()
        .map(|(i, r)| GlyphRef {
            offset: Some((0, i as i16)),
            ..r.clone()
        })
        .collect();
    let (effective, _, issues) = derive(&pinned);
    assert_eq!(
        effective[2].name, "acute",
        "explicit offsets never substitute"
    );
    assert!(
        issues.iter().any(|i| matches!(
            i,
            DeriveIssue::SizeMismatchedAttachment { ref_name, .. } if ref_name == "acute"
        )),
        "{issues:?}",
    );
}

/// Alternative selection also runs on the *publisher* side, by size: an
/// offset-less ref whose declared `+X` name-matches but size-mismatches a
/// sibling consumer's `-X` is substituted by an alternative whose `+X` fits.
/// This is the `enclosing-circle:alt` case — the letters cannot adapt (there
/// is no descender variant with a taller `-center`), so the circle must.
#[test]
fn publisher_alternative_is_selected_by_anchor_size() {
    let input = "\
glyph circle 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +center 2 1..2

glyph circle:alt
ref circle
anchor +center 2 1

glyph a-inner 2 2
@@@@
@@@@
anchor -center 1 0..1

glyph j-inner 2 2
@@@@
@@@@
anchor -center 1 0

glyph a-circled
ref circle
ref a-inner

glyph j-circled
ref circle
ref j-inner

glyph j-circled-reversed
ref j-inner
ref circle
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    let derive = |glyph: &str| {
        let body = doc
            .items
            .iter()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == glyph => Some(body),
                _ => None,
            })
            .unwrap();
        derive_ref_offsets_with(
            &body.points,
            &body.refs,
            |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
            |name| alt_idx.get(name).to_vec(),
            |name| resolved.get(name).map(|r| r.declared_anchors.clone()),
        )
    };

    // The 2-cell consumer matches the primary circle: no substitution.
    let (effective, _, issues) = derive("a-circled");
    assert_eq!(effective[0].name, "circle");
    assert!(issues.is_empty(), "{issues:?}");

    // The 1-cell consumer fits only circle:alt, whichever side comes first.
    for glyph in ["j-circled", "j-circled-reversed"] {
        let (effective, _, issues) = derive(glyph);
        let circle_ref = effective
            .iter()
            .find(|r| r.name.starts_with("circle"))
            .unwrap();
        assert_eq!(circle_ref.name, "circle:alt", "{glyph}");
        assert!(issues.is_empty(), "{glyph}: {issues:?}");
        // The consumer really attached: its offset aligns -center on (2, 1).
        let inner = effective
            .iter()
            .find(|r| r.name.ends_with("-inner"))
            .unwrap();
        assert_eq!(inner.offset, Some((1, 1)), "{glyph}");
    }
}
