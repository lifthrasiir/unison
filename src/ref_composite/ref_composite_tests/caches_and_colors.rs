//! Tests of incremental re-composition, ref offsets to box corners, merged names, and colored refs.

use super::*;

/// A second resolve of an unchanged source recomposes nothing, and an edit
/// recomposes only what the edit reaches.
///
/// The editor resolves the whole source on every rebuild, and composing the
/// grids is essentially all of what that costs — 1.28 s of a 1.42 s run over
/// `font/`. Without the memo the pixel grid trailed the *built font* by more
/// than a second, because the font build has memoized its own composites
/// since `ContourCache` gained `composite_entries`; at a typing cadence
/// shorter than the resolve, each rebuild cancelled the last and the grid
/// never caught up at all. What is asserted here is the property that fixes
/// it: work proportional to the edit, not to the font.
#[test]
fn resolving_twice_recomposes_only_what_changed() {
    use crate::document_io;

    // Two source characters per pixel, as everywhere else in `.unf`.
    let source = |stem_pixels: &str| {
        format!(
            "\
glyph stem 2 2
{stem_pixels}

glyph bar 2 2
@@@@
....

glyph stem-bar 2 2
ref stem
ref bar

glyph double-bar 2 2
ref bar
ref bar
"
        )
    };
    let name_parts = NamePartsMap::default();
    let mut grid_cache = CompositeGridCache::default();
    let resolve = |text: &str, gc: &mut CompositeGridCache| {
        let doc = document_io::parse_document_from_str(text, "test.unf".into()).unwrap();
        let docs = vec![&doc];
        let expansion = crate::render::ttf_builder::expand_documents(&docs, &name_parts);
        let (resolved, _) = resolve_expansion_cached(
            expansion,
            &name_parts,
            &crate::cancel::CancelToken::never(),
            Some(gc),
        );
        resolved
    };

    let first = resolve(&source("@@..\n@@.."), &mut grid_cache);
    assert_eq!(
        grid_cache.stats(),
        (0, 2),
        "a cold cache composes both composites"
    );
    let before = first["stem-bar"].grid.clone();

    resolve(&source("@@..\n@@.."), &mut grid_cache);
    assert_eq!(
        grid_cache.stats(),
        (2, 0),
        "the same source composes nothing a second time"
    );

    // `stem` is what `stem-bar` draws over; `double-bar` never touches it.
    let after = resolve(&source("..@@\n..@@"), &mut grid_cache);
    assert_eq!(
        grid_cache.stats(),
        (1, 1),
        "only the composite whose ink changed is recomposed"
    );
    assert_ne!(
        after["stem-bar"].grid, before,
        "and it is recomposed to the new shape, not served stale"
    );
    assert_eq!(
        after["double-bar"].grid, first["double-bar"].grid,
        "the untouched composite is unchanged"
    );
}

/// A `ref` offset names the *target's declared box corner*, not its grid's.
///
/// This is what makes a glyph's box independent of the canvas around it: the
/// target can grow its grid to the left — to hold a hardblank, say — and
/// declare an origin that keeps its box where it was, and every glyph that
/// places it stays put.
///
/// The *parent's* origin deliberately does not enter. Everything inside a glyph
/// shares one coordinate system, its grid: its own pixels, its anchors and the
/// refs it places. The parent's origin says only where that grid sits relative
/// to the pen, which the output stage applies as a bearing. Letting it shift
/// the refs as well would move them relative to the glyph's own pixels, and a
/// composite that declares both a grid and refs would grow by exactly the
/// origin it declared.
#[test]
fn a_ref_offset_names_the_targets_box_corner() {
    let child = |origin: (i16, i16)| ResolvedGlyph {
        grid: filled_grid(1, 1),
        origin_row: 0,
        origin_col: 0,
        resolved_anchors: Vec::new(),
        declared_anchors: Vec::new(),
        scale: 1,
        declared_box: Some((1, 1)),
        declared_margin: Default::default(),
        declared_origin: origin,
        inline_source: None,
    };
    let one_ref = |offset: (i16, i16)| {
        vec![GlyphRef {
            raw_name: None,
            comment: None,
            name: "child".to_string(),
            offset: Some(offset),
            negated: false,
            inherit: false,
            goto: false,
            fill: None,
            visibility: None,
        }]
    };
    // Where the target's single ink cell lands in the parent's raster, as
    // (row, col), read off the layer rather than the flattened grid.
    let placed = |child_origin: (i16, i16), offset: (i16, i16)| {
        let mut cache: HashMap<String, ResolvedGlyph> = HashMap::default();
        cache.insert("child".to_string(), child(child_origin));
        let refs = one_ref(offset);
        let layout =
            resolve_composite_layout(None, &refs, &cache, &NamePartsMap::default(), 1, false);
        assert_eq!(layout.layers.len(), 1, "the ref must resolve");
        (layout.layers[0].raster_row, layout.layers[0].raster_col)
    };

    // No origin: the offset is the placement, exactly as before the box existed.
    assert_eq!(placed((0, 0), (2, 3)), (3, 2));

    // The target's box starts one cell into its own grid, so its grid — and the
    // ink in it — hangs one cell further out than the offset names.
    assert_eq!(placed((1, 0), (2, 3)), (3, 1));
    assert_eq!(placed((0, 1), (2, 3)), (2, 2));
}

