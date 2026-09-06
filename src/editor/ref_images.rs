//! The reference chart strip a code point carries, drawn above the first
//! `glyph` line of the file that names it.
//!
//! `scripts/extract_ref_charts.py` cuts one wide, short PNG per han code point
//! out of the published code charts and the IVD charts — every source's
//! drawing of that character, side by side and labelled. This module is what
//! puts that strip in front of the person drawing the glyph, so that what the
//! sources show is on screen beside what the source file says.
//!
//! # Where the strips are
//!
//! `audit ref-image-path DIR` names the directory, relative to the file the
//! line is written in (see [`crate::audit`]); for Unison that is one line in
//! `Unison.unf`. Inside it a code point's strip is at
//! `DIR/<name minus its last three digits>/<name>.png`, where the name is the
//! code point in lowercase hexadecimal, at least four digits: U+4E00 is
//! `4/4e00.png` and U+2A6D6 is `2a/2a6d6.png`. That is the layout the script
//! writes and this module only reads it.
//!
//! The directory is read **once**, into the set of code points it holds, and
//! never again: the strips are generated output that does not change while the
//! editor runs, and the alternative — asking the filesystem whether a
//! particular strip exists — is a per-file round trip on the UI thread, which
//! is the one thing this codebase will not do (see `startup.rs`). Until that
//! one scan lands the editor shows no strips at all, which is also what a
//! source with no `audit ref-image-path` shows.
//!
//! # Which line a strip goes above
//!
//! A `glyph` line, and nothing else. A code point is spelled in a glyph name
//! as a run of `[a-f0-9]` four to six characters long, delimited by anything
//! outside `[a-z0-9]` — `han-4e00`, `han-4e00:8x16`, `ext-2a6d6-alt` — and the
//! run has to fall inside the *defining* name of a `glyph` line
//! ([`crate::editor::line_fields::FieldRole::GlyphDef`]), which is what the
//! strip is a reference for. A `ref` naming the same glyph is not a second
//! place to show it: it would put a chart strip between a glyph's header and
//! its own grid, and the drawing is not what a component line is about. A
//! comment mentioning the name is out for the same reason, and so — for free —
//! is the `U+4E00` of a `map` line, whose hexadecimal is uppercase and so is
//! never a candidate.
//!
//! The strip is drawn once per file, above the *first* `glyph` line naming the
//! code point, since a source that draws several variants of one character
//! needs the chart once.
//!
//! # What the drawing costs
//!
//! Nothing on the UI thread reads a file. A strip that comes into view is
//! *requested*, one worker thread reads and decodes it, and the frame that
//! finds it decoded uploads it as a texture. Until then the row is a
//! placeholder of exactly the height the strip will take, so scrolling never
//! waits for a decode and the layout never jumps once one lands. A strip that
//! has been off screen for a while is dropped again ([`RefImages::end_frame`]);
//! the row keeps its height, and coming back into view re-reads it.
//!
//! # The one fixed height
//!
//! [`REF_IMAGE_HEIGHT`] is what a strip row is, whatever the editor's zoom:
//! this is a photograph of a published chart, not part of the drawing, so
//! magnifying it with the grid would only blur it. One image pixel is one
//! point. A strip taller than the row — which the script never writes; its
//! strips are 87 or 101 pixels tall — is scaled down to fit rather than
//! cropped, since a strip from somewhere else is still worth seeing whole.
//! Wider than the row is the normal case, and that scrolls: drag the strip.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

use crate::document::DocLine;

/// The height of a strip row, in points, and so also the height above which a
/// strip is scaled down. See the module docs.
pub(crate) const REF_IMAGE_HEIGHT: f32 = 104.0;

/// Room kept above and below the strip inside its row.
pub(crate) const REF_IMAGE_PAD: f32 = 3.0;

/// The row a strip occupies, padding included.
pub(crate) const REF_IMAGE_ROW: f32 = REF_IMAGE_HEIGHT + 2.0 * REF_IMAGE_PAD;

/// How long a strip nobody has drawn is kept in video memory. Long enough that
/// scrolling back and forth over the same block re-uses the texture, short
/// enough that a walk through a whole file does not accumulate one texture per
/// glyph.
const EVICT_AFTER: std::time::Duration = std::time::Duration::from_secs(20);

