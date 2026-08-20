//! Assembly of the final font: the non-layout OpenType tables.
//!
//! # `gasp` is derived, not declared
//!
//! The one range this writes — every size, all four behaviour bits — follows
//! from how *this builder* draws, so it is not a `meta` key: any font Uniform
//! builds wants the same value, and a different one would simply be wrong.
//! `GRIDFIT` because [`super::hints`] emits instructions, which GDI runs only
//! when the table asks it to; `DOGRAY` because a PPEM that is not a multiple of
//! the font height cannot land on the pixel grid, and blurred is the acceptable
//! outcome there where bi-level nearest-neighbour is not. The symmetric bits say
//! the same two things to DirectWrite. Note what this cannot buy: horizontal
//! hinting runs only under GDI Classic (DirectWrite's symmetric modes and
//! FreeType's default interpreter both drop x-moves, and CoreText runs no
//! instructions at all), so `gasp` keeps the 16-PPEM hints reachable rather than
//! making them universal. Should a font ever want its bitmap face deliberately
//! aliased at large sizes, the key to add states *that intent* — never a raw
//! bitmask, which is a rasterizer detail with no place in `.unf`.

use super::gpos::{build_anchor_gpos, merge_anchor_feature_lookups};
use super::gsub::{build_gsub, compute_max_context};
use super::outlines::{
    GlobalBounds, OutlineBuild, add_color_layer_glyphs, build_glyph_outlines, compute_global_bounds,
};
use super::*;

/// `head.created`/`modified` when `meta created` says nothing.
///
/// A fixed value rather than the wall clock, so that building the same source
/// twice produces the same bytes — a build that differs only in its timestamp
/// makes every diff of a built font useless. 1904-01-01 is the epoch itself,
/// which reads as "unstated" rather than as a wrong date.
const DEFAULT_CREATED: i64 = 0;

/// OpenType tag from a string, right-padded with spaces to 4 bytes.
pub(super) fn make_tag(name: &str) -> Tag {
    let mut tag_arr = [b' '; 4];
    for (i, &b) in name.as_bytes().iter().enumerate().take(4) {
        tag_arr[i] = b;
    }
    Tag::new(&tag_arr)
}

/// The `for` target of a `feature` directive: an OpenType script tag,
/// optionally narrowed to one language system under it.
///
/// The two registries are separate and their tags never collide as bytes —
/// script tags are lowercase and language tags uppercase — but the one pair
/// that looks like a collision is inverted (`DFLT` is the default *script*,
/// `dflt` the default *language*), and a language tag means nothing without
/// the script it hangs under (`SRB` exists below both `latn` and `cyrl`).
/// So the two are written explicitly as `script/LANG` rather than guessed
/// from the tag.
pub(super) fn parse_script_lang(tok: &str) -> (String, Option<String>) {
    match tok.split_once('/') {
        Some((script, lang)) => (script.to_string(), Some(lang.to_string())),
        None => (tok.to_string(), None),
    }
}

/// Feature indices for one script: the default language system plus any
/// explicit ones.
#[derive(Default)]
pub(super) struct ScriptFeatures {
    /// Features of the default LangSys, i.e. what a bare `for latn` declares.
    pub(super) default: Vec<u16>,
    /// Features of each explicit LangSys, keyed by language tag.
    pub(super) langs: BTreeMap<String, Vec<u16>>,
}

impl ScriptFeatures {
    pub(super) fn push(&mut self, lang: Option<&str>, feat_idx: u16) {
        match lang {
            None => self.default.push(feat_idx),
            Some(l) => self.langs.entry(l.to_string()).or_default().push(feat_idx),
        }
    }
}

/// Script records, each with its default LangSys and one record per explicit
/// language system.
///
/// `langs` is expected to be complete: a shaper that resolves a language picks
/// that LangSys *instead of* the default and never merges the two, so whatever
/// the default carries has to be repeated there. `gsub::build_gsub` does that
/// at feature-tag level, before the records exist, so a language that redefines
/// a tag the default already has ends up with one merged record rather than two
/// records the shaper would have to choose between.
pub(super) fn build_script_records(
    script_features: &BTreeMap<String, ScriptFeatures>,
) -> Vec<ScriptRecord> {
    let mut script_records: Vec<ScriptRecord> = Vec::new();
    for (script_tag, features) in script_features {
        let lang_sys = LangSys {
            required_feature_index: 0xFFFF,
            feature_indices: features.default.clone(),
        };
        let lang_sys_records: Vec<LangSysRecord> = features
            .langs
            .iter()
            .map(|(lang_tag, feat_indices)| {
                LangSysRecord::new(
                    make_tag(lang_tag),
                    LangSys {
                        required_feature_index: 0xFFFF,
                        feature_indices: feat_indices.clone(),
                    },
                )
            })
            .collect();
        let script = Script::new(Some(lang_sys), lang_sys_records);
        script_records.push(ScriptRecord::new(make_tag(script_tag), script));
    }
    script_records
}

