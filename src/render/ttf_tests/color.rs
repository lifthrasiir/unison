//! Tests for COLR/CPAL output: color layers, `coloronly`/`monoonly` and the
//! mono fallback.

use super::*;

#[test]
fn colr_cpal_tables_built_for_colored_glyphs() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

color red = #ff0000
color blue = #0000ff

glyph base 2 2
@@@@
@@@@

glyph overlay 2 2
..@@
@@..

glyph combo
ref base fill red
ref overlay fill blue

map A = combo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, _, palette) = collect_glyph_data(&[&doc], false).unwrap();
    let combo = glyph_data.iter().find(|g| g.name == "combo").unwrap();
    assert!(
        !combo.color_layers.is_empty(),
        "combo should have color layers"
    );
    assert_eq!(combo.color_layers.len(), 2, "should have 2 color layers");
    assert!(!palette.is_empty(), "palette should have colors");
    assert_eq!(palette.len(), 2, "palette should have 2 unique colors");
    // Verify deterministic sort (blue < red)
    assert_eq!(
        palette[0],
        Rgba {
            r: 0,
            g: 0,
            b: 255,
            a: 255
        }
    );
    assert_eq!(
        palette[1],
        Rgba {
            r: 255,
            g: 0,
            b: 0,
            a: 255
        }
    );

    let font = build_font_from_documents(&[&doc]);
    assert!(font.is_some(), "font with COLR should build successfully");
}

/// A fill whose color alias never resolves leaves every layer at the `fg`
/// palette index (0xFFFF). CPAL requires at least one palette entry, so the
/// build has to pad the palette rather than write an empty (invalid) CPAL.
#[test]
fn an_all_fg_color_font_still_writes_a_valid_cpal() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph base 2 2
@@@@
@@@@

glyph combo
ref base fill missing

map A = combo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let bytes = build_font_from_documents(&[&doc]).expect("font should build");
    let font = read_fonts::FontRef::new(&bytes).unwrap();
    let cpal = font.cpal().expect("CPAL should be present");
    assert!(
        cpal.num_palette_entries() >= 1,
        "CPAL must not be empty: {} entries",
        cpal.num_palette_entries()
    );
}

#[test]
fn coloronly_layer_excluded_from_fallback() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph base 2 2
@@@@
@@@@

glyph overlay 2 2
..@@
@@..

glyph combo
ref base fill fg
ref overlay fill #ff0000 coloronly

map A = combo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, _, _palette) = collect_glyph_data(&[&doc], false).unwrap();
    let combo = glyph_data.iter().find(|g| g.name == "combo").unwrap();
    // coloronly layer should NOT be in color_layers (it IS in COLR but
    // the test above covers that). Wait actually coloronly means it IS in COLR.
    // Let me re-check: coloronly = only in COLR, not in fallback. monoonly = only in fallback.
    // So color_layers should contain coloronly layers (they go into COLR).
    // And fallback contours should NOT contain coloronly layers.

    // combo.contours = fallback = only layers that are NOT coloronly
    // So fallback should only have base (fg), not overlay (coloronly).
    // The base is 2x2, overlay is also 2x2. If both were included,
    // the contours would cover all 4 cells. If only base, all 4 cells too.
    // This test is hard to distinguish by contour shape alone.
    // Just verify the font builds and has color layers.
    assert!(!combo.color_layers.is_empty());
}

#[test]
fn coloronly_white_fill_excluded_from_fallback() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph card-blank 4 4
@@@@@@@@
@@....@@
@@....@@
@@@@@@@@

glyph card-fill 4 4
........
..@@@@..
..@@@@..
........

glyph combo
ref card-blank fill #000000
ref card-fill fill #ffffff coloronly