/// The largest strip that will be decoded at all, as a guard against a stray
/// file in the directory: a chart strip is a few hundred kilopixels.
const MAX_PIXELS: usize = 64 << 20;

/// What the paint pass gets back for one code point.
pub(crate) enum RefImage {
    /// Ready to draw, at its natural size in points (one image pixel to a
    /// point).
    Ready {
        texture: egui::TextureId,
        size: egui::Vec2,
    },
    /// Being read, or waiting to be. The row is a placeholder this frame.
    Pending,
}

enum Entry {
    /// Queued for the worker, or in its hands, for the theme it carries.
    Requested(bool),
    /// Read and decoded, waiting for a frame to upload it. Carries the theme
    /// it was decoded for; see [`decode`].
    Decoded(egui::ColorImage, bool),
    Ready {
        texture: egui::TextureHandle,
        size: egui::Vec2,
        dark: bool,
        used: std::time::Instant,
    },
    /// The file is in the index but could not be read or decoded. Kept so the
    /// worker is not asked for it once a frame forever.
    Failed,
}

struct Inner {
    /// The code points the directory holds; `None` until the one scan lands.
    index: Option<HashSet<u32>>,
    /// Bumped when the index lands, which is the one thing here that changes
    /// what the *layout* is. Textures coming and going do not.
    generation: u64,
    entries: HashMap<u32, Entry>,
}

/// The strips of one font directory: the index, the textures, and the worker
/// that reads them.
///
/// Cloneable and shared — every editor pane draws from one store, since the
/// directory is one directory. Cheap to clone (one `Arc`).
#[derive(Clone)]
pub(crate) struct RefImages {
    root: Arc<PathBuf>,
    inner: Arc<Mutex<Inner>>,
    /// Requests to the worker. `None` in a store built for a test, which
    /// serves an index and nothing else.
    requests: Option<mpsc::Sender<(u32, bool)>>,
}

impl RefImages {
    /// Opens the store over `root` and starts the two threads it needs: one
    /// that reads the directory once, and one that reads a strip whenever a
    /// frame asks for one.
    ///
    /// `ctx` is repainted whenever either of them has something new to show,
    /// which is what makes a strip appear without the reader touching
    /// anything.
    pub(crate) fn spawn(root: PathBuf, ctx: egui::Context) -> Self {
        let inner = Arc::new(Mutex::new(Inner {
            index: None,
            generation: 0,
            entries: HashMap::new(),
        }));
        let (tx, rx) = mpsc::channel::<(u32, bool)>();
        let store = Self {
            root: Arc::new(root),
            inner,
            requests: Some(tx),
        };

        {
            let (root, inner, ctx) = (
                Arc::clone(&store.root),
                Arc::clone(&store.inner),
                ctx.clone(),
            );
            std::thread::Builder::new()
                .name("ref-image-index".into())
                .spawn(move || {
                    let index = scan_index(&root);
                    if let Ok(mut guard) = inner.lock() {
                        guard.index = Some(index);
                        guard.generation += 1;
                    }
                    ctx.request_repaint();
                })
                .ok();
        }
        {
            let (root, inner) = (Arc::clone(&store.root), Arc::clone(&store.inner));
            std::thread::Builder::new()
                .name("ref-image-loader".into())
                .spawn(move || {
                    // One thread, in request order: the frame that asked is
                    // the frame on screen, and a strip is a few milliseconds
                    // of decoding.
                    for (cp, dark) in rx {
                        // Dropped while it queued (scrolled away, theme
                        // switched): whatever asked will ask again.
                        let wanted = matches!(
                            inner.lock().map(|g| matches!(
                                g.entries.get(&cp),
                                Some(Entry::Requested(d)) if *d == dark
                            )),
                            Ok(true)
                        );
                        if !wanted {
                            continue;
                        }
                        let loaded = load(&image_path(&root, cp), dark);
                        if let Ok(mut guard) = inner.lock() {
                            let entry = match loaded {
                                Some(image) => Entry::Decoded(image, dark),
                                None => Entry::Failed,
                            };
                            guard.entries.insert(cp, entry);
                        }
                        ctx.request_repaint();
                    }
                })
                .ok();
        }
        store
    }

