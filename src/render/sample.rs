use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as FmtWrite;
use std::io::{self, Write};
use std::path::Path;

use crate::document::*;
use crate::pixel::PX_SUBPIXEL;
use crate::render::contour::{track_contour, track_contour_fullpixel, track_contour_multi, track_contour_multi_diff};
use crate::render::ttf_builder::{
    ColorAliasMap, Rgba, collect_color_aliases, effective_visibility, expand_map_pairs,
    resolve_fill_rgba,
};

#[derive(Clone)]
struct SampleComponent {
    row: i32,
    col: i32,
    grid: PixelGrid,
    negated: bool,
    fill_rgba: Option<Rgba>,
    visibility: LayerVisibility,
}

struct SampleGlyph {
    width: u16,
    _height: u16,
    components: Vec<SampleComponent>,
    left: i16,
    top: i16,
}

struct SampleData {
    height: u16,
    #[allow(dead_code)]
    ascent: u16,
    #[allow(dead_code)]
    descent: u16,
    /// codepoint → glyph name, sorted by codepoint
    cmap: BTreeMap<u32, String>,
    /// glyph name → sample glyph data
    glyphs: HashMap<String, SampleGlyph>,
    /// codepoints excluded from sample display
    excluded: BTreeSet<u32>,
    /// OpenType feature tags
    features: Vec<String>,
}

