//! Tests of composite layout: pattern refs, offsets and adjoin ordering, anchor forwarding, and contours.

use super::*;

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
    let mut cache: HashMap<String, ResolvedGlyph> = HashMap::default();
    cache.insert(
        "digit0".to_string(),
        ResolvedGlyph {
            grid: filled_grid(2, 2),
            origin_row: 0,
            origin_col: 0,
            resolved_anchors: Vec::new(),
            declared_anchors: Vec::new(),
            scale: 1,
            declared_box: None,
            declared_margin: Default::default(),
            declared_origin: (0, 0),
            inline_source: None,
        },
    );

    let refs = vec![GlyphRef {
        raw_name: None,
        comment: None,
        name: "digit(0|1)".to_string(),
        offset: None,
        negated: false,
        inherit: false,
        goto: false,
        fill: None,
        visibility: None,
    }];

    // compute_composite resolves the pattern ref via the shared layout's
    // fallback and includes the layer.
    let body = GlyphBody {
        refs: refs.clone(),
        ..GlyphBody::new()
    };
    let empty_parts = NamePartsMap::default();
    let composite = compute_composite(
        &body,
        &cache,
        &empty_parts,
        &AlternativesIndex::default(),
        &Default::default(),
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

/// An on-demand ref must draw from the frame it is typed in.
///
/// The editor's composite lookup reads the resolved-glyph map the derived
/// thread produces, and an on-demand name only enters that map when the whole
/// font is expanded again — a second or more over `font/`, behind a 1 s
/// debounce, and the request that would carry it is often queued behind the
/// resolve already running. Every other ref renders at once because its target
/// is already in the map; this one used to be the only kind that visibly
/// lagged. The lookup therefore synthesizes it here, from a map that does not
/// have it yet.
#[test]
fn on_demand_ref_composites_before_the_next_resolve() {
    let cache: HashMap<String, ResolvedGlyph> = HashMap::default();
    let refs = vec![GlyphRef {
        raw_name: None,
        comment: None,
        name: "4x2".to_string(),
        offset: None,
        negated: false,
        inherit: false,
        goto: false,
        fill: None,
        visibility: None,
    }];
    let body = GlyphBody {
        refs: refs.clone(),
        ..GlyphBody::new()
    };
    let empty_parts = NamePartsMap::default();
    let composite = compute_composite(
        &body,
        &cache,
        &empty_parts,
        &AlternativesIndex::default(),
        &Default::default(),
        &Default::default(),
    )
    .expect("has refs");
    assert_eq!(
        composite.layers.len(),
        1,
        "the on-demand ref should be a layer without waiting for a resolve"
    );
    assert_eq!((composite.width, composite.height), (4, 2));
    assert!(composite.any_layer_filled_at(0, 0));
    assert!(composite.any_layer_filled_at(1, 3));

    // The flattened form the thumbnails and the grid renderer share must agree.
    let grid = composite_to_grid(&None, &refs, &cache, &empty_parts, 1);
    assert!(grid.get(0, 0).is_bitmap_filled());
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
    let name_parts = NamePartsMap::default();
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
    let name_parts = NamePartsMap::default();

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
    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::default());
    assert_eq!(resolved["chain"].grid.width, 3);
    assert!(resolved["chain"].grid.get(0, 0).is_bitmap_filled());
    assert!(resolved["chain"].grid.get(0, 2).is_bitmap_filled());
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
                .is_some_and(|g| g.grid.get(0, 0).is_bitmap_filled()),
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
    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::default());

    let pf = resolved.get("combo-plus-first").unwrap();
    let mf = resolved.get("combo-minus-first").unwrap();
    assert_eq!(
        (pf.grid.width, pf.grid.height),
        (mf.grid.width, mf.grid.height),
        "ref order should not affect resolved dimensions"
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
    let (resolved, _) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::default());
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
    let (resolved, _) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::default());
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