    /// A store that serves `index` and reads nothing, for tests.
    #[cfg(test)]
    pub(crate) fn for_test(index: HashSet<u32>) -> Self {
        Self {
            root: Arc::new(PathBuf::from("/nonexistent")),
            inner: Arc::new(Mutex::new(Inner {
                index: Some(index),
                generation: 1,
                entries: HashMap::new(),
            })),
            requests: None,
        }
    }

    /// Puts a strip of `size` in the store as if it had just been read, for a
    /// test that needs the drawn path rather than the placeholder.
    #[cfg(test)]
    pub(crate) fn preload_for_test(&self, cp: u32, size: [usize; 2], dark: bool) {
        let image = egui::ColorImage {
            size,
            pixels: vec![egui::Color32::WHITE; size[0] * size[1]],
        };
        if let Ok(mut guard) = self.inner.lock() {
            guard.entries.insert(cp, Entry::Decoded(image, dark));
        }
    }

    /// Bumped when the set of code points with a strip changes — which is
    /// once, when the directory scan lands. The view cache keys on it, since
    /// it is what decides which rows exist.
    pub(crate) fn generation(&self) -> u64 {
        self.inner.lock().map_or(0, |g| g.generation)
    }

    /// The strip of `cp`, requesting it if this is the first frame to ask.
    ///
    /// `dark` is the theme it is drawn in: the strips are black on white, so a
    /// dark editor inverts them. A theme switch invalidates what is loaded,
    /// which is a re-read of the few strips on screen.
    pub(crate) fn image(&self, ctx: &egui::Context, cp: u32, dark: bool) -> RefImage {
        let Ok(mut guard) = self.inner.lock() else {
            return RefImage::Pending;
        };
        match guard.entries.get_mut(&cp) {
            Some(Entry::Ready {
                texture,
                size,
                dark: was_dark,
                used,
            }) if *was_dark == dark => {
                *used = std::time::Instant::now();
                return RefImage::Ready {
                    texture: texture.id(),
                    size: *size,
                };
            }
            // Loaded for the other theme: drop it and ask again.
            Some(Entry::Ready { .. }) => {
                guard.entries.remove(&cp);
            }
            Some(Entry::Decoded(_, was_dark)) if *was_dark != dark => {
                guard.entries.remove(&cp);
            }
            Some(Entry::Decoded(..)) => {
                let Some(Entry::Decoded(image, _)) = guard.entries.remove(&cp) else {
                    unreachable!()
                };
                let size = egui::vec2(image.width() as f32, image.height() as f32);
                // Uploading is a copy into egui's texture queue, not a round
                // trip to the GPU, so it is the paint pass's to do.
                let texture = ctx.load_texture(
                    format!("ref-image-{cp:04x}"),
                    image,
                    egui::TextureOptions::LINEAR,
                );
                let id = texture.id();
                guard.entries.insert(
                    cp,
                    Entry::Ready {
                        texture,
                        size,
                        dark,
                        used: std::time::Instant::now(),
                    },
                );
                return RefImage::Ready { texture: id, size };
            }
            Some(Entry::Requested(was_dark)) if *was_dark == dark => return RefImage::Pending,
            // Asked for under the other theme; ask again under this one.
            Some(Entry::Requested(_)) => {}
            Some(Entry::Failed) => return RefImage::Pending,
            None => {}
        }
        guard.entries.insert(cp, Entry::Requested(dark));
        drop(guard);
        if let Some(tx) = &self.requests {
            let _ = tx.send((cp, dark));
        }
        RefImage::Pending
    }