/// Add the cmap format 14 subtable for the face's variation sequences.
///
/// The split between the two arrays is decided here rather than stated in the
/// source, because it is a fact about the built face and not about the author's
/// intent: a pair whose target *is* what the base already maps to goes in the
/// Default UVS array, which carries no glyph id and says only "this sequence is
/// valid, use the base's glyph, and swallow the selector". Everything else
/// carries a glyph id in the Non-default array. A source that had to choose
/// would only ever get it wrong.
fn add_uvs_subtable(
    cmap: &mut Cmap,
    gsub_data: &GsubData,
    name_to_gid: &HashMap<String, GlyphId16>,
    cp_to_gid: &HashMap<u32, GlyphId16>,
) {
    use std::collections::BTreeSet;

    use write_fonts::tables::cmap::{
        Cmap14, DefaultUvs, EncodingRecord, NonDefaultUvs, PlatformId, UnicodeRange, UvsMapping,
        VariationSelector,
    };
    use write_fonts::types::Uint24;

    // selector → (default bases, non-default (base, gid) pairs), both ascending.
    let mut by_selector: BTreeMap<u32, (BTreeSet<u32>, BTreeMap<u32, GlyphId16>)> = BTreeMap::new();
    for pair in &gsub_data.uvs_pairs {
        let Some(&target) = name_to_gid.get(pair.glyph.as_str()) else {
            continue;
        };
        let entry = by_selector.entry(pair.selector).or_default();
        if cp_to_gid.get(&pair.base) == Some(&target) {
            entry.0.insert(pair.base);
        } else {
            entry.1.insert(pair.base, target);
        }
    }
    if by_selector.is_empty() {
        return;
    }

    // `length` is a stored field, not one the writer recomputes, but it cannot
    // be summed from what goes in either: two selectors naming the same pairs
    // share one array in the writer's object graph, so a sum over-counts. It is
    // read back off the serialized bytes instead (`fix_uvs_subtable_lengths`).
    let mut records = Vec::with_capacity(by_selector.len());
    for (selector, (defaults, non_defaults)) in by_selector {
        // Consecutive bases collapse into one range. `additional_count` is a
        // byte, so a run longer than 256 becomes several ranges.
        let mut ranges: Vec<UnicodeRange> = Vec::new();
        for cp in defaults {
            match ranges.last_mut() {
                Some(last)
                    if u32::from(last.start_unicode_value)
                        + u32::from(last.additional_count)
                        + 1
                        == cp
                        && last.additional_count < u8::MAX =>
                {
                    last.additional_count += 1;
                }
                _ => ranges.push(UnicodeRange::new(Uint24::checked_new(cp).unwrap(), 0)),
            }
        }

        let mappings: Vec<UvsMapping> = non_defaults
            .into_iter()
            .map(|(cp, gid)| UvsMapping::new(Uint24::checked_new(cp).unwrap(), gid.to_u16()))
            .collect();

        let default_uvs =
            (!ranges.is_empty()).then(|| DefaultUvs::new(ranges.len() as u32, ranges));
        let non_default_uvs =
            (!mappings.is_empty()).then(|| NonDefaultUvs::new(mappings.len() as u32, mappings));

        records.push(VariationSelector::new(
            Uint24::checked_new(selector).expect("a selector is well below 2^24"),
            default_uvs,
            non_default_uvs,
        ));
    }

    let subtable = Cmap14::new(0, records.len() as u32, records);
    cmap.encoding_records.push(EncodingRecord::new(
        PlatformId::Unicode,
        UNICODE_VARIATION_SEQUENCES_ENCODING,
        subtable.into(),
    ));
    // Encoding records are read in (platform, encoding) order, and format 14 is
    // (0, 5) — between the Unicode records `from_mappings` emitted and the
    // Windows ones.
    cmap.encoding_records
        .sort_by_key(|r| (r.platform_id as u16, r.encoding_id));
}