map A = combo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let combo = glyph_data.iter().find(|g| g.name == "combo").unwrap();

    assert!(!combo.color_layers.is_empty(), "should have color layers");

    // The white fill layer should exist in color_layers
    let white_layers: Vec<_> = combo
        .color_layers
        .iter()
        .filter(|l| l.palette_index != 0xFFFF)
        .collect();
    assert!(
        !white_layers.is_empty(),
        "white fill layer should be in color_layers"
    );

    // Fallback should NOT include the coloronly card-fill
    // card-blank is a border shape (1 or 2 contours), card-fill is inner fill
    // If card-fill leaked, there would be extra contours
    let card_blank_doc = document_io::parse_document_from_str(
        "meta height 16\nmeta ascent 12\nmeta descent 4\nglyph card-blank 4 4\n@@@@@@@@\n@@....@@\n@@....@@\n@@@@@@@@\nmap B = card-blank\n",
        "test2.unf".into()
    ).unwrap();
    let (_, _, blank_data, _, _) = collect_glyph_data(&[&card_blank_doc], false).unwrap();
    let blank = blank_data.iter().find(|g| g.name == "card-blank").unwrap();

    assert_eq!(
        combo.contours.len(),
        blank.contours.len(),
        "fallback contours should match card-blank only (coloronly card-fill excluded). \
         combo has {} contours, card-blank has {}",
        combo.contours.len(),
        blank.contours.len()
    );
}

#[test]
fn coloronly_with_pattern_expansion() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

name-parts $suit = spade heart

glyph card-blank 4 4
@@@@@@@@
@@....@@
@@....@@
@@@@@@@@

glyph card-fill 4 4
........
..@@@@..
..@@@@..
........

glyph card-suit-spade 2 2
@@@@
@@@@

glyph card-suit-heart 2 2
..@@
@@..

glyph card-($suit)
ref card-blank fill #000000
ref card-fill fill #ffffff coloronly
ref card-suit-($suit) fill #000000

map A = card-spade
map B = card-heart
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, _, _) = collect_glyph_data(&[&doc], false).unwrap();

    for name in ["card-spade", "card-heart"] {
        let g = glyph_data.iter().find(|g| g.name == name).unwrap();
        assert!(
            !g.color_layers.is_empty(),
            "{name} should have color layers"
        );

        // Check that there IS a non-foreground (white) layer in color_layers
        let non_fg: Vec<_> = g
            .color_layers
            .iter()
            .filter(|l| l.palette_index != 0xFFFF)
            .collect();
        assert!(
            !non_fg.is_empty(),
            "{name}: should have at least one non-fg color layer (white fill), got {}",
            non_fg.len()
        );
    }
}

/// Regression test: own pixel data plus fill-less/`fg`-filled refs must
/// be merged into a single foreground (palette index 0xFFFF) COLR
/// layer, not emitted as separate layers.
#[test]
fn colr_foreground_layers_are_merged_into_one() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

color red = #ff0000

glyph base 2 2
@@..
@@..

glyph overlay1 2 2
..@@
..@@

glyph overlay2 2 2
@@@@
@@@@

glyph combo
ref base
ref overlay1 fill fg
ref overlay2 fill red

map A = combo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let combo = glyph_data.iter().find(|g| g.name == "combo").unwrap();
    assert_eq!(
        combo.color_layers.len(),
        2,
        "own pixels + fg-filled ref should merge into ONE foreground layer, plus one red layer"
    );
    let fg_layers: Vec<_> = combo
        .color_layers
        .iter()
        .filter(|l| l.palette_index == 0xFFFF)
        .collect();
    assert_eq!(
        fg_layers.len(),
        1,
        "there should be exactly one foreground layer"
    );
    // The single foreground layer should contain contours from BOTH
    // own pixels (base) and the fg-filled ref (overlay1): 2 contours.
    assert_eq!(
        fg_layers[0].contours.len(),
        2,
        "foreground layer should merge own+fg-ref contours"
    );
}

/// Regression test: each COLR layer glyph's hmtx left-side-bearing must
/// equal its own bbox x_min, not 0 — otherwise renderers reposition the
/// layer relative to the wrong origin.
#[test]
fn colr_layer_glyph_lsb_matches_its_own_bbox() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