    /// Drops the textures nothing has drawn for a while. Called once a frame
    /// by the host; a strip that comes back into view is read again.
    pub(crate) fn end_frame(&self) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        let now = std::time::Instant::now();
        guard.entries.retain(|_, entry| match entry {
            Entry::Ready { used, .. } => now.duration_since(*used) < EVICT_AFTER,
            _ => true,
        });
    }

    /// The strip rows one file's lines call for: the code point and the
    /// `glyph` line its strip is drawn above, in line order, one row per code
    /// point.
    ///
    /// Cheap enough to run on every view rebuild: only a `glyph` line is
    /// looked at at all, its characters are scanned for a hex-shaped run, and
    /// only one whose strip actually exists is tokenized.
    pub(crate) fn rows_for(&self, lines: &[DocLine]) -> Vec<(usize, u32)> {
        let Ok(guard) = self.inner.lock() else {
            return Vec::new();
        };
        let Some(index) = guard.index.as_ref() else {
            return Vec::new();
        };
        let mut seen: HashSet<u32> = HashSet::new();
        let mut rows = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let DocLine::Text(text) = line else { continue };
            if !text.trim_start().starts_with("glyph ") {
                continue;
            }
            let mut candidates = candidate_codepoints(text);
            candidates.retain(|&(_, _, cp)| index.contains(&cp) && !seen.contains(&cp));
            if candidates.is_empty() {
                continue;
            }
            for (_, _, cp) in in_glyph_definitions(text, &candidates) {
                if seen.insert(cp) {
                    rows.push((i, cp));
                }
            }
        }
        rows
    }
}

/// Draws one strip's row: the strip itself where it has been read, a
/// placeholder naming the code point where it has not, and the drag that
/// scrolls a strip wider than the row.
///
/// Returns whether a drag on this strip is in flight, which the caller uses to
/// keep the same gesture from also selecting text.
///
/// The scroll offset is the caller's (one per pane, per code point) and is
/// clamped here, since only this pass knows how far the strip actually
/// overflows — and it only knows that once the strip has been read, which is
/// why a placeholder swallows the drag rather than moving anything.
pub(crate) fn paint_strip(
    ui: &egui::Ui,
    painter: &egui::Painter,
    row: egui::Rect,
    store: &RefImages,
    cp: u32,
    scroll: &mut f32,
    id: egui::Id,
) -> bool {
    let pal = crate::editor::colors::Palette::get(ui);
    painter.rect_filled(row, 0.0, pal.ref_image_bg);
    let response = ui.interact(row, id, egui::Sense::drag());
    match store.image(ui.ctx(), cp, ui.visuals().dark_mode) {
        RefImage::Ready { texture, size } => {
            // Its own size, in points, unless it is taller than the row — the
            // strips this was written for never are; see the module docs.
            let factor = (REF_IMAGE_HEIGHT / size.y).min(1.0);
            let drawn = size * factor;
            let overflow = (drawn.x - row.width()).max(0.0);
            if response.dragged() {
                *scroll -= response.drag_delta().x;
            }
            *scroll = scroll.clamp(0.0, overflow);
            if overflow > 0.0 {
                ui.ctx().set_cursor_icon(if response.dragged() {
                    egui::CursorIcon::Grabbing
                } else if response.hovered() {
                    egui::CursorIcon::Grab
                } else {
                    egui::CursorIcon::Default
                });
            }
            let at = egui::Rect::from_min_size(
                egui::pos2(row.left() - *scroll, row.top() + REF_IMAGE_PAD),
                drawn,
            );
            painter
                .with_clip_rect(row.intersect(painter.clip_rect()))
                .image(
                    texture,
                    at,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
        }
        RefImage::Pending => {
            painter.text(
                egui::pos2(row.left() + REF_IMAGE_PAD * 2.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                format!("U+{cp:04X}"),
                egui::FontId::monospace(12.0),
                pal.ref_image_pending,
            );
        }
    }
    response.dragged()
}

/// The strip of `cp`: `<root>/<name minus its last three digits>/<name>.png`,
/// the name being lowercase hexadecimal padded to four digits. This is
/// `out_path` in `scripts/extract_ref_charts.py`, and the two have to agree.
fn image_path(root: &Path, cp: u32) -> PathBuf {
    let name = format!("{cp:04x}");
    let prefix = &name[..name.len() - 3];
    root.join(prefix).join(format!("{name}.png"))
}

/// Every code point the directory holds a strip for, from one pass over it.
/// Anything not shaped like a strip is ignored rather than reported: this is
/// generated output, and a directory that is not there at all is the ordinary
/// case for a checkout that has never run the script.
fn scan_index(root: &Path) -> HashSet<u32> {
    let mut found = HashSet::new();
    let Ok(dirs) = std::fs::read_dir(root) else {
        return found;
    };
    for dir in dirs.flatten() {
        if !dir.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name();
            let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".png")) else {
                continue;
            };
            // Written by `{cp:04x}`, so anything else in the directory — a
            // half-written temporary, an unrelated file — is not a strip.
            if stem.len() < 4 || !stem.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            if let Ok(cp) = u32::from_str_radix(stem, 16) {
                found.insert(cp);
            }
        }
    }
    found
}