/// Unicode platform, "Unicode Variation Sequences" — the only encoding a format
/// 14 subtable may be listed under.
const UNICODE_VARIATION_SEQUENCES_ENCODING: u16 = 5;

/// Serialize the cmap, writing each format 14 subtable's real `length` over the
/// placeholder `add_uvs_subtable` left.
///
/// The extent is measured off the bytes rather than summed from the records,
/// because the writer shares two identical arrays as one object: a summed
/// length runs past the end of the table, which is what OTS rejects a
/// downloadable font for ("Over long cmap subtable").
fn dump_cmap(cmap: &Cmap) -> Vec<u8> {
    let mut bytes = write_fonts::dump_table(cmap).expect("cmap is valid");
    let be16 = |bytes: &[u8], at: usize| u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
    let be32 = |bytes: &[u8], at: usize| {
        u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
    };

    for i in 0..be16(&bytes, 2) {
        let subtable = be32(&bytes, 4 + 8 * i + 4);
        if be16(&bytes, subtable) != 14 {
            continue;
        }
        // A record names its two arrays by offset; several records may name the
        // same one, so the length is the farthest any of them reaches.
        let record_count = be32(&bytes, subtable + 6);
        let mut extent = 10 + 11 * record_count;
        for k in 0..record_count {
            let record = subtable + 10 + 11 * k;
            let default_uvs = be32(&bytes, record + 3);
            if default_uvs != 0 {
                extent = extent.max(default_uvs + 4 + 4 * be32(&bytes, subtable + default_uvs));
            }
            let non_default_uvs = be32(&bytes, record + 7);
            if non_default_uvs != 0 {
                extent =
                    extent.max(non_default_uvs + 4 + 5 * be32(&bytes, subtable + non_default_uvs));
            }
        }
        let extent = u32::try_from(extent).expect("a cmap subtable is far below 4 GiB");
        bytes[subtable + 2..subtable + 6].copy_from_slice(&extent.to_be_bytes());
    }
    bytes
}

