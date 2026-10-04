//! `uniform render`: draw glyphs or shaped text from the built font, for
//! whoever is drawing them.
//!
//! The picture is taken from the *font*, not from the source: the project is
//! built exactly as the demo page builds it (one variable font carrying both
//! drawings on the `BMAP` axis,
//! [`build_face_variable`](crate::render::ttf_builder::build_face_variable)),
//! the items are shaped with the same rustybuzz call `assert shape` uses, and
//! each one is drawn at both ends of the axis. So what the picture shows is
//! what ships — composites resolved, sub-pixel shapes traced, `remap`/anchors
//! applied for text — and a bug anywhere on the way is visible in it, which a
//! renderer of the grid alone would hide.
//!
//! Every row of the PNG is one item, in three panels: the vector build, the
//! bitmap build, and the two laid over each other (bitmap ink pale, vector ink
//! dark), which is where a sub-pixel shape lighting the wrong cell shows. The
//! pixel grid is drawn over all three, the baseline in red and the pen's start
//! and end in blue. `--ascii` prints the bitmap build in the source's own
//! `@@`/`..` spelling instead, one row per pixel from the ascent down.
//!
//! A `-g` name need not be reachable: the build keeps every one as if it said
//! `keep` ([`build_face_variable_keeping`]), which is what makes a helper
//! (`@-bar`) or a glyph nothing maps yet drawable, under its own advance. An
//! on-demand shape (`4x4-dr`) is synthesized only once something refers to
//! it, so each name is also `ref`'d from a root glyph added to the project.
//!
//! The rasterizer is the signed-area accumulator of `font-rs`: small, exact
//! for polygons, and it keeps the headless binary free of `tiny-skia`.

use std::path::PathBuf;

use skrifa::instance::{Location, LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

use crate::document::Document;
use crate::render::ttf_builder::{UNITS_PER_EM, build_face_variable_keeping};

/// The name of the `n`th synthesized root; see the module docs.
fn root_name(n: usize) -> String {
    format!("uniform-render-root-{n}")
}

/// One thing to draw, as the command line gave it.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Glyph(String),
    Text(String),
}

pub struct Options {
    pub input: PathBuf,
    pub face: Option<String>,
    pub output: Option<PathBuf>,
    pub zoom: u32,
    pub ascii: bool,
    pub items: Vec<Item>,
}

const USAGE: &str = "Usage: uniform render -i DIR [--face ID] [-o OUT.png] [--zoom N] [--ascii] \
                     (-g GLYPH | -t TEXT)...";

pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut input = None;
    let mut face = None;
    let mut output = None;
    let mut zoom = 16;
    let mut ascii = false;
    let mut items = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or(format!("{arg} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--input" | "-i" => input = Some(PathBuf::from(value()?)),
            "--face" => face = Some(value()?),
            "--output" | "-o" => output = Some(PathBuf::from(value()?)),
            "--zoom" | "-z" => {
                zoom = value()?
                    .parse()
                    .ok()
                    .filter(|z| (1..=256).contains(z))
                    .ok_or(format!(
                        "--zoom takes a whole number from 1 to 256\n{USAGE}"
                    ))?
            }
            "--ascii" => ascii = true,
            "--glyph" | "-g" => items.push(Item::Glyph(value()?)),
            "--text" | "-t" => items.push(Item::Text(value()?)),
            _ => return Err(format!("Unknown render option: {arg}\n{USAGE}")),
        }
    }
    let input = input.ok_or(USAGE.to_string())?;
    if items.is_empty() {
        return Err(format!(
            "nothing to render: give at least one -g or -t\n{USAGE}"
        ));
    }
    if output.is_none() && !ascii {
        return Err(format!("give -o OUT.png, --ascii, or both\n{USAGE}"));
    }
    Ok(Options {
        input,
        face,
        output,
        zoom,
        ascii,
        items,
    })
}

/// The source of the roots that make every `-g` name reachable.
fn roots_source(items: &[Item]) -> String {
    let mut src = String::new();
    for (n, item) in items.iter().enumerate() {
        if let Item::Glyph(name) = item {
            src += &format!("glyph {} keep\nref {name}\n\n", root_name(n));
        }
    }
    src
}

/// One glyph placed on a row, in font units, the pen at the row's origin.
#[derive(Clone, Copy, Debug)]
struct Placed {
    gid: u32,
    x: f32,
    y: f32,
}

/// One rendered item: what was asked, what it shaped to, where the pen ended.
struct Row {
    label: String,
    glyphs: Vec<Placed>,
    advance: f32,
}

