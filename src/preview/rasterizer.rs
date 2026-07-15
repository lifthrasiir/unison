use std::collections::HashMap;

use skrifa::color::{Brush, ColorGlyph, ColorPainter, CompositeMode, Transform};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::*;
use skrifa::{FontRef, MetadataProvider};

pub struct CachedGlyph {
    pub texture: egui::TextureHandle,
    pub bearing_x: f32,
    pub bearing_y: f32,
    pub width: f32,
    pub height: f32,
    pub is_color: bool,
}

pub struct GlyphCache {
    cache: HashMap<(u16, u32, u32), Option<CachedGlyph>>,
    font_gen: u64,
}

impl GlyphCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            font_gen: u64::MAX,
        }
    }

    pub fn invalidate_if_changed(&mut self, font_gen: u64) {
        if self.font_gen != font_gen {
            self.cache.clear();
            self.font_gen = font_gen;
        }
    }

    pub fn get_or_rasterize(
        &mut self,
        ctx: &egui::Context,
        font_data: &[u8],
        glyph_id: u16,
        px_size: f32,
        color: bool,
        text_color: egui::Color32,
    ) -> Option<&CachedGlyph> {
        let color_key = if color {
            let [r, g, b, _] = text_color.to_array();
            0x80000000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
        } else {
            0
        };
        let key = (glyph_id, (px_size * 4.0) as u32, color_key);

        self.cache
            .entry(key)
            .or_insert_with(|| {
                if color {
                    let [r, g, b, _] = text_color.to_array();
                    rasterize_color_glyph(ctx, font_data, glyph_id, px_size, [r, g, b, 255])
                        .or_else(|| rasterize_glyph(ctx, font_data, glyph_id, px_size, false))
                } else {
                    rasterize_glyph(ctx, font_data, glyph_id, px_size, false)
                }
            });

        self.cache.get(&key).and_then(|v| v.as_ref())
    }
}

struct SkiaPen {
    builder: tiny_skia::PathBuilder,
    scale_y: f32,
    offset_y: f32,
}

impl OutlinePen for SkiaPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.builder
            .move_to(x, self.offset_y + y * self.scale_y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.builder
            .line_to(x, self.offset_y + y * self.scale_y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.builder.quad_to(
            cx0,
            self.offset_y + cy0 * self.scale_y,
            x,
            self.offset_y + y * self.scale_y,
        );
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.builder.cubic_to(
            cx0,
            self.offset_y + cy0 * self.scale_y,
            cx1,
            self.offset_y + cy1 * self.scale_y,
            x,
            self.offset_y + y * self.scale_y,
        );
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

fn draw_outline_to_path(
    font: &FontRef<'_>,
    gid: GlyphId,
    px_size: f32,
) -> Option<tiny_skia::Path> {
    let outlines = font.outline_glyphs();
    let outline = outlines.get(gid)?;
    let settings = DrawSettings::unhinted(Size::new(px_size), LocationRef::default());

    let mut pen = SkiaPen {
        builder: tiny_skia::PathBuilder::new(),
        scale_y: -1.0,
        offset_y: 0.0,
    };
    outline.draw(settings, &mut pen).ok()?;
    pen.builder.finish()
}

fn rasterize_glyph(
    ctx: &egui::Context,
    font_data: &[u8],
    glyph_id: u16,
    px_size: f32,
    is_color: bool,
) -> Option<CachedGlyph> {
    let font = FontRef::new(font_data).ok()?;

    let gid = GlyphId::new(glyph_id as u32);
    let path = draw_outline_to_path(&font, gid, px_size)?;

    let metrics = font.glyph_metrics(Size::new(px_size), LocationRef::default());
    let bounds = metrics.bounds(gid)?;

    let pad = 1.0;
    let w = ((bounds.x_max - bounds.x_min).ceil() + pad * 2.0).max(1.0) as u32;
    let h = ((bounds.y_max - bounds.y_min).ceil() + pad * 2.0).max(1.0) as u32;

    if w > 512 || h > 512 {
        return None;
    }

    let bearing_x = bounds.x_min;
    let bearing_y = bounds.y_max;

    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;

    let paint = tiny_skia::Paint {
        shader: tiny_skia::Shader::SolidColor(tiny_skia::Color::WHITE),
        anti_alias: true,
        ..Default::default()
    };

    let transform = tiny_skia::Transform::from_translate(
        -bearing_x + pad,
        bearing_y + pad,
    );

    pixmap.fill_path(&path, &paint, tiny_skia::FillRule::Winding, transform, None);

    let premul = pixmap.data();
    let mut rgba = vec![0u8; premul.len()];
    for i in (0..premul.len()).step_by(4) {
        let a = premul[i + 3];
        rgba[i] = 255;
        rgba[i + 1] = 255;
        rgba[i + 2] = 255;
        rgba[i + 3] = a;
    }
    let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);

    let color_suffix = if is_color { "c" } else { "" };
    let texture = ctx.load_texture(
        format!("glyph_{glyph_id}_{}{color_suffix}", (px_size * 4.0) as u32),
        image,
        egui::TextureOptions::LINEAR,
    );

    Some(CachedGlyph {
        texture,
        bearing_x: bearing_x - pad,
        bearing_y: bearing_y + pad,
        width: w as f32,
        height: h as f32,
        is_color: false,
    })
}

struct ColrPainter<'a> {
    font: &'a FontRef<'a>,
    px_size: f32,
    pixmap: &'a mut tiny_skia::Pixmap,
    transform: tiny_skia::Transform,
    palette: Vec<[u8; 4]>,
    fg_color: [u8; 4],
}