/// A name merged into another of its block's expansions is still a name the
/// editor finds in the text, so resolution has to answer for it exactly as it
/// does for a declared alias — otherwise a perfectly good `ref a-j` is
/// underlined as undefined. See [`crate::merge`].
#[test]
fn a_merged_away_name_still_resolves() {
    use crate::document_io;

    let input = "\
glyph a-(g|j|k) 1 1
@@
glyph user 1 1
ref a-j
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (resolved, _alt_idx) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::default());
    for name in ["a-g", "a-j", "a-k", "user"] {
        assert!(resolved.contains_key(name), "{name} should resolve");
    }
    assert_eq!(
        resolved["a-j"].grid.width, resolved["a-g"].grid.width,
        "the merged-away name resolves to the glyph it is a name for",
    );
}

/// A `ref` to a glyph that is itself coloured draws in that glyph's colours,
/// and a `fill` claims every colour below it — the two rules the font build
/// follows, seen from the editor's live composite. The layer stays one layer;
/// what carries the colours is `cell_colors`.
// Both the colours and the walk that derives them are the editor's.
#[cfg(feature = "editor")]
#[test]
fn a_ref_to_a_colored_glyph_draws_its_colors() {
    use crate::document_io;

    let input = "\
color red = #ff0000
color blue = #0000ff
color green = #00ff00

glyph left 1 2
@@
@@

glyph right 1 2
@@
@@

glyph combo 2 2
ref left 0 0 fill red
ref right 1 0 fill blue

glyph outer 2 2
ref combo

glyph forced 2 2
ref outer fill green
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (resolved, alt_index) = resolve_named_glyphs_with_parts(&[&doc], &NamePartsMap::default());
    let aliases = crate::render::ttf_builder::collect_color_aliases(&[&doc]);
    let body_of = |name: &str| {
        doc.items
            .iter()
            .find_map(|item| match item {
                DocumentItem::Glyph {
                    name: glyph_name,
                    body,
                    ..
                } if glyph_name.0 == name => Some(body.clone()),
                _ => None,
            })
            .expect("glyph is declared")
    };
    let composite_of = |name: &str| {
        compute_composite(
            &body_of(name),
            &resolved,
            &NamePartsMap::default(),
            &alt_index,
            &aliases,
            &AnchorAligns::default(),
        )
        .expect("has refs")
    };
    let red = egui::Color32::from_rgba_unmultiplied(0xff, 0, 0, 0xff);
    let blue = egui::Color32::from_rgba_unmultiplied(0, 0, 0xff, 0xff);
    let green = egui::Color32::from_rgba_unmultiplied(0, 0xff, 0, 0xff);

    // The unfilled ref keeps both of its target's colours, cell by cell.
    let outer = composite_of("outer");
    assert_eq!(outer.layers.len(), 1, "one ref is still one layer");
    let layer = &outer.layers[0];
    assert_eq!(
        layer.fill_color, None,
        "the ref writes no colour of its own"
    );
    let colors = layer.cell_colors.as_ref().expect("colours travel up");
    let at = |r: u16, c: u16| colors[r as usize * layer.grid.width as usize + c as usize];
    assert_eq!(at(0, 0), Some(red), "the left half is its target's red");
    assert_eq!(at(0, 1), Some(blue), "the right half is its target's blue");

    // A `fill` two levels above claims all of it.
    let forced = composite_of("forced");
    let layer = &forced.layers[0];
    assert_eq!(layer.fill_color, Some(green));
    assert!(
        layer.cell_colors.is_none(),
        "a filled ref needs no per-cell colours: {:?}",
        layer.cell_colors
    );
}

/// A hardblank a `ref` lays exact geometry over must not swallow it: a
/// `PX_CUSTOM` cell is only half a cell — its region lives in the
/// grid's detail table — so handing back the shape code alone left the
/// flattened grid a cell that is not clear and yet draws nothing. The glyph's
/// own view paints its layers and so still showed the ink; every glyph that
/// referred to it saw the hole.
#[test]
fn a_hardblank_keeps_the_exact_geometry_a_ref_lays_over_it() {
    use crate::document_io;

    // `9x2-ys1` is an on-demand shear, so its top row is sub-pixel geometry.
    let hb = "$$".repeat(9);
    let input = format!(
        "glyph host 9 2\n{hb}\n{hb}\nref 9x2-ys1\n\nglyph parent 9 2\n{}\n{}\nref host\n",
        "..".repeat(9),
        "..".repeat(9),
    );
    let doc = document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
    let name_parts = NamePartsMap::default();
    let (resolved, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    for name in ["host", "parent"] {
        let grid = &resolved[name].grid;
        let mut drew = false;
        for r in 0..grid.height {
            for c in 0..grid.width {
                if grid.get(r, c).is_contour_empty() {
                    continue;
                }
                assert!(
                    !grid.region_at(r, c).is_empty(),
                    "{name} ({r},{c}) is {:?} with no geometry behind it",
                    grid.get(r, c)
                );
                drew = true;
            }
        }
        assert!(drew, "{name} draws the shear at all");
    }
}