/// A flattened outline: closed polygons in font units, y up.
#[derive(Default)]
struct Polygons {
    contours: Vec<Vec<(f32, f32)>>,
    dx: f32,
    dy: f32,
}

impl Polygons {
    fn last(&mut self) -> &mut Vec<(f32, f32)> {
        if self.contours.is_empty() {
            self.contours.push(Vec::new());
        }
        self.contours.last_mut().unwrap()
    }

    fn cursor(&mut self) -> (f32, f32) {
        self.last().last().copied().unwrap_or((0.0, 0.0))
    }

    fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut pts = self.contours.iter().flatten();
        let &(x, y) = pts.next()?;
        Some(pts.fold((x, y, x, y), |(x0, y0, x1, y1), &(x, y)| {
            (x0.min(x), y0.min(y), x1.max(x), y1.max(y))
        }))
    }
}

/// Segments per curve: plenty at the sizes this draws, and a curve is rare in
/// this font (circles and polygons on demand).
const CURVE_STEPS: usize = 16;

impl OutlinePen for Polygons {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = (x + self.dx, y + self.dy);
        self.contours.push(vec![p]);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = (x + self.dx, y + self.dy);
        self.last().push(p);
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (x0, y0) = self.cursor();
        let (cx, cy, x, y) = (cx + self.dx, cy + self.dy, x + self.dx, y + self.dy);
        for i in 1..=CURVE_STEPS {
            let t = i as f32 / CURVE_STEPS as f32;
            let u = 1.0 - t;
            let p = (
                u * u * x0 + 2.0 * u * t * cx + t * t * x,
                u * u * y0 + 2.0 * u * t * cy + t * t * y,
            );
            self.last().push(p);
        }
    }
    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        let (x0, y0) = self.cursor();
        let (d, e) = (self.dx, self.dy);
        let (c0x, c0y, c1x, c1y, x, y) = (c0x + d, c0y + e, c1x + d, c1y + e, x + d, y + e);
        for i in 1..=CURVE_STEPS {
            let t = i as f32 / CURVE_STEPS as f32;
            let u = 1.0 - t;
            let (a, b, c, f) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            let p = (
                a * x0 + b * c0x + c * c1x + f * x,
                a * y0 + b * c0y + c * c1y + f * y,
            );
            self.last().push(p);
        }
    }
    fn close(&mut self) {}
}

/// Coverage in `0.0..=1.0`, one value per output pixel, from a signed-area
/// accumulation buffer (nonzero winding).
struct Coverage {
    w: usize,
    h: usize,
    acc: Vec<f32>,
}

impl Coverage {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            acc: vec![0.0; w * h + 4],
        }
    }

    /// One edge, in pixel coordinates with y down. `x` is clamped into
    /// `0..=w`, which keeps the winding of anything outside it correct; a
    /// write at `w` lands on the next row's first cell, which is where the
    /// running sum in [`Self::finish`] wants it.
    fn line(&mut self, p0: (f32, f32), p1: (f32, f32)) {
        let max_x = self.w as f32;
        let clamp = |(x, y): (f32, f32)| (x.clamp(0.0, max_x), y);
        let (p0, p1) = (clamp(p0), clamp(p1));
        if (p0.1 - p1.1).abs() <= f32::EPSILON {
            return;
        }
        let (dir, p0, p1) = if p0.1 < p1.1 {
            (1.0, p0, p1)
        } else {
            (-1.0, p1, p0)
        };
        let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
        let mut x = p0.0;
        if p0.1 < 0.0 {
            x -= p0.1 * dxdy;
        }
        let y_start = p0.1.max(0.0) as usize;
        let y_end = self.h.min(p1.1.ceil().max(0.0) as usize);
        for y in y_start..y_end {
            let row = y * self.w;
            let dy = ((y + 1) as f32).min(p1.1) - (y as f32).max(p0.1);
            let x_next = x + dxdy * dy;
            let d = dy * dir;
            let (x0, x1) = if x < x_next { (x, x_next) } else { (x_next, x) };
            let x0_floor = x0.floor();
            let x0i = x0_floor as usize;
            let x1_ceil = x1.ceil();
            let x1i = x1_ceil as usize;
            if x1i <= x0i + 1 {
                let xmf = 0.5 * (x + x_next) - x0_floor;
                self.acc[row + x0i] += d - d * xmf;
                self.acc[row + x0i + 1] += d * xmf;
            } else {
                let s = (x1 - x0).recip();
                let x0f = x0 - x0_floor;
                let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
                let x1f = x1 - x1_ceil + 1.0;
                let am = 0.5 * s * x1f * x1f;
                self.acc[row + x0i] += d * a0;
                if x1i == x0i + 2 {
                    self.acc[row + x0i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0f);
                    self.acc[row + x0i + 1] += d * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        self.acc[row + xi] += d * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    self.acc[row + x1i - 1] += d * (1.0 - a2 - am);
                }
                self.acc[row + x1i] += d * am;
            }
            x = x_next;
        }
    }

    fn fill(&mut self, polygons: &Polygons, to_px: impl Fn((f32, f32)) -> (f32, f32)) {
        for contour in &polygons.contours {
            for (i, &p) in contour.iter().enumerate() {
                let q = contour[(i + 1) % contour.len()];
                self.line(to_px(p), to_px(q));
            }
        }
    }

    fn finish(self) -> Vec<f32> {
        let mut sum = 0.0;
        self.acc[..self.w * self.h]
            .iter()
            .map(|a| {
                sum += a;
                sum.abs().min(1.0)
            })
            .collect()
    }
}