/// Reads and decodes one strip, inverted for a dark editor. `None` for
/// anything that cannot be read as a PNG this module can draw.
fn load(path: &Path, dark: bool) -> Option<egui::ColorImage> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    // Whatever the file carries — a grayscale strip is what the script writes,
    // but a palette or 16-bit one still has to come out as 8-bit color.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let size = reader.output_buffer_size()?;
    if size > MAX_PIXELS {
        return None;
    }
    let mut buf = vec![0u8; size];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        // `normalize_to_color8` expands a palette, so this is unreachable.
        png::ColorType::Indexed => return None,
    };
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = &buf[y * info.line_size..][..w * channels];
        for x in 0..w {
            let px = &row[x * channels..][..channels];
            let (r, g, b, a) = match channels {
                1 => (px[0], px[0], px[0], 255),
                2 => (px[0], px[0], px[0], px[1]),
                3 => (px[0], px[1], px[2], 255),
                _ => (px[0], px[1], px[2], px[3]),
            };
            // The charts are black on white; a dark editor gets the negative
            // rather than a sheet of paper in the middle of the document.
            let (r, g, b) = if dark {
                (255 - r, 255 - g, 255 - b)
            } else {
                (r, g, b)
            };
            pixels.push(egui::Color32::from_rgba_unmultiplied(r, g, b, a));
        }
    }
    Some(egui::ColorImage {
        size: [w, h],
        pixels,
    })
}

/// Every run of `[a-f0-9]` four to six characters long that a whole
/// `[^a-z0-9]`-delimited part of the line consists of, as
/// `(char start, char end, code point)`.
///
/// The delimiter set is what makes the hexadecimal of a `map U+4E00` line a
/// non-candidate: `U` and the uppercase digits are outside `[a-z0-9]`, so
/// `U+4E00` is not one part but several, none of them four hex digits long.
fn candidate_codepoints(text: &str) -> Vec<(usize, usize, u32)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut all_hex = true;
    let mut len = 0usize;
    let push = |start: usize, len: usize, all_hex: bool, out: &mut Vec<_>, text: &str| {
        if !all_hex || !(4..=6).contains(&len) {
            return;
        }
        let part: String = text.chars().skip(start).take(len).collect();
        if let Ok(cp) = u32::from_str_radix(&part, 16)
            && char::from_u32(cp).is_some()
        {
            out.push((start, start + len, cp));
        }
    };
    for (i, c) in text.chars().enumerate() {
        let in_part = c.is_ascii_lowercase() || c.is_ascii_digit();
        if !in_part {
            push(start, len, all_hex, &mut out, text);
            len = 0;
            all_hex = true;
            start = i + 1;
            continue;
        }
        if len == 0 {
            start = i;
        }
        all_hex &= c.is_ascii_hexdigit();
        len += 1;
    }
    push(start, len, all_hex, &mut out, text);
    out
}

/// The candidates that fall inside the name a `glyph` line *declares*.
///
/// The classification is [`crate::editor::line_fields`]'s, so which token that
/// is stays one question with one answer: the alias form `glyph A = B` names
/// `A` here and refers to `B`, and only `A` carries the strip.
fn in_glyph_definitions(
    text: &str,
    candidates: &[(usize, usize, u32)],
) -> Vec<(usize, usize, u32)> {
    use crate::editor::line_fields::FieldRole;
    let fields = crate::editor::line_fields::classify_line(text);
    candidates
        .iter()
        .copied()
        .filter(|&(start, end, _)| {
            fields
                .iter()
                .any(|f| f.role == FieldRole::GlyphDef && f.col_start <= start && end <= f.col_end)
        })
        .collect()
}

#[cfg(test)]
#[path = "ref_images_tests.rs"]
mod ref_images_tests;