// The font tables' inputs, gathered from unrelated stages.
#[expect(clippy::too_many_arguments)]
pub(super) fn build_ttf(
    ascender: i16,
    descender: i16,
    glyphs: &[CollectedGlyph],
    hint_ppem: u16,
    gsub_data: &GsubData,
    palette: &[Rgba],
    scale: f32,
    meta: &FontMeta,
) -> Vec<u8> {
    // `.notdef` is `glyphs[0]`, so the collected count is already the GID count.
    let mut num_glyphs = u16::try_from(glyphs.len()).expect("glyph count checked earlier");

    let default_aw = glyphs
        .iter()
        .find(|g| g.codepoints.contains(&0x20))
        .or(glyphs.first())
        .map(|g| g.advance_width)
        .unwrap_or(UNITS_PER_EM / 2);

    let mut outlines = build_glyph_outlines(glyphs, hint_ppem);
    let (colr_base_glyphs, colr_layers) =
        add_color_layer_glyphs(glyphs, &mut outlines, &mut num_glyphs);
    let has_color = !colr_base_glyphs.is_empty();
    let colr_layers_count = colr_layers.len() as u16;

    let OutlineBuild {
        glyf_builder,
        h_metrics,
        cmap_mappings,
        name_to_gid,
        max_points,
        max_contours,
        max_insn_size,
        max_stack,
        max_composite_points,
        max_composite_contours,
        max_component_elements,
        max_component_depth,
    } = outlines;

    let (glyf, loca, loca_format) = glyf_builder.build();

    let GlobalBounds {
        x_min,
        y_min,
        x_max,
        y_max,
        aw_max,
        min_lsb,
        min_rsb,
        x_max_extent,
    } = compute_global_bounds(glyphs, &h_metrics, default_aw, ascender, descender);

    // Every `meta` value on the pixel grid goes through the same scale the
    // outlines did, so a declaration reads in the units the source is drawn in.
    let px = |v: i16| (v as f32 * scale).round() as i16;
    let px_of = |key: crate::meta::PixelKey| meta.pixels(key).map(px);

    // head
    let head = Head {
        font_revision: Fixed::from_f64(meta.revision()),
        created: LongDateTime::new(meta.created.unwrap_or(DEFAULT_CREATED)),
        modified: LongDateTime::new(meta.created.unwrap_or(DEFAULT_CREATED)),
        mac_style: MacStyle::from_bits_truncate(meta.mac_style()),
        magic_number: 0x5F0F3CF5,
        // Not INSTRUCTIONS_MAY_ALTER_ADVANCE_WIDTH: the grid-snap hints in
        // `hints.rs` only `SHPIX` contour points and never touch a phantom
        // point, so the advance a rasterizer reads from `hmtx` always holds.
        // Claiming otherwise costs — Windows GDI honors the bit by dropping the
        // linear advance and, with no `LTSH`/`hdmx` here to consult, running
        // every glyph's hint program just to recover a number it already had.
        flags: Flags::BASELINE_AT_Y_0 | Flags::LSB_AT_X_0,
        units_per_em: UNITS_PER_EM,
        x_min,
        y_min,
        x_max,
        y_max,
        lowest_rec_ppem: 8,
        font_direction_hint: 2,
        index_to_loc_format: loca_format as i16,
        ..Default::default()
    };

    // hhea
    let hhea = Hhea {
        ascender: ascender.into(),
        descender: descender.into(),
        line_gap: px_of(crate::meta::PixelKey::LineGap).unwrap_or(0).into(),
        advance_width_max: aw_max.into(),
        min_left_side_bearing: min_lsb.into(),
        min_right_side_bearing: min_rsb.into(),
        x_max_extent: x_max_extent.into(),
        caret_slope_rise: meta.caret_slope.map_or(1, |(rise, _)| rise),
        caret_slope_run: meta.caret_slope.map_or(0, |(_, run)| run),
        caret_offset: px_of(crate::meta::PixelKey::CaretOffset).unwrap_or(0),
        number_of_h_metrics: num_glyphs,
    };

    // maxp
    let maxp = Maxp {
        num_glyphs,
        max_points: Some(max_points),
        max_contours: Some(max_contours),
        max_composite_points: Some(max_composite_points),
        max_composite_contours: Some(max_composite_contours),
        max_zones: Some(1),
        max_twilight_points: Some(0),
        max_storage: Some(0),
        max_function_defs: Some(0),
        max_instruction_defs: Some(0),
        max_stack_elements: Some(max_stack),
        max_size_of_instructions: Some(max_insn_size),
        max_component_elements: Some(max_component_elements),
        max_component_depth: Some(max_component_depth),
    };

    // hmtx
    let hmtx = Hmtx {
        h_metrics,
        left_side_bearings: Vec::new(),
    };

    // Codepoint → GID, which the variation-sequence stages need in both
    // directions the name map cannot answer: cmap 14 is keyed by codepoint, and
    // the fallback lookup's first element is the *base's* glyph.
    let cp_to_gid: HashMap<u32, GlyphId16> = cmap_mappings
        .iter()
        .filter_map(|(ch, gid)| {
            Some((
                *ch as u32,
                GlyphId16::new(u16::try_from(gid.to_u32()).ok()?),
            ))
        })
        .collect();

    // cmap
    // Cannot fail: `build_glyph_outlines` already resolved characters claimed by
    // more than one glyph, which is the only thing `from_mappings` rejects.
    let mut cmap =
        Cmap::from_mappings(cmap_mappings).expect("cmap mappings deduplicated by character");
    add_uvs_subtable(&mut cmap, gsub_data, &name_to_gid, &cp_to_gid);

    // name
    let name = build_name_table(meta);

    // os2
    // Over the real glyphs only: `.notdef` is a reserved slot, not a character
    // whose width belongs in the average.
    let real_glyphs = glyphs.split_first().map_or(&[][..], |(_, rest)| rest);
    let avg_width = if real_glyphs.is_empty() {
        default_aw as i16
    } else {
        let total: u32 = real_glyphs.iter().map(|g| g.advance_width as u32).sum();
        (total / real_glyphs.len() as u32) as i16
    };
    let first_cp = glyphs
        .iter()
        .flat_map(|g| &g.codepoints)
        .min()
        .copied()
        .unwrap_or(0x20);
    let last_cp = glyphs
        .iter()
        .flat_map(|g| &g.codepoints)
        .max()
        .copied()
        .unwrap_or(0x7E);

    let max_context = compute_max_context(gsub_data);

    // Coverage-derived, never declared: these describe the font that came out.
    let mapped: std::collections::HashSet<u32> =
        glyphs.iter().flat_map(|g| &g.codepoints).copied().collect();
    let unicode_ranges = super::os2_ranges::unicode_ranges(mapped.iter().copied());
    let code_pages = super::os2_ranges::code_page_ranges(&mapped);

    // Sub/superscript and strikeout keep the conventional proportions of the em
    // when undeclared, rather than being left at zero.
    let em_frac = |num: i32| ((UNITS_PER_EM as i32) * num / 100) as i16;
    let default_sub_sup = [em_frac(65), em_frac(70), 0, em_frac(20)];
    let sub = meta.subscript.map(|v| v.map(px)).unwrap_or(default_sub_sup);
    let sup = meta.superscript.map(|v| v.map(px)).unwrap_or([
        default_sub_sup[0],
        default_sub_sup[1],
        0,
        em_frac(48),
    ]);
    let strikeout = meta
        .strikeout
        .map(|(size, pos)| (px(size), px(pos)))
        .unwrap_or((UNITS_PER_EM as i16 / 20, ascender / 3));

    let os2 = Os2 {
        x_avg_char_width: avg_width,
        us_weight_class: meta.weight.unwrap_or(400),
        us_width_class: meta.width.unwrap_or(5),
        fs_type: meta.fs_type.unwrap_or(0),
        s_typo_ascender: ascender,
        s_typo_descender: descender,
        s_typo_line_gap: px_of(crate::meta::PixelKey::LineGap).unwrap_or(0),
        us_win_ascent: ascender as u16,
        us_win_descent: descender.unsigned_abs(),
        fs_selection: SelectionFlags::from_bits_truncate(meta.fs_selection()),
        us_first_char_index: first_cp.min(0xFFFF) as u16,
        us_last_char_index: last_cp.min(0xFFFF) as u16,
        ach_vend_id: vendor_tag(meta.vendor_id()),
        // bProportion (index 3) is 3 = Modern, never 9 = Monospaced: Windows
        // GDI acts on a PANOSE monospace claim by laying every glyph out on one
        // cell, which is wrong for any font whose advances are not all equal —
        // including one that declares `meta fixed-pitch` for the sake of
        // terminal font pickers. A font that wants the claim declares the whole
        // `meta panose`.
        panose_10: meta.panose.unwrap_or([2, 0, 5, 3, 0, 0, 0, 0, 0, 0]),
        sx_height: Some(px_of(crate::meta::PixelKey::XHeight).unwrap_or(ascender * 2 / 3)),
        s_cap_height: Some(px_of(crate::meta::PixelKey::CapHeight).unwrap_or(ascender)),
        y_subscript_x_size: sub[0],
        y_subscript_y_size: sub[1],
        y_subscript_x_offset: sub[2],
        y_subscript_y_offset: sub[3],
        y_superscript_x_size: sup[0],
        y_superscript_y_size: sup[1],
        y_superscript_x_offset: sup[2],
        y_superscript_y_offset: sup[3],
        y_strikeout_size: strikeout.0,
        y_strikeout_position: strikeout.1,
        us_default_char: Some(0),
        us_break_char: Some(0x20),
        us_max_context: Some(max_context),
        ul_unicode_range_1: unicode_ranges[0],
        ul_unicode_range_2: unicode_ranges[1],
        ul_unicode_range_3: unicode_ranges[2],
        ul_unicode_range_4: unicode_ranges[3],
        ul_code_page_range_1: Some(code_pages[0]),
        ul_code_page_range_2: Some(code_pages[1]),
        ..Default::default()
    };

    // post
    let post = Post {
        version: write_fonts::types::Version16Dot16::new(3, 0),
        underline_position: meta
            .underline
            .map_or(descender / 2, |(pos, _)| px(pos))
            .into(),
        underline_thickness: meta
            .underline
            .map_or(UNITS_PER_EM as i16 / 20, |(_, thick)| px(thick))
            .into(),
        is_fixed_pitch: u32::from(meta.has(crate::meta::StyleFlag::FixedPitch)),
        ..Default::default()
    };

    // gasp
    let gasp = Gasp {
        version: 1,
        num_ranges: 1,
        gasp_ranges: vec![GaspRange {
            range_max_ppem: u16::MAX,
            range_gasp_behavior: GaspRangeBehavior::GASP_GRIDFIT
                | GaspRangeBehavior::GASP_DOGRAY
                | GaspRangeBehavior::GASP_SYMMETRIC_GRIDFIT
                | GaspRangeBehavior::GASP_SYMMETRIC_SMOOTHING,
        }],
    };

    let mut gsub = build_gsub(gsub_data, &name_to_gid, &cp_to_gid);

    let mut anchor_data = build_anchor_gpos(glyphs, gsub_data, &name_to_gid, scale, meta.ascent());
    merge_anchor_feature_lookups(&mut gsub, std::mem::take(&mut anchor_data.feature_lookups));

    let mut builder = FontBuilder::new();
    builder
        .add_table(&head)
        .unwrap()
        .add_table(&gasp)
        .unwrap()
        .add_table(&hhea)
        .unwrap()
        .add_table(&maxp)
        .unwrap()
        .add_table(&hmtx)
        .unwrap()
        .add_raw(Tag::new(b"cmap"), dump_cmap(&cmap))
        .add_table(&name)
        .unwrap()
        .add_table(&os2)
        .unwrap()
        .add_table(&post)
        .unwrap()
        .add_table(&glyf)
        .unwrap()
        .add_table(&loca)
        .unwrap();

    let has_gsub = gsub.is_some();
    let has_gpos = anchor_data.gpos.is_some();

    if let Some(ref gsub) = gsub {
        builder.add_table(gsub).unwrap();
    }

    if let Some(ref gpos) = anchor_data.gpos {
        builder.add_table(gpos).unwrap();
    }

    if has_gsub || has_gpos {
        let mut gdef = anchor_data.gdef;
        if !anchor_data.mark_glyph_sets.is_empty() {
            gdef.mark_glyph_sets_def = Some(MarkGlyphSets::new(anchor_data.mark_glyph_sets)).into();
        }
        builder.add_table(&gdef).unwrap();
    }

    if has_color {
        let colr = Colr::new(
            colr_base_glyphs.len() as u16,
            Some(colr_base_glyphs),
            Some(colr_layers),
            colr_layers_count,
        );
        builder.add_table(&colr).unwrap();

        let mut color_records: Vec<ColorRecord> = palette
            .iter()
            .map(|c| ColorRecord::new(c.b, c.g, c.r, c.a))
            .collect();
        // CPAL requires at least one palette entry. Every layer at the `fg`
        // index (0xFFFF) — e.g. when the only fill's color alias never
        // resolved — would otherwise write an empty, invalid CPAL.
        if color_records.is_empty() {
            color_records.push(ColorRecord::new(0, 0, 0, 255));
        }
        let num_entries = color_records.len() as u16;
        let cpal = Cpal::new(num_entries, 1, num_entries, Some(color_records), vec![0]);
        builder.add_table(&cpal).unwrap();
    }

    builder.build()
}