/// A row's frame, in whole font pixels: columns `x0..x1` from the pen origin,
/// rows from `top` (ascent, counted up from the baseline) down to `bottom`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    x0: i32,
    x1: i32,
    top: i32,
    bottom: i32,
}

impl Frame {
    fn width(&self) -> usize {
        (self.x1 - self.x0).max(1) as usize
    }
    fn height(&self) -> usize {
        (self.top - self.bottom).max(1) as usize
    }
}

/// Both drawings of a row, as polygons in font units.
struct Drawn {
    vector: Polygons,
    bitmap: Polygons,
    frame: Frame,
}

struct Font<'a> {
    font: FontRef<'a>,
    vector: Location,
    bitmap: Location,
    /// Font units per font pixel.
    unit: f32,
    ascent: f32,
    descent: f32,
}

impl Font<'_> {
    fn draw(&self, glyphs: &[Placed], at: &Location) -> Polygons {
        let outlines = self.font.outline_glyphs();
        let mut out = Polygons::default();
        for g in glyphs {
            let Some(outline) = outlines.get(GlyphId::new(g.gid)) else {
                continue;
            };
            out.dx = g.x;
            out.dy = g.y;
            let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::from(at));
            let _ = outline.draw(settings, &mut out);
        }
        out
    }

    fn drawn(&self, row: &Row) -> Drawn {
        let vector = self.draw(&row.glyphs, &self.vector);
        let bitmap = self.draw(&row.glyphs, &self.bitmap);
        // The frame covers the em box from the pen's start to its end, and
        // whatever ink lies outside it (a mark's, a negative bearing's).
        let (mut x0, mut y0, mut x1, mut y1) = (0.0f32, self.descent, row.advance, self.ascent);
        for b in [vector.bounds(), bitmap.bounds()].into_iter().flatten() {
            (x0, y0, x1, y1) = (x0.min(b.0), y0.min(b.1), x1.max(b.2), y1.max(b.3));
        }
        let px = |v: f32, up: bool| {
            let v = v / self.unit;
            // A hair of rounding error must not add a whole blank pixel.
            let v = if (v - v.round()).abs() < 1e-3 {
                v.round()
            } else {
                v
            };
            (if up { v.ceil() } else { v.floor() }) as i32
        };
        let frame = Frame {
            x0: px(x0, false),
            x1: px(x1, true),
            top: px(y1, true),
            bottom: px(y0, false),
        };
        Drawn {
            vector,
            bitmap,
            frame,
        }
    }
}

/// Coverage of `polygons` over `frame` at `zoom` output pixels per font pixel.
fn rasterize(polygons: &Polygons, frame: Frame, unit: f32, zoom: u32) -> Vec<f32> {
    let z = zoom as f32;
    let mut cov = Coverage::new(
        frame.width() * zoom as usize,
        frame.height() * zoom as usize,
    );
    cov.fill(polygons, |(x, y)| {
        (
            (x / unit - frame.x0 as f32) * z,
            (frame.top as f32 - y / unit) * z,
        )
    });
    cov.finish()
}

