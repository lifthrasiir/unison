//! Tests of on-demand synthesized glyphs (fractional rects, color/mono) used as refs.

use super::*;

#[test]
fn on_demand_fractional_rect_resolved() {
    // 1p2r3x4 → scale 3, grid 6×12, rect (0,0)-(5,12)
    let doc = make_doc("glyph container\n  ref 1p2r3x4\n");
    let name_parts = NamePartsMap::default();
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
                resolved.grid.get(r, c).is_bitmap_filled(),
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
    let name_parts = NamePartsMap::default();
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
                resolved.grid.get(r, c).is_bitmap_filled(),
                r >= 3,
                "pixel ({r},{c}) fill={} expected={}",
                resolved.grid.get(r, c).is_bitmap_filled(),
                r >= 3,
            );
        }
    }
}

#[test]
fn on_demand_glyph_injected_for_ref() {
    let doc = make_doc("glyph test 3 5\n......\n......\n......\n......\n......\n  ref 2x3\n");
    let name_parts = NamePartsMap::default();
    let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    assert!(cache.contains_key("2x3"));
    let resolved = &cache["2x3"];
    assert_eq!(resolved.grid.width, 2);
    assert_eq!(resolved.grid.height, 3);
    for r in 0..3 {
        for c in 0..2 {
            assert!(resolved.grid.get(r, c).is_bitmap_filled());
        }
    }
}

#[test]
fn on_demand_glyph_composite_resolves() {
    let doc = make_doc("glyph composite\n  ref 3x2\n");
    let name_parts = NamePartsMap::default();
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
                comp.grid.get(r, c).is_bitmap_filled(),
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
    let name_parts = NamePartsMap::default();
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
    let name_parts = NamePartsMap::default();
    let (cache, _) = resolve_named_glyphs_with_parts(&[&doc], &name_parts);

    let resolved = &cache["2x3"];
    for r in 0..3 {
        for c in 0..2 {
            assert!(!resolved.grid.get(r, c).is_bitmap_filled());
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
    let name_parts = NamePartsMap::default();
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
    let name_parts = NamePartsMap::default();
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
    let name_parts = NamePartsMap::default();
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