fn collect_sample_data(docs: &[&Document]) -> Option<SampleData> {
    if docs.is_empty() {
        return None;
    }

    let mut height: u16 = 16;
    let mut ascent: u16 = 14;
    let mut descent: u16 = 2;
    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::FontMeta(s) = item {
                for pair in s.split_whitespace().collect::<Vec<_>>().chunks(2) {
                    if pair.len() == 2
                        && let Ok(v) = pair[1].parse::<u16>() {
                            match pair[0] {
                                "height" => height = v,
                                "ascent" => ascent = v,
                                "descent" => descent = v,
                                _ => {}
                            }
                        }
                }
            }
        }
    }
    if height == 0 {
        return None;
    }

    let name_parts = collect_name_parts(docs);
    let color_aliases = collect_color_aliases(docs);

    let all_items =
        crate::render::ttf_builder::collect_expanded_items(docs, &name_parts);

    // Build contour cache for named glyphs
    struct CachedGlyph {
        width: u16,
        height: u16,
        contours: Vec<Vec<(f32, f32)>>,
        anchors: Vec<GlyphPoint>,
        grid: Option<PixelGrid>,
        components: Vec<SampleComponent>,
    }

    impl CachedGlyph {
        fn from_grid(grid: &PixelGrid) -> Self {
            let contours = track_contour(grid, PX_SUBPIXEL);
            Self {
                width: grid.width,
                height: grid.height,
                contours,
                anchors: Vec::new(),
                grid: Some(grid.clone()),
                components: vec![SampleComponent {
                    row: 0,
                    col: 0,
                    grid: grid.clone(),
                    negated: false,
                    fill_rgba: None,
                    visibility: LayerVisibility::Both,
                }],
            }
        }
    }

    fn resolve_cached_ref<'a>(
        name: &str,
        cache: &'a HashMap<String, CachedGlyph>,
    ) -> Option<&'a CachedGlyph> {
        if let Some(cached) = cache.get(name) {
            return Some(cached);
        }
        let expanded = crate::ref_composite::expand_ref_names(name)?;
        cache.get(expanded.first()?)
    }

    fn build_cached_alternatives(
        cache: &HashMap<String, CachedGlyph>,
    ) -> HashMap<String, Vec<(String, Vec<GlyphPoint>)>> {
        let mut map: HashMap<String, Vec<(String, Vec<GlyphPoint>)>> = HashMap::new();
        for (name, cached) in cache {
            let mut prefix = name.as_str();
            while let Some(colon_pos) = prefix.rfind(':') {
                prefix = &prefix[..colon_pos];
                map.entry(prefix.to_string())
                    .or_default()
                    .push((name.clone(), cached.anchors.clone()));
            }
        }
        for alts in map.values_mut() {
            alts.sort_by(|(a, _), (b, _)| a.cmp(b));
        }
        map
    }

    let mut cache: HashMap<String, CachedGlyph> = HashMap::new();

    struct PendingGlyph {
        name: String,
        pixels: Option<PixelGrid>,
        refs: Vec<GlyphRef>,
        points: Vec<GlyphPoint>,
    }
    let mut pending: Vec<PendingGlyph> = Vec::new();

    let mut glyph_declared_anchors: HashMap<String, Vec<GlyphPoint>> = HashMap::new();
    let mut glyph_offsets: HashMap<String, (i16, i16)> = HashMap::new();
    for item in &all_items {
        if let DocumentItem::Glyph { name: GlyphName(n), body } = item {
            glyph_declared_anchors.entry(n.clone()).or_insert_with(|| body.points.clone());
            if body.left.is_some() || body.top.is_some() {
                glyph_offsets.entry(n.clone())
                    .or_insert((body.left.unwrap_or(0), body.top.unwrap_or(0)));
            }
        }
    }

    for item in &all_items {
        let (cache_key, body) = match item {
            DocumentItem::Glyph { name: GlyphName(n), body } => (n.clone(), body),
            _ => continue,
        };
        if !cache_key.is_empty() && !cache.contains_key(&cache_key) {
            if let Some(ref pixels) = body.pixels && body.refs.is_empty() {
                let mut cached = CachedGlyph::from_grid(pixels);
                cached.anchors = body.points.clone();
                cache.insert(cache_key, cached);
            } else if body.pixels.is_some() || !body.refs.is_empty() {
                pending.push(PendingGlyph {
                    name: cache_key,
                    pixels: body.pixels.clone(),
                    refs: body.refs.clone(),
                    points: body.points.clone(),
                });
            } else if body.sticky {
                cache.insert(
                    cache_key,
                    CachedGlyph {
                        width: 0,
                        height: 0,
                        contours: Vec::new(),
                        anchors: body.points.clone(),
                        grid: None,
                        components: Vec::new(),
                    },
                );
            }
        }
    }

    // Resolve refs iteratively
    let mut progress = true;
    while progress {
        progress = false;
        let alt_index = build_cached_alternatives(&cache);
        let mut i = 0;
        while i < pending.len() {
            if !pending[i]
                .refs
                .iter()
                .all(|gref| resolve_cached_ref(&gref.name, &cache).is_some())
            {
                i += 1;
                continue;
            }
            let pg = pending.swap_remove(i);
            let (effective_refs, anchors) =
                crate::ref_composite::derive_ref_offsets_with(
                    &pg.points,
                    &pg.refs,
                    |name| {
                        resolve_cached_ref(name, &cache)
                            .map(|resolved| resolved.anchors.clone())
                    },
                    |name| {
                        alt_index
                            .get(name)
                            .map_or_else(Vec::new, |v| v.clone())
                    },
                    |name| {
                        glyph_declared_anchors.get(name).cloned()
                    },
                );

            let mut cached = composite_glyph(
                pg.pixels.as_ref(),
                &effective_refs,
                &cache,
                &color_aliases,
            ).unwrap_or_else(|| if let Some(grid) = &pg.pixels {
                CachedGlyph::from_grid(grid)
            } else {
                CachedGlyph {
                    width: 0,
                    height: 0,
                    contours: Vec::new(),
                    anchors: Vec::new(),
                    grid: None,
                    components: Vec::new(),
                }
            });
            cached.anchors = anchors;
            cache.insert(pg.name.clone(), cached);
            progress = true;
        }
    }

    fn ref_fill_info(
        gref: &GlyphRef,
        color_aliases: &ColorAliasMap,
    ) -> (Option<Rgba>, LayerVisibility) {
        let rgba = gref.fill.as_ref().and_then(|f| resolve_fill_rgba(f, color_aliases));
        let vis = effective_visibility(gref.visibility, gref.fill.as_ref(), color_aliases);
        (rgba, vis)
    }

    fn composite_glyph(
        own_pixels: Option<&PixelGrid>,
        refs: &[GlyphRef],
        cache: &HashMap<String, CachedGlyph>,
        color_aliases: &ColorAliasMap,
    ) -> Option<CachedGlyph> {
        let has_negated = refs.iter().any(|r| r.negated);

        if has_negated || own_pixels.is_some() {
            let mut min_r: i32 = 0;
            let mut min_c: i32 = 0;
            let mut max_r: i32 = 0;
            let mut max_c: i32 = 0;
            if let Some(grid) = own_pixels {
                max_r = grid.height as i32;
                max_c = grid.width as i32;
            }
            for gref in refs {
                if let Some(cached) = resolve_cached_ref(&gref.name, cache) {
                    let eff_r = gref.row() as i32;
                    let eff_c = gref.col() as i32;
                    if cached.width != 0 && cached.height != 0 {
                        min_r = min_r.min(eff_r);
                        min_c = min_c.min(eff_c);
                        max_r = max_r.max(eff_r + cached.height as i32);
                        max_c = max_c.max(eff_c + cached.width as i32);
                    }
                }
            }
            let width = (max_c - min_c).max(0) as u16;
            let height = (max_r - min_r).max(0) as u16;
            let mut result = PixelGrid::new(width, height);

            let mut components = Vec::new();
            let mut contour_layers: Vec<(&PixelGrid, i32, i32)> = Vec::new();
            let mut diff_layers: Vec<(&PixelGrid, i32, i32, bool)> = Vec::new();

            if let Some(grid) = own_pixels {
                let off_r = -min_r;
                let off_c = -min_c;
                result.blit(grid, off_r, off_c, false);
                components.push(SampleComponent {
                    row: off_r,
                    col: off_c,
                    grid: grid.clone(),
                    negated: false,
                    fill_rgba: None,
                    visibility: LayerVisibility::Both,
                });
                if has_negated {
                    diff_layers.push((grid, off_r, off_c, false));
                } else {
                    contour_layers.push((grid, off_r, off_c));
                }
            }

            for gref in refs {
                let Some(cached) = resolve_cached_ref(&gref.name, cache) else { continue };
                let off_r = gref.row() as i32 - min_r;
                let off_c = gref.col() as i32 - min_c;
                let (fill_rgba, fill_vis) = ref_fill_info(gref, color_aliases);
                if let Some(ref_grid) = &cached.grid {
                    result.blit(ref_grid, off_r, off_c, gref.negated);
                    if has_negated {
                        diff_layers.push((ref_grid, off_r, off_c, gref.negated));
                    } else if !gref.negated {
                        contour_layers.push((ref_grid, off_r, off_c));
                    }
                }
                for comp in &cached.components {
                    components.push(SampleComponent {
                        row: comp.row + off_r,
                        col: comp.col + off_c,
                        grid: comp.grid.clone(),
                        negated: comp.negated ^ gref.negated,
                        fill_rgba: fill_rgba.clone().or_else(|| comp.fill_rgba.clone()),
                        visibility: if gref.fill.is_some() || gref.visibility.is_some() { fill_vis } else { comp.visibility },
                    });
                }
            }

            let contours = if has_negated {
                track_contour_multi_diff(&diff_layers, PX_SUBPIXEL)
            } else {
                track_contour_multi(&contour_layers, PX_SUBPIXEL)
            };
            return Some(CachedGlyph {
                width,
                height,
                contours,
                anchors: Vec::new(),
                grid: Some(result),
                components,
            });
        }

        // Simple contour translation
        let mut all_contours = Vec::new();
        let mut max_width = 0u16;
        let mut max_height = 0u16;
        let mut combined_grid: Option<PixelGrid> = None;
        let mut components = Vec::new();

        for gref in refs {
            let cached = resolve_cached_ref(&gref.name, cache)?;
            let dx = gref.col() as f32;
            let dy = gref.row() as f32;
            for contour in &cached.contours {
                let translated: Vec<(f32, f32)> =
                    contour.iter().map(|&(x, y)| (x + dx, y + dy)).collect();
                all_contours.push(translated);
            }
            let w = (gref.col() as i32 + cached.width as i32).max(0) as u16;
            let h = (gref.row() as i32 + cached.height as i32).max(0) as u16;
            max_width = max_width.max(w);
            max_height = max_height.max(h);

            let off_r = gref.row() as i32;
            let off_c = gref.col() as i32;
            let (fill_rgba, fill_vis) = ref_fill_info(gref, color_aliases);
            for comp in &cached.components {
                components.push(SampleComponent {
                    row: comp.row + off_r,
                    col: comp.col + off_c,
                    grid: comp.grid.clone(),
                    negated: comp.negated,
                    fill_rgba: fill_rgba.clone().or_else(|| comp.fill_rgba.clone()),
                    visibility: if gref.fill.is_some() || gref.visibility.is_some() { fill_vis } else { comp.visibility },
                });
            }

            if let Some(ref_grid) = &cached.grid {
                let cg = combined_grid.get_or_insert_with(|| PixelGrid::new(max_width, max_height));
                if cg.width < max_width || cg.height < max_height {
                    cg.resize(max_width, max_height);
                }
                for r in 0..ref_grid.height as i32 {
                    for c in 0..ref_grid.width as i32 {
                        let shape = ref_grid.get(r as u16, c as u16);
                        if !shape.is_empty() {
                            let dr = off_r + r;
                            let dc = off_c + c;
                            if dr >= 0 && dc >= 0 && dr < max_height as i32 && dc < max_width as i32 {
                                cg.set(dr as u16, dc as u16, shape);
                            }
                        }
                    }
                }
            }
        }

        Some(CachedGlyph {
            width: max_width,
            height: max_height,
            contours: all_contours,
            anchors: Vec::new(),
            grid: combined_grid,
            components,
        })
    }

    // Collect cmap
    let mut cmap: BTreeMap<u32, String> = BTreeMap::new();
    for item in &all_items {
        match item {
            DocumentItem::Map { char_repr, glyph } => {
                let pairs = expand_map_pairs(char_repr, glyph);
                for (cp, glyph_name) in pairs {
                    cmap.entry(cp).or_insert(glyph_name);
                }
            }
            DocumentItem::MapDecomposed { char_repr } => {
                if let Some(cp) = crate::render::ttf_builder::parse_map_char(char_repr) {
                    cmap.entry(cp).or_insert_with(|| format!("uni{cp:04X}"));
                }
            }
            _ => {}
        }
    }

    // Collect exclude-from-sample
    let mut excluded: BTreeSet<u32> = BTreeSet::new();
    for item in &all_items {
        if let DocumentItem::Directive(s) = item
            && let Some(rest) = s.strip_prefix("exclude-from-sample ") {
                for tok in rest.split_whitespace() {
                    if let Some(cp) = crate::render::ttf_builder::parse_map_char(tok) {
                        excluded.insert(cp);
                    } else {
                        let pairs = expand_map_pairs(tok, "");
                        for (cp, _) in pairs {
                            excluded.insert(cp);
                        }
                    }
                }
            }
    }

    // Collect features
    let mut features: Vec<String> = Vec::new();
    let mut seen_features: HashSet<String> = HashSet::new();
    for item in &all_items {
        if let DocumentItem::Feature { name, .. } = item
            && seen_features.insert(name.clone()) {
                features.push(name.clone());
            }
    }

    // Build sample glyphs from cache
    let mut sample_glyphs: HashMap<String, SampleGlyph> = HashMap::new();
    for glyph_name in cmap.values() {
        if sample_glyphs.contains_key(glyph_name) {
            continue;
        }
        if let Some(cached) = cache.get(glyph_name) {
            let (left, top) = glyph_offsets.get(glyph_name).copied().unwrap_or((0, 0));
            sample_glyphs.insert(glyph_name.clone(), SampleGlyph {
                width: cached.width,
                _height: cached.height,
                components: cached.components.clone(),
                left,
                top,
            });
        }
    }

    Some(SampleData {
        height,
        ascent,
        descent,
        cmap,
        glyphs: sample_glyphs,
        excluded,
        features,
    })
}