/// The bitmap build of one row, spelled as the source spells a grid.
fn ascii_rows(drawn: &Drawn, unit: f32) -> Vec<String> {
    // Sampled at zoom 4 and read at the cell's center, where a squared-off
    // cell is either fully inked or not at all.
    let zoom = 4;
    let cov = rasterize(&drawn.bitmap, drawn.frame, unit, zoom);
    let w = drawn.frame.width() * zoom as usize;
    (0..drawn.frame.height())
        .map(|r| {
            (0..drawn.frame.width())
                .map(|c| {
                    let i = (r * 4 + 2) * w + c * 4 + 2;
                    if cov[i] >= 0.5 { "@@" } else { ".." }
                })
                .collect()
        })
        .collect()
}

type Rgb = [u8; 3];

fn blend(dst: &mut Rgb, src: Rgb, alpha: f32) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d = (*d as f32 * (1.0 - alpha) + s as f32 * alpha).round() as u8;
    }
}

const PAPER: Rgb = [255, 255, 255];
const INK: Rgb = [0, 0, 0];
const PALE_INK: Rgb = [150, 180, 230];
const GRID: Rgb = [128, 128, 128];
const BASELINE: Rgb = [220, 40, 40];
const PEN: Rgb = [40, 90, 220];
const GAP: usize = 16;

/// The three panels of one row, side by side, as RGB rows.
fn panels(font: &Font, row: &Row, drawn: &Drawn, zoom: u32) -> (usize, usize, Vec<Rgb>) {
    let f = drawn.frame;
    let z = zoom as usize;
    let (pw, ph) = (f.width() * z, f.height() * z);
    let vector = rasterize(&drawn.vector, f, font.unit, zoom);
    let bitmap = rasterize(&drawn.bitmap, f, font.unit, zoom);
    let w = pw * 3 + GAP * 2;
    let mut img = vec![PAPER; w * ph];
    for panel in 0..3 {
        let left = panel * (pw + GAP);
        for y in 0..ph {
            for x in 0..pw {
                let i = y * pw + x;
                let px = &mut img[y * w + left + x];
                match panel {
                    0 => blend(px, INK, vector[i]),
                    1 => blend(px, INK, bitmap[i]),
                    _ => {
                        blend(px, PALE_INK, bitmap[i]);
                        blend(px, INK, vector[i] * 0.7);
                    }
                }
            }
        }
        let mut line = |x: usize, y: usize, color: Rgb, alpha: f32| {
            if x < pw && y < ph {
                blend(&mut img[y * w + left + x], color, alpha);
            }
        };
        if z >= 4 {
            for y in 0..ph {
                for x in 0..pw {
                    if x % z == 0 || y % z == 0 {
                        line(x, y, GRID, 0.35);
                    }
                }
            }
        }
        let baseline = (f.top * zoom as i32) as usize;
        for x in 0..pw {
            line(x, baseline, BASELINE, 0.8);
        }
        let end = (row.advance / font.unit).round() as i32;
        for pen in [0, end] {
            let x = ((pen - f.x0) * zoom as i32) as usize;
            for y in 0..ph {
                line(x, y, PEN, 0.8);
            }
        }
    }
    (w, ph, img)
}