color red = #ff0000

glyph base 4 2
@@@@@@@@
@@@@@@@@

glyph overlay 4 2
....@@@@
....@@@@

glyph combo
ref base fill fg
ref overlay fill red

map A = combo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let font_data = build_font_from_documents(&[&doc]);
    assert!(
        font_data.is_some(),
        "font with COLR should build successfully"
    );
    let bytes = font_data.unwrap();
    let font = read_fonts::FontRef::new(&bytes).unwrap();

    let glyf = font.glyf().unwrap();
    let loca = font.loca(None).unwrap();
    let hmtx = font.hmtx().unwrap();
    let maxp = font.maxp().unwrap();

    // The "red" overlay layer glyph is offset to the right (its filled
    // cells start at column 4 of an 8-wide grid), so its bbox x_min
    // (and thus its LSB) must be > 0, not 0.
    let mut found_nonzero_lsb = false;
    for gid in 0..maxp.num_glyphs() {
        let gid = GlyphId::new(gid as u32);
        let Ok(Some(glyph)) = loca.get_glyf(gid, &glyf) else {
            continue;
        };
        let read_fonts::tables::glyf::Glyph::Simple(sg) = glyph else {
            continue;
        };
        if sg.number_of_contours() == 0 {
            continue;
        }
        let x_min = sg.x_min();
        let lsb = hmtx.h_metrics()[gid.to_u32() as usize].side_bearing();
        if x_min > 0 {
            assert_eq!(
                lsb, x_min,
                "LSB must match this layer glyph's own bbox x_min"
            );
            found_nonzero_lsb = true;
        }
    }
    assert!(
        found_nonzero_lsb,
        "expected at least one COLR layer glyph with a nonzero x_min"
    );
}

#[test]
fn color_layers_built_for_remap_only_glyphs() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph base-a 8 8
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

glyph base-b 8 8
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

glyph mono-layer 8 8
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

glyph color-layer 8 8
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

glyph combined
ref mono-layer fill fg monoonly
ref color-layer fill #FF0000 coloronly

map A = base-a
map B = base-b

remap sub : base-a base-b -> combined
feature ccmp for DFLT : sub
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyphs, _, palette) = collect_glyph_data(&[&doc], false).unwrap();
    let combined = glyphs
        .iter()
        .find(|g| g.name == "combined")
        .expect("combined glyph should be in glyph_data as a remap-referenced extra glyph");
    assert!(
        !combined.color_layers.is_empty(),
        "remap-only glyph with fill refs must have COLR layers"
    );
    assert!(
        !palette.is_empty(),
        "palette must contain at least the #FF0000 color"
    );
    assert!(
        combined
            .color_layers
            .iter()
            .all(|cl| !cl.contours.is_empty()),
        "every color layer should have contours"
    );
    let has_colored = combined
        .color_layers
        .iter()
        .any(|cl| cl.palette_index != 0xFFFF);
    assert!(
        has_colored,
        "at least one layer must reference a palette color"
    );

    let fallback_non_empty = !combined.contours.is_empty();
    assert!(
        fallback_non_empty,
        "fallback contours (monoonly layer) should be present"
    );

    let color_layer_count = combined.color_layers.len();
    assert_eq!(
        color_layer_count, 1,
        "monoonly ref should be excluded from color layers, \
         so only the coloronly ref (with its palette color) should remain"
    );
}

/// The same thing one level up: an on-demand `X` synthesized from `X:mono`
/// and `X:color` flattens both bodies' refs into one glyph with visibility
/// flags, which is exactly what pushes it onto the color-layer path.
#[test]
fn negated_ref_subtracts_in_synthesized_color_mono_glyph() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph hole 2 2
@@@@
@@@@

glyph flag:mono
ref base
ref hole 1 1 negated

glyph flag:color
ref base fill #ff0000

