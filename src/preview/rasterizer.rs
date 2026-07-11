use std::collections::HashMap;

use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::*;
use skrifa::{FontRef, MetadataProvider};

pub struct CachedGlyph {
    pub texture: egui::TextureHandle,
    pub bearing_x: f32,
    pub bearing_y: f32,
    pub width: f32,
    pub height: f32,
}

pub struct GlyphCache {
    cache: HashMap<(u16, u32), Option<CachedGlyph>>,
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
    ) -> Option<&CachedGlyph> {
        let key = (glyph_id, (px_size * 4.0) as u32);

        self.cache
            .entry(key)
            .or_insert_with(|| rasterize_glyph(ctx, font_data, glyph_id, px_size));

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

fn rasterize_glyph(
    ctx: &egui::Context,
    font_data: &[u8],
    glyph_id: u16,
    px_size: f32,
) -> Option<CachedGlyph> {
    let font = FontRef::new(font_data).ok()?;

    let outlines = font.outline_glyphs();
    let gid = GlyphId::new(glyph_id as u32);
    let outline = outlines.get(gid)?;

    let settings = DrawSettings::unhinted(Size::new(px_size), LocationRef::default());

    // get glyph bounds
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

    let mut pen = SkiaPen {
        builder: tiny_skia::PathBuilder::new(),
        scale_y: -1.0,
        offset_y: 0.0,
    };

    outline.draw(settings, &mut pen).ok()?;
    let path = pen.builder.finish()?;

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

    // tiny-skia produces premultiplied RGBA; for a white fill, R=G=B=A.
    // Convert to un-premultiplied: extract alpha, set color to white.
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

    let texture = ctx.load_texture(
        format!("glyph_{glyph_id}_{}", (px_size * 4.0) as u32),
        image,
        egui::TextureOptions::LINEAR,
    );

    Some(CachedGlyph {
        texture,
        bearing_x: bearing_x - pad,
        bearing_y: bearing_y + pad,
        width: w as f32,
        height: h as f32,
    })
}