fn write_png(path: &std::path::Path, rows: &[(usize, usize, Vec<Rgb>)]) -> Result<(), String> {
    let w = rows.iter().map(|r| r.0).max().unwrap_or(1);
    let h = rows.iter().map(|r| r.1).sum::<usize>() + GAP * rows.len().saturating_sub(1);
    let mut data = Vec::with_capacity(w * h * 3);
    for (n, (rw, rh, img)) in rows.iter().enumerate() {
        if n > 0 {
            data.extend(std::iter::repeat_n(PAPER, w * GAP).flatten());
        }
        for y in 0..*rh {
            data.extend(img[y * rw..(y + 1) * rw].iter().flatten());
            data.extend(std::iter::repeat_n(PAPER, w - rw).flatten());
        }
    }
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    writer
        .write_image_data(&data)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Shape and place every item. A name the font has no glyph for is an error
/// naming it, rather than a blank row.
fn rows(
    font: &Font,
    ttf: &[u8],
    glyph_names: &[String],
    items: &[Item],
    canonical: &[String],
) -> Result<Vec<Row>, String> {
    let mut gid_of = crate::hash::HashMap::default();
    for (gid, name) in glyph_names.iter().enumerate() {
        gid_of.entry(name.as_str()).or_insert(gid as u32);
    }
    let metrics = font
        .font
        .glyph_metrics(Size::unscaled(), LocationRef::from(&font.vector));
    let mut out = Vec::new();
    let mut canonical = canonical.iter();
    for item in items {
        out.push(match item {
            Item::Glyph(name) => {
                let gid = canonical
                    .next()
                    .and_then(|c| gid_of.get(c.as_str()))
                    .copied()
                    .ok_or(format!("no glyph named `{name}`"))?;
                Row {
                    label: name.clone(),
                    glyphs: vec![Placed {
                        gid,
                        x: 0.0,
                        y: 0.0,
                    }],
                    advance: metrics.advance_width(GlyphId::new(gid)).unwrap_or(0.0),
                }
            }
            Item::Text(text) => {
                let mut pen = 0.0;
                let mut glyphs = Vec::new();
                for g in crate::render::assert::shape_text(ttf, text, &[], None) {
                    glyphs.push(Placed {
                        gid: g.glyph_id as u32,
                        x: pen + g.x_offset as f32,
                        y: g.y_offset as f32,
                    });
                    pen += g.x_advance as f32;
                }
                let names: Vec<&str> = glyphs
                    .iter()
                    .map(|g| glyph_names.get(g.gid as usize).map_or("?", |s| s.as_str()))
                    .collect();
                Row {
                    label: format!("{text:?} = {}", names.join(" ")),
                    glyphs,
                    advance: pen,
                }
            }
        });
    }
    Ok(out)
}

/// Build the project with its roots and draw `items`: the PNG, the ASCII
/// text, or both. Returns the ASCII output rather than printing it, so the
/// tests can read it.
pub fn render(docs: &[Document], opts: &Options) -> Result<String, String> {
    let roots = crate::document_io::parse_document_from_str(
        &roots_source(&opts.items),
        PathBuf::from("<render>"),
    )
    .map_err(|e| format!("a -g name does not parse as a ref target: {e}"))?;
    let mut refs: Vec<&Document> = docs.iter().collect();
    refs.push(&roots);
    let faces = crate::faces::FaceSet::collect(&refs);
    let face = match &opts.face {
        None => faces.primary(),
        Some(id) => faces
            .faces
            .iter()
            .find(|f| &f.id == id)
            .ok_or(format!("no face named `{id}`"))?,
    };
    let mut names: Vec<String> = opts
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Glyph(name) => Some(name.clone()),
            Item::Text(_) => None,
        })
        .collect();
    let built =
        build_face_variable_keeping(&refs, face, &mut names).ok_or("the font did not build")?;

    let font_ref = FontRef::new(&built.ttf).map_err(|e| format!("reading the built font: {e}"))?;
    let axes = font_ref.axes();
    let metrics = font_ref.metrics(Size::unscaled(), LocationRef::default());
    let font = Font {
        vector: axes.location([("BMAP", 0.0)]),
        bitmap: axes.location([("BMAP", 1.0)]),
        unit: UNITS_PER_EM as f32 / built.height.max(1) as f32,
        ascent: metrics.ascent,
        descent: metrics.descent,
        font: font_ref,
    };

    let rows = rows(&font, &built.ttf, &built.glyph_names, &opts.items, &names)?;
    let drawn: Vec<Drawn> = rows.iter().map(|r| font.drawn(r)).collect();

    if let Some(path) = &opts.output {
        let images: Vec<_> = rows
            .iter()
            .zip(&drawn)
            .map(|(r, d)| panels(&font, r, d, opts.zoom))
            .collect();
        write_png(path, &images)?;
    }

    let mut ascii = String::new();
    for (row, d) in rows.iter().zip(&drawn) {
        let advance = row.advance / font.unit;
        // `x`/`y` are in pixels from the pen's origin, y up; the rows printed
        // below run from `y = top` down, so the baseline sits under row
        // `top - 1` of them.
        ascii += &format!(
            "{}: advance {advance}, x {}..{}, y {}..{} (baseline under row {})\n",
            row.label,
            d.frame.x0,
            d.frame.x1,
            d.frame.bottom,
            d.frame.top,
            d.frame.top - 1,
        );
        if opts.ascii {
            for line in ascii_rows(d, font.unit) {
                ascii += &line;
                ascii += "\n";
            }
            ascii += "\n";
        }
    }
    Ok(ascii)
}

/// `uniform render ARGS...`; returns the exit status.
pub fn run(args: &[String]) -> i32 {
    let opts = match parse_args(args) {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let (docs, errors) = crate::render::ttf_builder::load_docs_from_directory_checked(&opts.input);
    for error in &errors {
        let file = crate::document_io::file_label(&error.file);
        eprintln!("error: {file}:{}: {}", error.file_line, error.message);
    }
    if docs.is_empty() {
        eprintln!("No .unf files found in {}", opts.input.display());
        return 1;
    }
    match render(&docs, &opts) {
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