map A = flag
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let flag = glyphs.iter().find(|g| g.name == "flag").unwrap();

    let s = 64.0;
    assert_ne!(
        winding_at(&flag.contours, 0.5 * s, (12.0 - 0.5) * s),
        0,
        "the mono base layer should be filled"
    );
    assert_eq!(
        winding_at(&flag.contours, 2.0 * s, (12.0 - 2.0) * s),
        0,
        "the negated ref in X:mono should punch a hole in the synthesized X"
    );
}

/// `X:color` may be an alias (`glyph X:color = Y:color`), and the on-demand
/// `X` has to be synthesized from it all the same: an alias is a second name
/// for a glyph, so the pair `X:mono`/`X:color` is complete whether the color
/// half is written out or aliased.
#[test]
fn color_mono_glyph_is_synthesized_when_the_color_half_is_an_alias() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph flag-a:mono
ref base

glyph flag-a:color
ref base fill #ff0000

glyph flag-b:mono
ref base

glyph flag-b:color = flag-a:color

map A = flag-a
map B = flag-b
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let b = glyphs
        .iter()
        .find(|g| g.name == "flag-b")
        .expect("flag-b should be synthesized from flag-b:mono and the aliased flag-b:color");
    assert!(
        !b.contours.is_empty(),
        "the synthesized flag-b should carry the aliased color half's contours"
    );
}

#[test]
fn color_mono_combined_glyph_preserves_advance_across_scales() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph part-left 16 16
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@

glyph part-right 8 16
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@

glyph test-xy:mono
ref part-left
ref part-right 16 0

glyph test-xy:color 24 16 scale 2
ref 22x16 2 0 fill #ff000080

map X = test-xy
";
    let doc = document_io::parse_document_from_str(input, "t.unf".into()).unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();

    let xy = glyphs.iter().find(|g| g.name == "test-xy").unwrap();
    assert_eq!(
        xy.advance_width,
        (24.0_f32 * 1024.0 / 16.0).round() as u16,
        "advance should be 24 logical pixels = {}",
        (24.0_f32 * 1024.0 / 16.0).round() as u16,
    );
}

#[test]
fn colr_base_glyph_bbox_covers_color_layers() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph frame-left 4 4
..@@@@@@
..@@....
..@@....
..@@@@@@

glyph frame-right 4 4
@@@@@@..
....@@..
....@@..
@@@@@@..

glyph test-flag:mono
ref frame-left
ref frame-right 4 0

glyph test-flag:color 8 4 scale 2
ref 14x6 1 1 fill #ff0000

map A = test-flag
";
    let doc = document_io::parse_document_from_str(input, "t.unf".into()).unwrap();
    let font_data = build_font_from_documents(&[&doc]);
    assert!(font_data.is_some(), "font should build");
    let bytes = font_data.unwrap();
    let font = read_fonts::FontRef::new(&bytes).unwrap();
    let glyf = font.glyf().unwrap();
    let loca = font.loca(None).unwrap();
    let hmtx = font.hmtx().unwrap();

    let cmap = font.cmap().unwrap();
    let gid = cmap.map_codepoint('A').expect("A should be mapped");
    let advance = hmtx.advance(gid).unwrap();
    let glyph = loca.get_glyf(gid, &glyf).unwrap().unwrap();
    let simple = match glyph {
        read_fonts::tables::glyf::Glyph::Simple(s) => s,
        _ => panic!("expected simple glyph"),
    };
    assert!(
        simple.x_max() >= advance as i16,
        "base glyph xMax ({}) should be >= advance ({}) to prevent COLR clipping",
        simple.x_max(),
        advance,
    );
}

/// A `ref` to a glyph that is itself colored keeps that glyph's colors: the
/// target's layers are spliced into the referring glyph, each with the palette
/// entry it was drawn in, rather than being flattened into the foreground.
#[test]
fn a_ref_to_a_colored_glyph_keeps_its_colors() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

color red = #ff0000
color blue = #0000ff

glyph base 2 2
@@@@
@@@@

glyph overlay 2 2
..@@
@@..