impl ColorPainter for ColrPainter<'_> {
    fn push_transform(&mut self, _transform: Transform) {}
    fn pop_transform(&mut self) {}

    fn push_clip_glyph(&mut self, _glyph_id: GlyphId) {}
    fn push_clip_box(&mut self, _clip_box: read_fonts::types::BoundingBox<f32>) {}
    fn pop_clip(&mut self) {}

    fn fill(&mut self, _brush: Brush<'_>) {}

    fn fill_glyph(
        &mut self,
        glyph_id: GlyphId,
        _brush_transform: Option<Transform>,
        brush: Brush<'_>,
    ) {
        let Brush::Solid { palette_index, alpha } = brush else { return };

        let [r, g, b, a] = if palette_index == 0xFFFF {
            self.fg_color
        } else if let Some(&color) = self.palette.get(palette_index as usize) {
            color
        } else {
            return;
        };

        let effective_a = ((a as f32) * alpha).clamp(0.0, 255.0) as u8;

        let Some(path) = draw_outline_to_path(self.font, glyph_id, self.px_size) else {
            return;
        };

        let paint = tiny_skia::Paint {
            shader: tiny_skia::Shader::SolidColor(
                tiny_skia::Color::from_rgba8(r, g, b, effective_a),
            ),
            anti_alias: true,
            blend_mode: tiny_skia::BlendMode::SourceOver,
            ..Default::default()
        };

        self.pixmap.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            self.transform,
            None,
        );
    }

    fn push_layer(&mut self, _composite_mode: CompositeMode) {}
    fn pop_layer(&mut self) {}
}

fn rasterize_color_glyph(
    ctx: &egui::Context,
    font_data: &[u8],
    glyph_id: u16,
    px_size: f32,
    fg_color: [u8; 4],
) -> Option<CachedGlyph> {
    let font = FontRef::new(font_data).ok()?;
    let gid = GlyphId::new(glyph_id as u32);

    let color_glyph: ColorGlyph<'_> = font.color_glyphs().get(gid)?;

    let metrics = font.glyph_metrics(Size::new(px_size), LocationRef::default());
    let bounds = metrics.bounds(gid)?;

    let pad = 1.0;
    let w = ((bounds.x_max - bounds.x_min).ceil() + pad * 2.0).max(1.0) as u32;
    let h = ((bounds.y_max - bounds.y_min).ceil() + pad * 2.0).max(1.0) as u32;

    if w > 512 || h > 512 {
        return None;
    }

    let bearing_x = bounds.x_min;
    let bearing_y = bounds.y_max;

    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;

    let transform = tiny_skia::Transform::from_translate(
        -bearing_x + pad,
        bearing_y + pad,
    );

    let palette: Vec<[u8; 4]> = font
        .color_palettes()
        .get(0)
        .map(|p| {
            p.colors()
                .iter()
                .map(|c| [c.red, c.green, c.blue, c.alpha])
                .collect()
        })
        .unwrap_or_default();

    let mut painter = ColrPainter {
        font: &font,
        px_size,
        pixmap: &mut pixmap,
        transform,
        palette,
        fg_color,
    };

    color_glyph
        .paint(LocationRef::default(), &mut painter)
        .ok()?;

    let premul = pixmap.data();
    let mut rgba = vec![0u8; premul.len()];
    for i in (0..premul.len()).step_by(4) {
        let pa = premul[i + 3];
        if pa == 0 {
            continue;
        }
        let inv = 255.0 / pa as f32;
        rgba[i] = (premul[i] as f32 * inv).min(255.0) as u8;
        rgba[i + 1] = (premul[i + 1] as f32 * inv).min(255.0) as u8;
        rgba[i + 2] = (premul[i + 2] as f32 * inv).min(255.0) as u8;
        rgba[i + 3] = pa;
    }
    let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);

    let texture = ctx.load_texture(
        format!("glyph_{glyph_id}_{}c{:02x}{:02x}{:02x}", (px_size * 4.0) as u32,
            fg_color[0], fg_color[1], fg_color[2]),
        image,
        egui::TextureOptions::LINEAR,
    );

    Some(CachedGlyph {
        texture,
        bearing_x: bearing_x - pad,
        bearing_y: bearing_y + pad,
        width: w as f32,
        height: h as f32,
        is_color: true,
    })
}