// ---------------------------------------------------------------------------
// SVG path generation from contours
// ---------------------------------------------------------------------------

fn contours_to_svg_path(
    contours: &[Vec<(f32, f32)>],
    scale: f32,
    off_x: f32,
    off_y: f32,
) -> String {
    let mut path = String::new();
    for contour in contours {
        if contour.is_empty() {
            continue;
        }
        let (x0, y0) = contour[0];
        let _ = write!(path, "M{} {}", (x0 + off_x) * scale, (y0 + off_y) * scale);
        let mut prev_x = (x0 + off_x) * scale;
        let mut prev_y = (y0 + off_y) * scale;
        for &(x, y) in &contour[1..] {
            let sx = (x + off_x) * scale;
            let sy = (y + off_y) * scale;
            let dx = sx - prev_x;
            let dy = sy - prev_y;
            if dy == 0.0 {
                let _ = write!(path, "h{dx}");
            } else if dx == 0.0 {
                let _ = write!(path, "v{dy}");
            } else {
                let _ = write!(path, "l{dx} {dy}");
            }
            prev_x = sx;
            prev_y = sy;
        }
        path.push('z');
    }
    path
}

fn path_hash_color(path: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in path.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    (h & 0x7f7f7f) + 0x808080
}

fn sample_display_metrics(sg: &SampleGlyph, font_height: u16) -> (u16, u16, i16, i16) {
    let col_off = sg.left.max(0);
    let row_off = sg.top.max(0);
    let display_w = col_off as u16 + sg.width;
    let display_h = font_height;
    (display_w, display_h, col_off, row_off)
}