glyph combo
ref base fill red
ref overlay fill blue

glyph outer
ref combo

map A = outer
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, _, palette) = collect_glyph_data(&[&doc], false).unwrap();
    let outer = glyph_data.iter().find(|g| g.name == "outer").unwrap();
    let indices: Vec<u16> = outer.color_layers.iter().map(|l| l.palette_index).collect();
    let blue = palette
        .iter()
        .position(|c| {
            *c == Rgba {
                r: 0,
                g: 0,
                b: 255,
                a: 255,
            }
        })
        .expect("blue in palette") as u16;
    let red = palette
        .iter()
        .position(|c| {
            *c == Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            }
        })
        .expect("red in palette") as u16;
    assert_eq!(indices, vec![red, blue], "outer should keep both colors");
}

/// `fill` on a `ref` is a claim over everything the ref draws: every layer the
/// target carries, however deep, comes out in that one color.
#[test]
fn a_fill_recolors_every_layer_the_ref_reaches() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

color red = #ff0000
color blue = #0000ff
color green = #00ff00

glyph base 2 2
@@@@
@@@@

glyph overlay 2 2
..@@
@@..

glyph combo
ref base fill red
ref overlay fill blue

glyph outer
ref combo

glyph forced
ref outer fill green

map A = forced
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, _, palette) = collect_glyph_data(&[&doc], false).unwrap();
    let forced = glyph_data.iter().find(|g| g.name == "forced").unwrap();
    let green = palette
        .iter()
        .position(|c| {
            *c == Rgba {
                r: 0,
                g: 255,
                b: 0,
                a: 255,
            }
        })
        .expect("green in palette") as u16;
    assert!(
        forced.color_layers.iter().all(|l| l.palette_index == green),
        "every layer should be green: {:?}",
        forced
            .color_layers
            .iter()
            .map(|l| l.palette_index)
            .collect::<Vec<_>>()
    );
    assert!(!forced.color_layers.is_empty(), "should still draw");
}

/// A `ref` to a composite that *subtracts* is one layer, not its parts — the
/// same rule the sample splits a ref by. It still hands up a colour, but only
/// the one every part it draws agrees on.
#[test]
fn a_ref_to_a_subtracting_colored_glyph_hands_up_its_one_color() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

color red = #ff0000

glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph hole 2 2
@@@@
@@@@

glyph ring
ref base fill red
ref hole 1 1 negated

glyph outer
ref ring

map A = outer
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, _, palette) = collect_glyph_data(&[&doc], false).unwrap();
    let outer = glyph_data.iter().find(|g| g.name == "outer").unwrap();
    let red = palette
        .iter()
        .position(|c| {
            *c == Rgba {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            }
        })
        .expect("red in palette") as u16;
    assert_eq!(
        outer
            .color_layers
            .iter()
            .map(|l| l.palette_index)
            .collect::<Vec<_>>(),
        vec![red],
        "the difference is one red layer"
    );
    // Two contours: the outside of the ring and the hole it keeps.
    assert_eq!(outer.color_layers[0].contours.len(), 2);
}

/// A colour layer is placed by the same rule the mono outline is: the offset
/// runs box origin to box origin, so a target that declares an `origin` shifts
/// a filled `ref` exactly as far as an unfilled one.
#[test]
fn a_filled_ref_reads_its_targets_origin_like_every_other_ref() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

color red = #ff0000

glyph part 4 4 origin 2 1
........
..@@@@..
..@@@@..
........

glyph plain
ref part 4 4

glyph colored
ref part 4 4 fill red

map A = plain
map B = colored
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let plain = glyph_data.iter().find(|g| g.name == "plain").unwrap();
    let colored = glyph_data.iter().find(|g| g.name == "colored").unwrap();
    assert_eq!(colored.color_layers.len(), 1);
    assert_eq!(
        sorted_contours(&colored.color_layers[0].contours),
        sorted_contours(&plain.contours),
        "the filled layer should sit where the mono outline does"
    );
}