/// `OS/2.achVendID` is a 4-byte tag; `meta` bounds the value to 1-4 printable
/// ASCII characters, so padding is all that is left to do here.
fn vendor_tag(id: &str) -> Tag {
    let mut bytes = [b' '; 4];
    for (i, &b) in id.as_bytes().iter().enumerate().take(4) {
        bytes[i] = b;
    }
    Tag::new(&bytes)
}

/// The name table, from what `meta` declares plus what [`FontMeta::name_records`]
/// derives.
///
/// Windows platform records only (platform 3, encoding 1). Platform 1 (Mac)
/// records are optional in practice and carry no language slot worth having,
/// and platform 0 records cannot be localized at all — so a `@LANG` on a `meta`
/// line would have nowhere to go outside platform 3.
fn build_name_table(meta: &FontMeta) -> Name {
    let records: Vec<NameRecord> = meta
        .name_records()
        .into_iter()
        .map(|(name_id, language_id, text)| {
            NameRecord::new(
                3, // Windows
                1, // Unicode BMP
                language_id,
                NameId::new(name_id),
                text.into(),
            )
        })
        .collect();

    Name::new(records)
}

pub(super) fn glyph_bounds(contours: &[Vec<(i16, i16)>]) -> (i16, i16, i16, i16) {
    let mut x_min = i16::MAX;
    let mut y_min = i16::MAX;
    let mut x_max = i16::MIN;
    let mut y_max = i16::MIN;
    for c in contours {
        for &(x, y) in c {
            x_min = x_min.min(x);
            y_min = y_min.min(y);
            x_max = x_max.max(x);
            y_max = y_max.max(y);
        }
    }
    if x_min > x_max {
        return (0, 0, 0, 0);
    }
    (x_min, y_min, x_max, y_max)
}