fn composite_components(width: u16, height: u16, components: &[SampleComponent]) -> PixelGrid {
    use crate::pixel::PixelShape;
    let mut grid = PixelGrid::new(width, height);
    for comp in components {
        for r in 0..comp.grid.height as i32 {
            for c in 0..comp.grid.width as i32 {
                let shape = comp.grid.get(r as u16, c as u16);
                if shape.is_filled() {
                    let dr = comp.row + r;
                    let dc = comp.col + c;
                    if dr >= 0 && dc >= 0 && dr < height as i32 && dc < width as i32 {
                        if comp.negated {
                            grid.set(dr as u16, dc as u16, PixelShape::EMPTY);
                        } else {
                            grid.set(dr as u16, dc as u16, shape);
                        }
                    }
                }
            }
        }
    }
    grid
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

fn char_name_str(cp: u32) -> String {
    if let Some(ch) = char::from_u32(cp) {
        let name = unicode_names2::name(ch)
            .map(|n| n.to_string())
            .unwrap_or_default();
        if name.is_empty() {
            format!("U+{cp:04X} ({ch})")
        } else {
            format!("U+{cp:04X} {name} ({ch})")
        }
    } else {
        format!("U+{cp:04X}")
    }
}

// ---------------------------------------------------------------------------
// sample.html
// ---------------------------------------------------------------------------

pub fn write_sample_html(w: &mut dyn Write, docs: &[&Document]) -> io::Result<()> {
    let Some(data) = collect_sample_data(docs) else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "no glyph data"));
    };

    let svg_scale: f32 = 2.0;

    write!(w, "\
<!doctype html>
<html><head><meta charset=utf-8><title>Unison: graphic sample</title><style>
body{{background:black;color:white;line-height:1}}div{{color:gray}}#sampleglyphs{{display:none}}body.sample #sampleglyphs{{display:block}}body.sample #glyphs{{display:none}}.scaled{{font-size:500%}}
svg{{background:#111;fill:white;vertical-align:top}}.glyphs>:nth-child(even) svg{{background:#222}}:target svg{{background:#333}}svg:hover>path:not(.c),body.sample svg>path:not(.c){{fill:white}}a svg>path:not(.c){{fill:gray}}
</style></head><body>
<input id=sample placeholder='Input sample text here' size=40> <input type=reset id=reset value=Reset> | {nchars} characters
<hr><div id=sampleglyphs></div><div id=glyphs><span class=glyphs>",
        nchars = data.cmap.len(),
    )?;

    // Small glyphs (1x scale)
    let mut excluded_run = false;
    for (&cp, glyph_name) in &data.cmap {
        if data.excluded.contains(&cp) {
            if !excluded_run {
                write!(w, "\u{2026}")?;
                excluded_run = true;
            }
            continue;
        }
        excluded_run = false;

        let title = html_escape(&char_name_str(cp));
        write!(w, "<a href='#u{cp:x}'><span id='sm-u{cp:x}' title='{title}'>")?;
        if let Some(sg) = data.glyphs.get(glyph_name) {
            let (display_w, display_h, col_off, row_off) = sample_display_metrics(sg, data.height);
            let shifted: Vec<SampleComponent> = sg.components.iter()
                .filter(|c| c.visibility != LayerVisibility::ColorOnly)
                .map(|c| {
                    let mut c2 = c.clone();
                    c2.col += col_off as i32;
                    c2.row += row_off as i32;
                    c2
                }).collect();
            let combined = composite_components(display_w, display_h, &shifted);
            let contours = track_contour_fullpixel(&combined);
            let path = contours_to_svg_path(&contours, 1.0, 0.0, 0.0);
            write!(w, "<svg viewBox=\"0 0 {display_w} {display_h}\" width=\"{display_w}\" height=\"{display_h}\"><path d='{path}'/></svg>")?;
        }
        write!(w, "</span></a>")?;
    }

    write!(w, "</span><hr><span class='scaled glyphs'>")?;

    // Large glyphs (scaled)
    excluded_run = false;
    for (&cp, glyph_name) in &data.cmap {
        if data.excluded.contains(&cp) {
            if !excluded_run {
                write!(w, "\u{2026}")?;
                excluded_run = true;
            }
            continue;
        }
        excluded_run = false;

        let title = html_escape(&char_name_str(cp));
        write!(w, "<span id='u{cp:x}' title='{title}'>")?;
        if let Some(sg) = data.glyphs.get(glyph_name) {
            let (display_w, display_h, col_off, row_off) = sample_display_metrics(sg, data.height);
            let vw = display_w as f32 * svg_scale;
            let vh = display_h as f32 * svg_scale;
            let sw = display_w as u32 * 5;
            let sh = display_h as u32 * 5;
            write!(w, "<svg viewBox=\"0 0 {vw} {vh}\" width=\"{sw}\" height=\"{sh}\">")?;
            for comp in &sg.components {
                if comp.visibility == LayerVisibility::MonoOnly {
                    continue;
                }
                let contours = track_contour(&comp.grid, PX_SUBPIXEL);
                let path = contours_to_svg_path(&contours, svg_scale, (comp.col + col_off as i32) as f32, (comp.row + row_off as i32) as f32);
                if !path.is_empty() {
                    if comp.negated {
                        write!(w, "<path class='c' d='{path}' fill='#000'/>")?;
                    } else if let Some(ref rgba) = comp.fill_rgba {
                        write!(w, "<path class='c' d='{path}' fill='#{:02x}{:02x}{:02x}'/>"
                            , rgba.r, rgba.g, rgba.b)?;
                    } else {
                        let color = path_hash_color(&path);
                        write!(w, "<path d='{path}' fill='#{color:06x}'/>")?;
                    }
                }
            }
            write!(w, "</svg>")?;
        }
        write!(w, "</span>")?;
    }

    write!(w, "</span></div><script>\n\
prevt=0;
function $(x){{return document.getElementById(x)}}
function f(t,h){{if(t.normalize)t=t.normalize();if(prevt===t)return;prevt=t;if(!h)location.hash=t?'#!'+encodeURIComponent(t):'';$('sample').value=t;document.body.className=t?'sample':'';var sm='',bg='';for(var i=0;i<t.length;++i){{var c=t.charCodeAt(i).toString(16);sm+=($('sm-u'+c)||{{}}).innerHTML||t[i];bg+=($('u'+c)||{{}}).innerHTML||t[i]}}$('sampleglyphs').innerHTML=sm+'<hr><span class=scaled>'+bg+'</span>'}}
(window.onhashchange=function(){{var h=location.hash||'';f(h.match(/^#!/)?decodeURIComponent(h.substring(2)):'',1);return false}})();
$('sample').onchange=$('sample').onkeyup=function(e){{f(this.value)}}
$('reset').onclick=function(){{$('sample').value='';f('')}}
</script></body></html>\n")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// sample.png
// ---------------------------------------------------------------------------

pub fn write_sample_png(w: &mut dyn Write, docs: &[&Document]) -> io::Result<()> {
    let Some(data) = collect_sample_data(docs) else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "no glyph data"));
    };

    let max_height = data.height as u32;
    let line_width: u32 = 512;
    let num_glyphs_per_line: &[u32] = &[64, 32, 16, 8, 4, 2, 1];

    fn multiples(a: u32, b: u32) -> u32 {
        a & (-(b as i32) as u32)
    }

    // Determine glyph widths and unavailable width slots
    let mut glyph_widths: HashMap<String, (u32, u32)> = HashMap::new();
    let mut unavailable_widths: HashSet<(u32, u32)> = HashSet::new();

    for (&cp, glyph_name) in &data.cmap {
        if let Some(sg) = data.glyphs.get(glyph_name) {
            let (dw, dh, _, _) = sample_display_metrics(sg, data.height);
            let w = dw as u32;
            let h = dh as u32;
            glyph_widths.insert(glyph_name.clone(), (w, h));
            for &ngl in num_glyphs_per_line {
                if w > line_width / ngl {
                    unavailable_widths.insert((ngl, multiples(cp, ngl)));
                }
            }
        }
    }

    // Determine glyph positions
    let mut last: Option<(u32, u32)> = None;
    let mut row: i32 = -1;
    let mut gap: u32 = 0;
    let mut positions: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
    let mut row_starts: Vec<u32> = Vec::new();
    let mut row_offsets: Vec<u32> = Vec::new();
    let max_glyphs_per_line: u32 = 64;

    for &cp in data.cmap.keys() {
        let mut current = None;
        for &ngl in num_glyphs_per_line {
            if !unavailable_widths.contains(&(ngl, multiples(cp, ngl))) {
                current = Some((ngl, multiples(cp, ngl)));
                break;
            }
        }
        let Some(cur) = current else { continue };
        let ngl = cur.0;
        if last != Some(cur) {
            if cur.1.saturating_sub(last.map_or(0, |(_, l)| l)) > max_glyphs_per_line {
                gap += 8;
            }
            row += 1;
            row_starts.push(multiples(cp, ngl));
            row_offsets.push(gap);
            last = Some(cur);
        }
        positions.insert(cp, (row as u32, (cp & (ngl - 1)) * (line_width / ngl)));
    }
    let nrows = (row + 1) as u32;
    if nrows == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "no glyph positions"));
    }

    let label_width: u32 = 8 * 8 + 1;
    let img_width = label_width + 1 + line_width + 1;
    let img_height = (max_height + 1) * nrows + 1 + gap;

    // Render to RGBA pixel buffer
    let mut pixels = vec![0xFFu8; (img_width * img_height * 4) as usize];
    let stride = img_width as usize;

    fn set_pixel(pixels: &mut [u8], stride: usize, x: usize, y: usize, r: u8, g: u8, b: u8) {
        let off = (y * stride + x) * 4;
        pixels[off] = r;
        pixels[off + 1] = g;
        pixels[off + 2] = b;
        pixels[off + 3] = 255;
    }

    // Draw grid lines
    for row_idx in 0..nrows {
        let prev_offset = if row_idx > 0 { row_offsets[row_idx as usize - 1] } else { 0 };
        let offset = row_offsets[row_idx as usize];

        for extra_y in prev_offset..offset {
            let y = (max_height + 1) * row_idx + extra_y;
            if y < img_height {
                for x in label_width..img_width {
                    set_pixel(&mut pixels, stride, x as usize, y as usize, 0x80, 0x80, 0x80);
                }
            }
        }

        let y = (max_height + 1) * row_idx + offset;
        if y < img_height {
            for x in label_width..img_width {
                set_pixel(&mut pixels, stride, x as usize, y as usize, 0x80, 0x80, 0x80);
            }
        }

        let label = format!("U+{:04X}", row_starts[row_idx as usize]);
        let label_y = y + 1;
        for (char_idx, ch) in label.chars().enumerate() {
            let cp = ch as u32;
            if let Some(glyph_name) = data.cmap.get(&cp)
                && let Some(sg) = data.glyphs.get(glyph_name) {
                    render_glyph_bitmap_rgba(
                        &mut pixels,
                        stride,
                        img_height as usize,
                        (char_idx as u32 * 8) as i32,
                        label_y as i32,
                        sg,
                        [0x80, 0x80, 0x80],
                        false,
                    );
                }
        }
    }
    // Bottom border
    {
        let y = img_height - 1;
        for x in label_width..img_width {
            set_pixel(&mut pixels, stride, x as usize, y as usize, 0x80, 0x80, 0x80);
        }
    }

    // Fill glyph content area with gray so empty slots are distinguishable
    for row_idx in 0..nrows {
        let offset = row_offsets[row_idx as usize];
        let y = (max_height + 1) * row_idx + offset + 1;
        for dy in 0..max_height {
            let py = (y + dy) as usize;
            if py < img_height as usize {
                for x in (label_width + 1) as usize..(img_width - 1) as usize {
                    set_pixel(&mut pixels, stride, x, py, 0xC0, 0xC0, 0xC0);
                }
            }
        }
    }

    // Render each glyph
    for (&cp, glyph_name) in &data.cmap {
        let Some(&(r, left)) = positions.get(&cp) else { continue };
        let Some(sg) = data.glyphs.get(glyph_name) else { continue };
        let y = (max_height + 1) * r + row_offsets[r as usize] + 1;
        let x = label_width + 1 + left;

        let (dw, dh, col_off, row_off) = sample_display_metrics(sg, data.height as u16);

        // Clear glyph area to white
        for dy in 0..dh.min(max_height as u16) as u32 {
            for dx in 0..dw as u32 {
                let py = (y + dy) as usize;
                let px = (x + dx) as usize;
                if py < img_height as usize && px < stride {
                    set_pixel(&mut pixels, stride, px, py, 0xFF, 0xFF, 0xFF);
                }
            }
        }

        render_glyph_bitmap_rgba(
            &mut pixels,
            stride,
            img_height as usize,
            x as i32 + col_off as i32,
            y as i32 + row_off as i32,
            sg,
            [0x00, 0x00, 0x00],
            true,
        );
    }

    // Encode as PNG
    encode_rgba_png(w, &pixels, img_width, img_height)
}

fn render_glyph_bitmap_rgba(
    pixels: &mut [u8],
    stride: usize,
    img_height: usize,
    x: i32,
    y: i32,
    sg: &SampleGlyph,
    fg_color: [u8; 3],
    use_fill_colors: bool,
) {
    for comp in &sg.components {
        if use_fill_colors && comp.visibility == LayerVisibility::MonoOnly {
            continue;
        }

        let [cr, cg, cb] = if use_fill_colors && !comp.negated {
            if let Some(ref rgba) = comp.fill_rgba {
                [rgba.r, rgba.g, rgba.b]
            } else {
                fg_color
            }
        } else if comp.negated {
            [0xFF, 0xFF, 0xFF]
        } else {
            fg_color
        };

        for r in 0..comp.grid.height as i32 {
            for c in 0..comp.grid.width as i32 {
                let shape = comp.grid.get(r as u16, c as u16);
                if shape.is_filled() {
                    let py = y + comp.row + r;
                    let px = x + comp.col + c;
                    if py >= 0 && px >= 0 && (py as usize) < img_height && (px as usize) < stride {
                        let off = (py as usize * stride + px as usize) * 4;
                        pixels[off] = cr;
                        pixels[off + 1] = cg;
                        pixels[off + 2] = cb;
                        pixels[off + 3] = 255;
                    }
                }
            }
        }
    }
}

fn encode_rgba_png(w: &mut dyn Write, pixels: &[u8], width: u32, height: u32) -> io::Result<()> {
    let mut encoder = png::Encoder::new(w, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(pixels)?;
    writer.finish()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// live.html
// ---------------------------------------------------------------------------

pub fn write_live_html(
    w: &mut dyn Write,
    docs: &[&Document],
    ttf_bytes: &[u8],
    data_dir: Option<&Path>,
) -> io::Result<()> {
    write_live_html_inner(w, docs, ttf_bytes, None, data_dir)
}

pub fn write_live_html_woff2(
    w: &mut dyn Write,
    docs: &[&Document],
    ttf_bytes: &[u8],
    woff2_bytes: &[u8],
    data_dir: Option<&Path>,
) -> io::Result<()> {
    write_live_html_inner(w, docs, ttf_bytes, Some(woff2_bytes), data_dir)
}

fn write_live_html_inner(
    w: &mut dyn Write,
    docs: &[&Document],
    ttf_bytes: &[u8],
    woff2_bytes: Option<&[u8]>,
    data_dir: Option<&Path>,
) -> io::Result<()> {
    let Some(data) = collect_sample_data(docs) else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "no glyph data"));
    };

    let (font_mime, font_data) = if let Some(w2) = woff2_bytes {
        ("font/woff2", w2)
    } else {
        ("font/ttf", ttf_bytes)
    };
    let font_base64 = base64_encode(font_data);

    let features = if data.features.is_empty() {
        "inherit".to_string()
    } else {
        data.features.iter().map(|f| format!("'{f}'")).collect::<Vec<_>>().join(",")
    };

    let has_udhr = data_dir.is_some_and(|d| d.join("udhr-article1.json").exists());
    let has_confusables = data_dir.is_some_and(|d| {
        std::fs::read_dir(d)
            .map(|entries| entries.filter_map(|e| e.ok()).any(|e| {
                e.file_name().to_str().is_some_and(|n| n.starts_with("confusables") && n.ends_with(".txt"))
            }))
            .unwrap_or(false)
    });

    write!(w, "\
<!doctype html>
<html><head><meta charset=utf-8><title>Unison: live sample</title>
<style>
@font-face{{font-family:Unison;src:url(data:{font_mime};base64,{font_base64});font-feature-settings:{features}}}
pre{{font-family:Unison,monospace;font-size:200%;line-height:1;margin:0;white-space:pre-wrap}}pre span{{background:#eee}}.hide{{display:none}}
</style>
<script>
window.onload=function(){{var e=document.getElementById('edit');e.contentEditable='true';for(var x=document.querySelectorAll('a[href^=\"#\"]'),i=0;x[i];++i)x[i].onclick=function(){{e.innerHTML=document.getElementById(this.getAttribute('href').substring(1)).innerHTML;return false}}}}
</script>
</head><body><pre>
Hello? This is the <u>Unison</u> font.
You can play with it right here.
Please note that this is in development and subject to change.

Load: ")?;

    let mut links: Vec<(&str, &str)> = Vec::new();
    if has_udhr { links.push(("udhr", "UDHR")); }
    if has_confusables { links.push(("confus", "Confusables")); }
    links.push(("hangul", "All Hangul"));
    links.push(("all", "All Glyphs"));
    for (i, (id, label)) in links.iter().enumerate() {
        if i > 0 { write!(w, ", ")?; }
        write!(w, "<a href='#{id}'>{label}</a>")?;
    }

    write!(w, "\n\
\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}
</pre><pre id=edit>")?;

    // Logo
    write!(w, r#"
888     888          d8b
888     888          Y8P
888     888
888     888 88888b.  888 .d8888b   .d88b.  88888b.
888     888 888 "88b 888 88K      d88""88b 888 "88b
888     888 888  888 888 "Y8888b. 888  888 888  888
Y88b. .d88P 888  888 888      X88 Y88..88P 888  888
 "Y88888P"  888  888 888  88888P'  "Y88P"  888  888
"#)?;

    // UDHR section
    if has_udhr {
        write_live_udhr(w, data_dir.unwrap(), &data.cmap)?;
    }

    // Confusables section
    if has_confusables {
        write_live_confusables(w, data_dir.unwrap(), &data.cmap)?;
    }

    // Hangul section
    write_live_hangul(w)?;

    // All Glyphs section
    write!(w, "</pre><pre id=all class=hide>\n\
\u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}\n\
\u{2502}All Supported Glyphs\u{2502}\n\
\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}\n")?;

    let mut chars = String::new();
    let mut prev_block: Option<u32> = None;
    for &cp in data.cmap.keys() {
        let block = cp >> 5;
        if prev_block.is_some_and(|pb| pb != block) {
            writeln!(w, "<span>{}</span>", html_escape(&chars))?;
            chars.clear();
        }
        prev_block = Some(block);
        if let Some(ch) = char::from_u32(cp) {
            chars.push(ch);
        }
    }
    if !chars.is_empty() {
        writeln!(w, "<span>{}</span>", html_escape(&chars))?;
    }

    writeln!(w, "</pre></body></html>")?;

    Ok(())
}

// ---------------------------------------------------------------------------
// live.html: UDHR section
// ---------------------------------------------------------------------------

fn write_live_udhr(
    w: &mut dyn Write,
    data_dir: &Path,
    cmap: &BTreeMap<u32, String>,
) -> io::Result<()> {
    #[derive(serde::Deserialize)]
    struct UdhrEntry {
        lang: String,
        text: String,
    }

    let path = data_dir.join("udhr-article1.json");
    let content = std::fs::read_to_string(&path)?;
    let entries: Vec<UdhrEntry> = serde_json::from_str(&content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let cmap_set: HashSet<u32> = cmap.keys().copied().collect();

    // Filter to entries whose characters are all in the font
    let mut displayable: Vec<&UdhrEntry> = Vec::new();
    let mut unsupported_chars_by_entry: HashMap<usize, BTreeSet<char>> = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        let mut unsupported = BTreeSet::new();
        for ch in entry.text.chars() {
            if !cmap_set.contains(&(ch as u32)) {
                unsupported.insert(ch);
            }
        }
        if unsupported.is_empty() {
            displayable.push(entry);
        } else {
            unsupported_chars_by_entry.insert(i, unsupported);
        }
    }

    // Greedy set cover in JSON order: select entries that add new codepoints
    let mut covered: HashSet<u32> = HashSet::new();
    let mut selected_indices: Vec<usize> = Vec::new();

    for (i, entry) in displayable.iter().enumerate() {
        let has_new = entry.text.chars().any(|ch| !covered.contains(&(ch as u32)));
        if has_new {
            selected_indices.push(i);
            for ch in entry.text.chars() {
                covered.insert(ch as u32);
            }
        }
    }

    let udhr_title = "Article 1 of Universal Declaration of Human Rights";
    let border: String = std::iter::repeat_n('\u{2500}', udhr_title.len()).collect();
    write!(w, "</pre><pre id=udhr class=hide>\n\
\u{250c}{border}\u{2510}\n\
\u{2502}{udhr_title}\u{2502}\n\
\u{2514}{border}\u{2518}\n\n")?;

    let selected_set: HashSet<usize> = selected_indices.iter().copied().collect();

    let mut disp_idx = 0;
    for (orig_idx, entry) in entries.iter().enumerate() {
        if unsupported_chars_by_entry.contains_key(&orig_idx) {
            continue;
        }
        if selected_set.contains(&disp_idx) {
            writeln!(
                w,
                "\u{2022} {}: <span>{}</span>",
                html_escape(&entry.lang),
                html_escape(&entry.text),
            )?;
        }
        disp_idx += 1;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// live.html: Confusables section
// ---------------------------------------------------------------------------

fn write_live_confusables(
    w: &mut dyn Write,
    data_dir: &Path,
    cmap: &BTreeMap<u32, String>,
) -> io::Result<()> {
    let confusables_path = std::fs::read_dir(data_dir)?
        .filter_map(|e| e.ok())
        .find(|e| {
            e.file_name().to_str().is_some_and(|n| {
                n.starts_with("confusables") && n.ends_with(".txt")
            })
        })
        .map(|e| e.path())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "confusables file not found"))?;

    let content = std::fs::read_to_string(&confusables_path)?;
    let cmap_set: HashSet<u32> = cmap.keys().copied().collect();

    // Parse confusables: source_cp -> target_cp (both single codepoints)
    // Group by target_cp to form equivalence groups
    let mut groups: BTreeMap<Vec<u32>, Vec<Vec<u32>>> = BTreeMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Format: XXXX ; YYYY ZZZZ... ; MA/SA  # comment
        let parts: Vec<&str> = line.splitn(4, ';').collect();
        if parts.len() < 3 {
            continue;
        }
        let source_cps: Vec<u32> = parts[0]
            .split_whitespace()
            .filter_map(|s| u32::from_str_radix(s.trim(), 16).ok())
            .collect();
        let target_cps: Vec<u32> = parts[1]
            .split_whitespace()
            .filter_map(|s| u32::from_str_radix(s.trim(), 16).ok())
            .collect();
        if source_cps.is_empty() || target_cps.is_empty() {
            continue;
        }
        groups.entry(target_cps.clone()).or_default().push(source_cps);
    }

    // Build equivalence groups: target + all sources that map to it
    // Filter: only include groups where at least 2 members are fully in the font
    struct ConfusGroup {
        members: Vec<Vec<u32>>,
    }

    let mut display_groups: Vec<ConfusGroup> = Vec::new();

    for (target, sources) in &groups {
        let mut all_members: Vec<&Vec<u32>> = vec![target];
        for src in sources {
            all_members.push(src);
        }

        let displayable: Vec<Vec<u32>> = all_members
            .into_iter()
            .filter(|cps| cps.iter().all(|cp| cmap_set.contains(cp)))
            .cloned()
            .collect();

        if displayable.len() >= 2 {
            // Sort: target first, then sources by codepoint
            let mut members = displayable;
            members.sort();
            members.dedup();
            display_groups.push(ConfusGroup { members });
        }
    }

    // Sort groups by first member's first codepoint
    display_groups.sort_by(|a, b| a.members[0].cmp(&b.members[0]));

    // Merge groups that share members (transitive closure)
    let mut merged: Vec<ConfusGroup> = Vec::new();
    for group in display_groups {
        let mut found = None;
        for (i, existing) in merged.iter().enumerate() {
            if group.members.iter().any(|m| existing.members.contains(m)) {
                found = Some(i);
                break;
            }
        }
        if let Some(i) = found {
            for m in group.members {
                if !merged[i].members.contains(&m) {
                    merged[i].members.push(m);
                }
            }
            merged[i].members.sort();
        } else {
            merged.push(group);
        }
    }

    write!(w, "</pre><pre id=confus class=hide>\n\
\u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}\n\
\u{2502}Confusables\u{2502}\n\
\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}\n\n")?;

    for group in &merged {
        for (i, member) in group.members.iter().enumerate() {
            if i > 0 {
                write!(w, " ")?;
            }
            // Build title with each codepoint's name
            let title_parts: Vec<String> = member
                .iter()
                .map(|&cp| char_name_str(cp))
                .collect();
            let title = html_escape(&title_parts.join("\n"));
            let text: String = member
                .iter()
                .filter_map(|&cp| char::from_u32(cp))
                .collect();
            write!(w, "<span title='{title}'>{}</span>", html_escape(&text))?;
        }
        writeln!(w)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// live.html: Hangul section
// ---------------------------------------------------------------------------

fn write_live_hangul(w: &mut dyn Write) -> io::Result<()> {
    write!(w, "</pre><pre id=hangul class=hide>\n\
\u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}\n\
\u{2502}All Hangul Syllables\u{2502}\n\
\u{2502} (Modern + Ancient) \u{2502}\n\
\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}\n\n\
<div style='white-space:pre'><span><a href='#' onclick=\"\
var p=[],i,j,k,a=[],b=[],c=[''];\
function v(z,s){{for(i=0;s[i];++i)for(j=s[i][0];j&lt;=(s[i][1]||s[i][0]);++j)z.push(String.fromCharCode(j))}}\
v(a,[[0x115f],[0x1100,0x115e],[0xa960,0xa97c]]);\
v(b,[[0x1160,0x117e],[0x119e],[0x11a1],[0x11a3,0x11a4]]);\
v(c,[[0x11a8,0x11c2]]);\
for(i=0;i&lt;a.length;++i,p.push('\\n'))for(j=0;j&lt;b.length;++j,p.push('\\n'))for(k=0;k&lt;c.length;++k)p.push(a[i]+b[j]+c[k]);\
this.parentNode.replaceChild(document.createTextNode(p.join('')),this);\
return!1\">Render!</a></span></div>\n")?;

    Ok(())
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_io;

    fn parse(input: &str) -> Document {
        document_io::parse_document_from_str(input, "test.unf".into()).unwrap()
    }

    #[test]
    fn sample_selects_alternative_glyph_on_anchor_size_mismatch() {
        // Mirrors ref_composite::tests::alternative_glyph_selected_on_size_mismatch,
        // but exercised through the sample-rendering path (collect_sample_data),
        // which used to never consider alternatives because it passed
        // `|_| Vec::new()` as `lookup_alternatives`.
        let d = parse(
            "\
font-meta height 16 ascent 12 descent 4

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

map A = container
",
        );
        let data = collect_sample_data(&[&d]).expect("sample data should build");
        let container = data.glyphs.get("container").expect("container glyph present");
        // stem (1-wide -join) doesn't size-match +join (2-wide), so stem:wide
        // (2-wide -join) must be selected instead, placed at offset col=3.
        // Total width becomes max(6, 3 + 4) = 7; without alternative
        // selection, stem (width 2) is placed at (0, 0) giving width 6.
        assert_eq!(
            container.width, 7,
            "stem:wide should have been selected via anchor-size matching"
        );
    }

    #[test]
    fn sample_includes_map_decomposed_composite_glyph() {
        // `map <precomposed char>` (DocumentItem::MapDecomposed) synthesizes a
        // composite glyph via NFD decomposition; it used to be silently
        // skipped when collecting sample data.
        let d = parse(
            "\
font-meta height 16 ascent 12 descent 4

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
map ä
",
        );
        let data = collect_sample_data(&[&d]).expect("sample data should build");
        let gid = data
            .cmap
            .get(&('ä' as u32))
            .cloned()
            .expect("'a with combining diaeresis' should be mapped in cmap");
        assert!(
            data.glyphs.contains_key(&gid),
            "sample glyph entry should exist for the map-decomposed character"
        );
    }

    #[test]
    fn sample_expanded_glyph_retains_declared_pixel_dims() {
        // Callsites of expand_glyph_block used to copy over the expanded
        // glyph items but drop `body.pixels`, so a pattern-named glyph with
        // declared dims + an all-empty grid + refs lost its declared
        // width/height in sample rendering.
        let d = parse(
            "\
font-meta height 16 ascent 12 descent 4

glyph part 2 2
@@@@
@@@@

glyph test-(a|b) 4 4
........
........
........
........
ref part

map A = test-a
",
        );
        let data = collect_sample_data(&[&d]).expect("sample data should build");
        let g = data.glyphs.get("test-a").expect("test-a should be in sample glyphs");
        assert_eq!(
            g.width, 4,
            "expanded glyph should retain its declared width despite an empty own grid"
        );
    }

    #[test]
    fn sample_display_metrics_reflects_top_flag() {
        // `sample_display_metrics` used to hardcode the vertical offset to 0,
        // so the `top N` glyph flag had no effect in sample output.
        let sg_with_top = SampleGlyph {
            width: 5,
            _height: 5,
            components: Vec::new(),
            left: 0,
            top: 3,
        };
        let (_, _, _, row_off) = sample_display_metrics(&sg_with_top, 16);
        assert_eq!(row_off, 3);

        let sg_without_top = SampleGlyph {
            width: 5,
            _height: 5,
            components: Vec::new(),
            left: 0,
            top: 0,
        };
        let (_, _, _, row_off) = sample_display_metrics(&sg_without_top, 16);
        assert_eq!(row_off, 0);
    }
}
