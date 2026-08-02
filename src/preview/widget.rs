//! The shaped preview field: a small multi-line text editor whose text is
//! drawn by shaping it with the built font and rasterizing the glyphs.
//!
//! The *text model* is the document editor's: a `Vec<DocLine>` (all
//! `DocLine::Text` here), a [`Caret`], a selection anchor and an
//! [`UndoStack`], edited through [`crate::editor::caret`],
//! [`crate::editor::editing`] and — the whole point — the same key handler,
//! [`doc_input::handle_text_keys`]. Everything the editor's Normal mode does
//! with a keyboard therefore works here too, down to word motions, Cmd+arrow
//! home/end, multi-line paste and the copy-the-whole-line-when-nothing-is-
//! selected rule.
//!
//! What is *not* shared is the layout: the editor lays text out in a monospace
//! grid, this widget lays out one shaped run per line, so line breaks, caret
//! placement and hit testing are computed here from [`ClusterSpan`]s.

use crate::document::DocLine;
use crate::edit_menu::{EditAction, EditMenuCaps};
use crate::editor::caret::{self, Caret};
use crate::editor::doc_input::{self, TextEdit};
use crate::editor::undo::UndoStack;
use crate::preview::cluster::{self, ClusterSpan};
use crate::preview::rasterizer::GlyphCache;
use crate::preview::{self, ShapedGlyph, ShaperBackend};

/// Padding from the field's edges to the first baseline's origin.
const LEFT_PAD: f32 = 16.0;
const TOP_PAD: f32 = 8.0;

/// Baseline-to-baseline distance for a given font size. The font's own metrics
/// are not consulted on purpose: the preview draws whatever face is selected,
/// and a stable, size-proportional rhythm keeps the caret and hit testing
/// simple to reason about.
fn line_height(px_size: f32) -> f32 {
    (px_size * 1.4).round().max(px_size + 4.0)
}

pub struct ShapedPreviewState {
    /// The text being previewed. Always `DocLine::Text`; the grid arms of the
    /// shared editing code are unreachable from here.
    lines: Vec<DocLine>,
    cursor: Caret,
    selection_anchor: Option<Caret>,
    undo: UndoStack,
    pub backends: Vec<Box<dyn ShaperBackend>>,
    pub selected_backend: usize,
    pub glyph_cache: GlyphCache,
    pub color_font: bool,
    shaped: Option<ShapedDoc>,
    last_error: Option<String>,
    preedit: String,
    /// Which keys the IME owns while it composes; see
    /// [`doc_input::ImeKeyGuard`].
    ime_guard: doc_input::ImeKeyGuard,
    /// The Ctrl+K code point popup, and the screen position it was opened at.
    /// The anchor is frozen on open so the popup does not walk sideways as
    /// the preedit it drives grows.
    codepoint: Option<(CodepointPopup, egui::Pos2)>,
    has_focus: bool,
    last_rect: Option<egui::Rect>,
    /// Set when the caret moved or the text changed, so the next frame scrolls
    /// it back into view.
    scroll_to_caret: bool,
}

/// One shaped line, cached until its display text (or a shaping parameter)
/// changes. Editing one line therefore re-shapes only that line.
struct ShapedLine {
    text: String,
    glyphs: Vec<ShapedGlyph>,
    clusters: Vec<ClusterSpan>,
    width: f32,
    /// Char range of the IME preedit within `text`, on the line that has one.
    preedit_char_range: Option<(usize, usize)>,
}

struct ShapedDoc {
    font_gen: u64,
    backend_idx: usize,
    px_size: f32,
    lines: Vec<ShapedLine>,
}

impl ShapedPreviewState {
    pub fn new() -> Self {
        Self {
            lines: vec![DocLine::Text(String::new())],
            cursor: Caret::zero(),
            selection_anchor: None,
            undo: UndoStack::new(),
            backends: preview::available_backends(),
            selected_backend: 0,
            glyph_cache: GlyphCache::new(),
            color_font: true,
            shaped: None,
            last_error: None,
            preedit: String::new(),
            ime_guard: Default::default(),
            codepoint: None,
            has_focus: false,
            last_rect: None,
            scroll_to_caret: false,
        }
    }

    pub fn is_focused(&self) -> bool {
        self.has_focus
    }

    /// The whole preview text, lines joined by `\n`.
    #[allow(
        dead_code,
        reason = "the field's text API; only the tests read it today"
    )]
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.as_text().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Replaces the text outright, putting the caret at its end. Undo history
    /// is dropped: this is not an edit the user made.
    #[allow(
        dead_code,
        reason = "the field's text API; only the tests write it today"
    )]
    pub fn set_text(&mut self, text: &str) {
        self.lines = text
            .split('\n')
            .map(|l| DocLine::Text(l.replace('\r', "")))
            .collect();
        if self.lines.is_empty() {
            self.lines.push(DocLine::Text(String::new()));
        }
        self.undo = UndoStack::new();
        self.selection_anchor = None;
        self.cursor = caret::doc_end(&self.lines);
        self.shaped = None;
    }

    /// The screen rect the preview text occupied on the last frame it was
    /// drawn, or `None` if it was not drawn (no font, shaping error). The app
    /// uses it to route Cmd/Ctrl + wheel to whatever the pointer is over.
    pub fn last_rect(&self) -> Option<egui::Rect> {
        self.last_rect
    }

    pub fn invalidate_font(&mut self, font_gen: u64) {
        self.glyph_cache.invalidate_if_changed(font_gen);
        if let Some(ref shaped) = self.shaped
            && shaped.font_gen != font_gen
        {
            self.shaped = None;
        }
    }

    /// The text of line `idx` as it is *displayed*: the line itself, with the
    /// IME preedit spliced in at the caret on the caret's line.
    fn display_line(&self, idx: usize) -> (String, Option<(usize, usize)>) {
        let text = self.lines[idx].as_text().unwrap_or_default();
        if self.preedit.is_empty() || idx != self.cursor.line {
            return (text.to_string(), None);
        }
        let byte = caret::char_to_byte(text, self.cursor.col);
        let preedit_len = self.preedit.chars().count();
        (
            format!("{}{}{}", &text[..byte], self.preedit, &text[byte..]),
            Some((self.cursor.col, self.cursor.col + preedit_len)),
        )
    }

    fn ensure_shaped(&mut self, font_data: &[u8], font_gen: u64, px_size: f32) {
        let params_match = self.shaped.as_ref().is_some_and(|s| {
            s.font_gen == font_gen && s.backend_idx == self.selected_backend && s.px_size == px_size
        });
        let mut cached: Vec<Option<ShapedLine>> = if params_match {
            self.shaped
                .take()
                .map(|s| s.lines.into_iter().map(Some).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let mut out: Vec<ShapedLine> = Vec::with_capacity(self.lines.len());
        let backend = &self.backends[self.selected_backend];

        for idx in 0..self.lines.len() {
            let (display, preedit_range) = self.display_line(idx);

            let reusable = cached
                .get_mut(idx)
                .and_then(|slot| {
                    slot.as_ref()
                        .is_some_and(|c| c.text == display && c.preedit_char_range == preedit_range)
                        .then(|| slot.take())
                })
                .flatten();
            if let Some(line) = reusable {
                out.push(line);
                continue;
            }

            if display.is_empty() {
                out.push(ShapedLine {
                    text: display,
                    glyphs: Vec::new(),
                    clusters: Vec::new(),
                    width: 0.0,
                    preedit_char_range: preedit_range,
                });
                continue;
            }

            match preview::shape_text(backend.as_ref(), font_data, &display, 1024, &[]) {
                Ok(glyphs) => {
                    let total_chars = display.chars().count();
                    let mut clusters = cluster::build_clusters(&glyphs, px_size);
                    cluster::finalize_clusters(&mut clusters, total_chars);
                    let width = clusters.last().map_or(0.0, |c| c.pen_x + c.advance);
                    out.push(ShapedLine {
                        text: display,
                        glyphs,
                        clusters,
                        width,
                        preedit_char_range: preedit_range,
                    });
                }
                Err(e) => {
                    self.last_error = Some(e.0);
                    self.shaped = None;
                    return;
                }
            }
        }

        self.last_error = None;
        self.shaped = Some(ShapedDoc {
            font_gen,
            backend_idx: self.selected_backend,
            px_size,
            lines: out,
        });
    }

    pub fn show_engine_combo(&mut self, ui: &mut egui::Ui) {
        ui.label("Engine:");
        let current_name = self
            .backends
            .get(self.selected_backend)
            .map(|b| b.name())
            .unwrap_or("none");
        egui::ComboBox::from_id_salt("shaper_backend")
            .selected_text(current_name)
            .show_ui(ui, |ui| {
                for (i, backend) in self.backends.iter().enumerate() {
                    ui.selectable_value(&mut self.selected_backend, i, backend.name());
                }
            });
    }

    fn selection_range_sorted(&self) -> Option<(Caret, Caret)> {
        let (lo, hi) = caret::selection_range(self.cursor, self.selection_anchor)?;
        // Clamped: every mutation is supposed to keep the anchor and the caret
        // within the text, but a stale index must degrade into a shorter
        // selection rather than into a slicing panic.
        let lo = caret::clamp(&self.lines, lo);
        let hi = caret::clamp(&self.lines, hi);
        if lo == hi { None } else { Some((lo, hi)) }
    }

    /// The status-bar line for the selection: the selected code points, with
    /// one code point of context on either side. Context comes from the
    /// selection's own line, so it does not silently jump a line break.
    pub fn selection_codepoints_label(&self) -> Option<String> {
        let (lo, hi) = self.selection_range_sorted()?;
        let selected: Vec<char> = caret::extract_text(&self.lines, lo, hi).chars().collect();
        if selected.is_empty() {
            return None;
        }
        let lo_line: Vec<char> = self.lines[lo.line]
            .as_text()
            .unwrap_or_default()
            .chars()
            .collect();
        let hi_line: Vec<char> = self.lines[hi.line]
            .as_text()
            .unwrap_or_default()
            .chars()
            .collect();

        let mut parts = Vec::new();

        if lo.col >= 2 {
            parts.push("\u{2026} ".to_string());
        }
        if lo.col >= 1 {
            parts.push(format!("{:04X} ", lo_line[lo.col - 1] as u32));
        }

        parts.push("[".to_string());
        for (i, &ch) in selected.iter().enumerate() {
            if i > 0 {
                parts.push(" ".to_string());
            }
            parts.push(format!("{:04X}", ch as u32));
        }
        parts.push("]".to_string());

        if hi.col < hi_line.len() {
            parts.push(format!(" {:04X}", hi_line[hi.col] as u32));
        }
        if hi.col + 1 < hi_line.len() {
            parts.push(" \u{2026}".to_string());
        }

        Some(parts.concat())
    }

    /// The range Copy/Cut act on: the selection, or the whole caret line when
    /// there is none — the document editor's rule.
    fn copy_range(&self) -> (Caret, Caret) {
        if let Some(range) = self.selection_range_sorted() {
            return range;
        }
        let line = self.cursor.line;
        let lo = Caret::new(line, 0);
        let hi = if line + 1 < self.lines.len() {
            Caret::new(line + 1, 0)
        } else {
            Caret::new(line, caret::line_char_len(&self.lines, line))
        };
        (lo, hi)
    }

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    pub fn has_selection(&self) -> bool {
        self.selection_range_sorted().is_some()
    }

    pub fn edit_menu_caps(&self) -> EditMenuCaps {
        EditMenuCaps {
            can_undo: self.can_undo(),
            can_redo: self.can_redo(),
            has_selection: self.has_selection(),
            can_edit: true,
        }
    }

    fn perform_undo(&mut self) {
        if let Some(c) = self.undo.undo(&mut self.lines) {
            self.cursor = caret::clamp(&self.lines, c);
            self.selection_anchor = None;
            self.scroll_to_caret = true;
        }
    }

    fn perform_redo(&mut self) {
        if let Some(c) = self.undo.redo(&mut self.lines) {
            self.cursor = caret::clamp(&self.lines, c);
            self.selection_anchor = None;
            self.scroll_to_caret = true;
        }
    }

    pub fn apply_edit_action(&mut self, action: EditAction, ctx: &egui::Context) {
        match action {
            EditAction::None => {}
            EditAction::Undo => self.perform_undo(),
            EditAction::Redo => self.perform_redo(),
            EditAction::Cut => {
                let (lo, hi) = self.copy_range();
                let text = caret::extract_text(&self.lines, lo, hi);
                if !text.is_empty() {
                    ctx.copy_text(text);
                }
                self.cursor = crate::editor::editing::delete_selection(
                    &mut self.lines,
                    &mut self.undo,
                    lo,
                    hi,
                );
                self.selection_anchor = None;
                self.scroll_to_caret = true;
            }
            EditAction::Copy => {
                let (lo, hi) = self.copy_range();
                let text = caret::extract_text(&self.lines, lo, hi);
                if !text.is_empty() {
                    ctx.copy_text(text);
                }
            }
            EditAction::Paste => {
                if let Ok(mut clip) = arboard::Clipboard::new()
                    && let Ok(text) = clip.get_text()
                    && !text.is_empty()
                {
                    doc_input::paste_text(
                        &mut self.lines,
                        &mut self.undo,
                        &mut self.cursor,
                        self.selection_anchor.take(),
                        &text,
                    );
                    self.scroll_to_caret = true;
                }
            }
            EditAction::Delete => self.delete_selection(),
            EditAction::SelectAll => {
                self.selection_anchor = Some(Caret::zero());
                self.cursor = caret::doc_end(&self.lines);
            }
        }
    }

    fn delete_selection(&mut self) {
        if let Some((lo, hi)) = self.selection_range_sorted() {
            self.cursor =
                crate::editor::editing::delete_selection(&mut self.lines, &mut self.undo, lo, hi);
            self.selection_anchor = None;
            self.scroll_to_caret = true;
        }
    }

    fn insert_at_caret(&mut self, s: &str) {
        if self.selection_anchor.is_some() {
            self.delete_selection();
        }
        self.cursor =
            crate::editor::editing::insert_str(&mut self.lines, &mut self.undo, self.cursor, s);
        self.scroll_to_caret = true;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        font_data: Option<&(Vec<u8>, Vec<u8>)>,
        font_gen: u64,
        px_size: f32,
    ) {
        let Some(font_pair) = font_data else {
            self.has_focus = false;
            self.last_rect = None;
            ui.label("No font built yet.");
            return;
        };

        self.ensure_shaped(&font_pair.1, font_gen, px_size);

        if let Some(ref err) = self.last_error {
            self.has_focus = false;
            self.last_rect = None;
            ui.colored_label(egui::Color32::RED, format!("Shaping error: {err}"));
            return;
        }

        let viewport = egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.show_field(ui, font_pair, px_size);
            });
        self.last_rect = Some(viewport.inner_rect);
    }

    /// The field itself, inside the scroll area's inner `Ui`.
    fn show_field(&mut self, ui: &mut egui::Ui, font_pair: &(Vec<u8>, Vec<u8>), px_size: f32) {
        let row_h = line_height(px_size);
        let line_count = self.lines.len();
        let content_w = self
            .shaped
            .as_ref()
            .map_or(0.0, |s| s.lines.iter().fold(0.0f32, |m, l| m.max(l.width)))
            + LEFT_PAD * 2.0;
        let content_h = TOP_PAD * 2.0 + row_h * line_count as f32;
        let desired = egui::vec2(
            content_w.max(ui.available_width()),
            content_h.max(ui.available_height()),
        );

        let widget_bg = ui.visuals().extreme_bg_color;
        let widget_stroke = ui.visuals().widgets.noninteractive.bg_stroke;

        let (response, painter) =
            ui.allocate_painter(desired, egui::Sense::click().union(egui::Sense::drag()));
        let rect = response.rect;

        painter.rect(
            rect,
            2.0,
            widget_bg,
            widget_stroke,
            egui::epaint::StrokeKind::Inside,
        );

        let origin_x = rect.left() + LEFT_PAD;
        let first_baseline = rect.top() + TOP_PAD + px_size;
        let baseline_of = |line: usize| first_baseline + row_h * line as f32;

        let focus = response.has_focus();
        self.has_focus = focus;

        if focus || response.clicked() {
            response.request_focus();
        }

        if focus {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: false,
                        tab: true,
                    },
                );
            });
        }

        let caret_x_at = |state: &Self, c: Caret| -> f32 {
            let Some(shaped) = state.shaped.as_ref() else {
                return origin_x;
            };
            let Some(line) = shaped.lines.get(c.line) else {
                return origin_x;
            };
            let display_col = committed_to_display(c.col, line.preedit_char_range);
            origin_x + cluster::caret_x(&line.clusters, display_col)
        };

        // Where a caret-anchored popup would go: just under the caret.
        let caret_screen = egui::pos2(
            caret_x_at(self, self.cursor),
            baseline_of(self.cursor.line) + 6.0,
        );

        if focus {
            let before = self.cursor;
            let rows_per_page = (ui.clip_rect().height() / row_h).floor().max(1.0) as usize;
            self.handle_input(ui, caret_screen, rows_per_page);
            if self.cursor != before {
                self.scroll_to_caret = true;
            }
            let ime_rect = egui::Rect::from_min_size(
                egui::pos2(caret_screen.x, baseline_of(self.cursor.line) - px_size),
                egui::vec2(16.0, px_size + 4.0),
            );
            ui.ctx().output_mut(|o| {
                o.ime = Some(egui::output::IMEOutput {
                    rect: ime_rect,
                    cursor_rect: ime_rect,
                })
            });
        }

        // Outside the focus check: while the popup is open it, not the
        // preview, holds the keyboard.
        self.show_codepoint_popup(ui, response.id);

        self.handle_pointer(&response, origin_x, first_baseline, row_h);

        let text_color = if ui.visuals().dark_mode {
            egui::Color32::WHITE
        } else {
            egui::Color32::BLACK
        };

        // Only the lines the clip rect actually shows are painted; the preview
        // can hold a whole paragraph of text at 128px.
        let clip = ui.clip_rect();
        let visible = |line: usize| {
            let top = baseline_of(line) - px_size;
            let bottom = baseline_of(line) + row_h;
            bottom >= clip.top() && top <= clip.bottom()
        };

        let selection = self.selection_range_sorted();

        if let Some(shaped) = self.shaped.as_ref() {
            for (idx, line) in shaped.lines.iter().enumerate() {
                if !visible(idx) {
                    continue;
                }
                let baseline_y = baseline_of(idx);
                let preedit_range = line.preedit_char_range;

                if let Some((lo, hi)) = selection
                    && idx >= lo.line
                    && idx <= hi.line
                {
                    let x0 = if idx == lo.line {
                        origin_x
                            + cluster::caret_x(
                                &line.clusters,
                                committed_to_display(lo.col, preedit_range),
                            )
                    } else {
                        origin_x
                    };
                    let x1 = if idx == hi.line {
                        origin_x
                            + cluster::caret_x(
                                &line.clusters,
                                committed_to_display(hi.col, preedit_range),
                            )
                    } else {
                        // A selected line break shows as a stub past the end
                        // of the line, the way every text editor draws it.
                        origin_x + line.width + px_size * 0.3
                    };
                    let sel_rect = egui::Rect::from_min_max(
                        egui::pos2(x0, baseline_y - px_size),
                        egui::pos2(x1.max(x0 + 1.0), baseline_y + 4.0),
                    );
                    painter.rect_filled(sel_rect, 0.0, ui.visuals().selection.bg_fill);
                }

                if let Some((ps, pe)) = preedit_range {
                    let preedit_x0 = origin_x + cluster::caret_x(&line.clusters, ps);
                    let preedit_x1 = origin_x + cluster::caret_x(&line.clusters, pe);
                    let preedit_rect = egui::Rect::from_min_max(
                        egui::pos2(preedit_x0, baseline_y - px_size),
                        egui::pos2(preedit_x1, baseline_y + 4.0),
                    );
                    painter.rect_filled(preedit_rect, 0.0, text_color);
                }

                let mut pen_x = origin_x;
                for g in &line.glyphs {
                    let gx = pen_x + g.x_offset * px_size;
                    let gy = baseline_y - g.y_offset * px_size;

                    let raster_font = if px_size == 16.0 {
                        &font_pair.0
                    } else {
                        &font_pair.1
                    };
                    if let Some(cached) = self.glyph_cache.get_or_rasterize(
                        ui.ctx(),
                        raster_font,
                        g.glyph_id,
                        px_size,
                        self.color_font,
                        text_color,
                    ) {
                        let draw_x = gx + cached.bearing_x;
                        let draw_y = gy - cached.bearing_y;
                        let draw_rect = egui::Rect::from_min_size(
                            egui::pos2(draw_x, draw_y),
                            egui::vec2(cached.width, cached.height),
                        );

                        let is_preedit =
                            preedit_range.is_some_and(|(ps, pe)| g.cluster >= ps && g.cluster < pe);
                        let glyph_color = if is_preedit {
                            widget_bg
                        } else if cached.is_color {
                            egui::Color32::WHITE
                        } else {
                            text_color
                        };

                        painter.image(
                            cached.texture.id(),
                            draw_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            glyph_color,
                        );
                    }

                    pen_x += g.x_advance * px_size;
                }

                painter.line_segment(
                    [
                        egui::pos2(rect.left(), baseline_y),
                        egui::pos2(rect.right(), baseline_y),
                    ],
                    egui::Stroke::new(0.5, ui.visuals().weak_text_color().gamma_multiply(0.3)),
                );
            }
        }

        let caret_x_pos = caret_x_at(self, self.cursor);
        let caret_top = baseline_of(self.cursor.line) - px_size;
        let caret_rect = egui::Rect::from_min_max(
            egui::pos2(caret_x_pos - 1.0, caret_top),
            egui::pos2(caret_x_pos + 1.0, baseline_of(self.cursor.line) + 4.0),
        );
        if focus {
            painter.line_segment(
                [
                    egui::pos2(caret_x_pos, caret_top),
                    egui::pos2(caret_x_pos, baseline_of(self.cursor.line) + 4.0),
                ],
                egui::Stroke::new(1.5, ui.visuals().text_color()),
            );
        }

        if self.scroll_to_caret {
            self.scroll_to_caret = false;
            // Margin so the caret does not sit flush against the edge.
            ui.scroll_to_rect(caret_rect.expand2(egui::vec2(LEFT_PAD, TOP_PAD)), None);
        }

        // Context menu
        let ctx_clone = ui.ctx().clone();
        response.context_menu(|ui| {
            let caps = self.edit_menu_caps();
            let action = crate::edit_menu::show_edit_menu_items(ui, &caps, false);
            self.apply_edit_action(action, &ctx_clone);
        });
    }

    /// Click, double/triple click and drag: the same gestures as the document
    /// editor's canvas, over shaped lines instead of a character grid.
    fn handle_pointer(
        &mut self,
        response: &egui::Response,
        origin_x: f32,
        first_baseline: f32,
        row_h: f32,
    ) {
        let Some(pos) = response.interact_pointer_pos() else {
            return;
        };
        let clicked = response.clicked();
        let double = response.double_clicked();
        let triple = response.triple_clicked();
        let dragged = response.dragged();
        if !(clicked || double || triple || dragged) {
            return;
        }

        let target = self.caret_at_pos(pos, origin_x, first_baseline, row_h);

        if triple {
            self.selection_anchor = Some(Caret::new(target.line, 0));
            self.cursor = Caret::new(target.line, caret::line_char_len(&self.lines, target.line));
        } else if double {
            let (lo, hi) = caret::word_bounds_at(&self.lines, target);
            self.selection_anchor = Some(lo);
            self.cursor = hi;
        } else if dragged || response.ctx.input(|i| i.modifiers.shift) {
            // Both extend from wherever the caret already is: a drag starts
            // its own selection, a shift-click grows the existing one.
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(self.cursor);
            }
            self.cursor = target;
        } else {
            self.selection_anchor = None;
            self.cursor = target;
        }
    }

    /// Screen position → caret, clamped into the text. The y picks the line by
    /// the same rhythm the baselines are drawn on; the x is resolved against
    /// that line's clusters.
    fn caret_at_pos(
        &self,
        pos: egui::Pos2,
        origin_x: f32,
        first_baseline: f32,
        row_h: f32,
    ) -> Caret {
        let rel = (pos.y - (first_baseline - row_h * 0.75)) / row_h;
        let line = if rel < 0.0 {
            0
        } else {
            (rel as usize).min(self.lines.len().saturating_sub(1))
        };
        let Some(shaped) = self.shaped.as_ref().and_then(|s| s.lines.get(line)) else {
            return caret::clamp(&self.lines, Caret::new(line, 0));
        };
        let display_col = cluster::char_idx_from_x(&shaped.clusters, pos.x - origin_x);
        let col = display_to_committed(display_col, shaped.preedit_char_range);
        caret::clamp(&self.lines, Caret::new(line, col))
    }

    /// Drives the Ctrl+K code point popup, if one is open. Like the editor's,
    /// it previews through the preedit and commits like an IME would.
    fn show_codepoint_popup(&mut self, ui: &egui::Ui, host: egui::Id) {
        let Some((popup, anchor)) = &mut self.codepoint else {
            return;
        };
        let outcome = popup.show(ui.ctx(), ui.id().with("codepoint_popup"), *anchor);
        self.preedit = popup.preedit();
        match outcome {
            CodepointOutcome::Open => {}
            CodepointOutcome::Commit(text) => {
                self.codepoint = None;
                self.preedit.clear();
                restore_host_focus(ui.ctx(), host);
                if !text.is_empty() {
                    self.insert_at_caret(&text);
                }
            }
            CodepointOutcome::Cancel => {
                self.codepoint = None;
                self.preedit.clear();
                restore_host_focus(ui.ctx(), host);
            }
        }
    }

    /// The status-bar line while a code point is being typed: the code point
    /// and its Unicode name. `None` when the popup is closed.
    pub fn codepoint_status(&self) -> Option<String> {
        self.codepoint.as_ref().map(|(p, _)| p.status_label())
    }

    /// `rows_per_page` is how many lines the field currently shows, which is
    /// what Page Up/Down move by. The shared handler deliberately leaves those
    /// two keys alone — only the host knows how tall its viewport is.
    fn handle_input(&mut self, ui: &egui::Ui, caret_screen: egui::Pos2, rows_per_page: usize) {
        let undo_pressed =
            ui.input(|i| i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::Z));
        let redo_pressed = ui.input(|i| {
            (i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::Z))
                || (i.modifiers.command && i.key_pressed(egui::Key::Y))
        });

        if undo_pressed {
            self.perform_undo();
            return;
        }
        if redo_pressed {
            self.perform_redo();
            return;
        }

        // Ctrl+K: type a character by its code point. See
        // `crate::editor::codepoint_popup` for why not Alt, and why Cmd is
        // excluded through `mac_cmd` and not `command`.
        if self.codepoint.is_none()
            && ui.input(|i| {
                i.modifiers.ctrl
                    && !i.modifiers.mac_cmd
                    && !i.modifiers.alt
                    && i.key_pressed(egui::Key::K)
            })
        {
            self.codepoint = Some((CodepointPopup::default(), caret_screen));
            return;
        }

        // Page Up/Down are read straight off the input, so they get the coarse
        // version of the shared handler's composing rule: not acted on while a
        // composition is open. See `doc_input::ImeKeyGuard`.
        let composing = ui.input(|i| doc_input::ime_composing(&i.events, &self.preedit));

        doc_input::handle_text_keys(
            ui,
            &mut TextEdit {
                lines: &mut self.lines,
                cursor: &mut self.cursor,
                selection_anchor: &mut self.selection_anchor,
                undo: &mut self.undo,
                preedit: &mut self.preedit,
                ime_guard: &mut self.ime_guard,
            },
        );

        let page = ui.input(|i| {
            if composing {
                return None;
            }
            let shift = i.modifiers.shift;
            if i.key_pressed(egui::Key::PageDown) {
                Some((1i64, shift))
            } else if i.key_pressed(egui::Key::PageUp) {
                Some((-1i64, shift))
            } else {
                None
            }
        });
        if let Some((dir, shift)) = page {
            if shift {
                self.selection_anchor.get_or_insert(self.cursor);
            } else {
                self.selection_anchor = None;
            }
            let rows = rows_per_page.max(1) as i64;
            let line = (self.cursor.line as i64 + dir * rows).max(0) as usize;
            self.cursor = caret::clamp(&self.lines, Caret::new(line, self.cursor.col));
        }
    }
}

use crate::editor::codepoint_popup::{CodepointOutcome, CodepointPopup, restore_host_focus};

fn committed_to_display(col: usize, preedit_range: Option<(usize, usize)>) -> usize {
    match preedit_range {
        Some((ps, pe)) if col >= ps => col + (pe - ps),
        _ => col,
    }
}

fn display_to_committed(col: usize, preedit_range: Option<(usize, usize)>) -> usize {
    match preedit_range {
        Some((ps, pe)) => {
            if col <= ps {
                col
            } else if col < pe {
                ps
            } else {
                col - (pe - ps)
            }
        }
        None => col,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(text: &str) -> ShapedPreviewState {
        let mut state = ShapedPreviewState::new();
        state.set_text(text);
        state
    }

    /// Drives one frame of just the key handling, as the focused `show` does.
    /// The rest of `show` needs a built font; the text model does not, so
    /// every shortcut can be tested on its own.
    fn key_frame(ctx: &egui::Context, state: &mut ShapedPreviewState, events: Vec<egui::Event>) {
        // The held-modifier state is separate from each event's own copy, and
        // the chords that are read through `i.modifiers` (undo, redo, Ctrl+K)
        // see only this one.
        let modifiers = events
            .iter()
            .find_map(|e| match e {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or_default();
        let input = egui::RawInput {
            events,
            modifiers,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.handle_input(ui, egui::pos2(0.0, 0.0), 2);
            });
        });
    }

    fn key_with(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// The second copy of a key press that macOS re-delivers after the Korean
    /// IME has answered; `egui` flags it as a repeat.
    fn key_repeat(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: true,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn key_release(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn key_press(key: egui::Key) -> egui::Event {
        key_with(key, egui::Modifiers::default())
    }

    #[test]
    fn enter_splits_the_line_and_typing_continues_on_it() {
        let ctx = egui::Context::default();
        let mut state = state_with("ab");
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::Enter)]);
        key_frame(&ctx, &mut state, vec![egui::Event::Text("c".into())]);
        assert_eq!(state.text(), "ab\nc");
        assert_eq!(state.cursor, Caret::new(1, 1));
    }

    #[test]
    fn vertical_motion_walks_lines() {
        let ctx = egui::Context::default();
        let mut state = state_with("abc\nde");
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::ArrowUp)]);
        // Column is clamped onto the shorter line and restored going back.
        assert_eq!(state.cursor, Caret::new(0, 2));
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::ArrowDown)]);
        assert_eq!(state.cursor, Caret::new(1, 2));
    }

    #[test]
    fn backspace_at_column_zero_joins_with_the_previous_line() {
        let ctx = egui::Context::default();
        let mut state = state_with("ab\ncd");
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::Home)]);
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::Backspace)]);
        assert_eq!(state.text(), "abcd");
        assert_eq!(state.cursor, Caret::new(0, 2));
    }

    #[test]
    fn word_motion_and_word_delete_work() {
        let ctx = egui::Context::default();
        let mut state = state_with("hello world");
        let word_mod = if cfg!(target_os = "macos") {
            egui::Modifiers {
                alt: true,
                ..Default::default()
            }
        } else {
            egui::Modifiers {
                ctrl: true,
                command: true,
                ..Default::default()
            }
        };
        key_frame(
            &ctx,
            &mut state,
            vec![key_with(egui::Key::ArrowLeft, word_mod)],
        );
        assert_eq!(state.cursor, Caret::new(0, 6));
        key_frame(
            &ctx,
            &mut state,
            vec![key_with(egui::Key::Backspace, word_mod)],
        );
        assert_eq!(state.text(), "world");
    }

    fn preedit(s: &str) -> egui::Event {
        egui::Event::Ime(egui::ImeEvent::Preedit(s.into()))
    }

    fn commit(s: &str) -> egui::Event {
        egui::Event::Ime(egui::ImeEvent::Commit(s.into()))
    }

    /// The Korean IME answers a key it does not want by committing what it
    /// has; macOS then re-delivers the same key press *after* the IME events,
    /// flagged as a repeat. The first copy is the IME's, the second is ours —
    /// so Enter commits and breaks the line exactly once.
    #[test]
    fn a_key_the_korean_ime_passes_through_acts_once_after_the_commit() {
        let ctx = egui::Context::default();
        let mut state = state_with("");
        key_frame(&ctx, &mut state, vec![preedit("하")]);
        // One physical Enter, exactly as the platform delivers it.
        key_frame(
            &ctx,
            &mut state,
            vec![
                key_press(egui::Key::Enter),
                egui::Event::Ime(egui::ImeEvent::Disabled),
                commit("한"),
                egui::Event::Ime(egui::ImeEvent::Disabled),
                key_repeat(egui::Key::Enter),
                key_release(egui::Key::Enter),
            ],
        );
        assert_eq!(state.text(), "한\n");
        assert_eq!(state.cursor, Caret::new(1, 0));
    }

    /// The same key, split across frames the way the platform sometimes
    /// delivers it: the press before the composition ends is still the IME's.
    #[test]
    fn the_key_that_ends_a_composition_is_not_acted_on_twice_across_frames() {
        let ctx = egui::Context::default();
        let mut state = state_with("");
        key_frame(&ctx, &mut state, vec![preedit("그")]);
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::ArrowLeft)]);
        assert_eq!(state.cursor, Caret::zero());
        key_frame(
            &ctx,
            &mut state,
            vec![commit("글"), key_repeat(egui::Key::ArrowLeft)],
        );
        assert_eq!(state.text(), "글");
        // The commit left the caret after "글"; the pass-through moved it in
        // front of it — once, not twice.
        assert_eq!(state.cursor, Caret::zero());
    }

    /// Picking a Hanja from the conversion window ends the composition and
    /// delivers only the *trailing* Enter: the press that opened the window
    /// and the one that moved through it went to the window, not to us. With
    /// no press of its own before the commit, that Enter is the IME's — the
    /// document must not break the line.
    #[test]
    fn the_enter_that_picks_a_hanja_is_not_the_documents() {
        let ctx = egui::Context::default();
        let mut state = state_with("");
        key_frame(&ctx, &mut state, vec![preedit("한")]);
        key_frame(
            &ctx,
            &mut state,
            vec![
                egui::Event::Ime(egui::ImeEvent::Disabled),
                commit("韓"),
                egui::Event::Ime(egui::ImeEvent::Disabled),
                egui::Event::Ime(egui::ImeEvent::Disabled),
                key_press(egui::Key::Enter),
                key_release(egui::Key::Enter),
            ],
        );
        assert_eq!(state.text(), "韓");
        assert_eq!(state.cursor, Caret::new(0, 1));
    }

    /// The Japanese IME consumes Enter outright: it confirms the composition
    /// and nothing else follows, so no line is broken. Its events are exactly
    /// the Korean ones minus the re-delivered key press.
    #[test]
    fn enter_on_a_japanese_composition_only_commits() {
        let ctx = egui::Context::default();
        let mut state = state_with("");
        key_frame(&ctx, &mut state, vec![preedit("ひらがな")]);
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::Enter)]);
        key_frame(
            &ctx,
            &mut state,
            vec![
                egui::Event::Ime(egui::ImeEvent::Disabled),
                commit("ひらがな"),
                egui::Event::Ime(egui::ImeEvent::Disabled),
                egui::Event::Ime(egui::ImeEvent::Disabled),
                key_release(egui::Key::Enter),
            ],
        );
        assert_eq!(state.text(), "ひらがな");
        assert_eq!(state.cursor, Caret::new(0, 4));
    }

    /// The Japanese IME also consumes motion keys, answering with a fresh
    /// preedit rather than a commit.
    #[test]
    fn a_key_the_japanese_ime_consumes_is_dropped() {
        let ctx = egui::Context::default();
        let mut state = state_with("ab");
        state.cursor = Caret::new(0, 2);
        key_frame(&ctx, &mut state, vec![preedit("ひらがな")]);
        key_frame(
            &ctx,
            &mut state,
            vec![key_press(egui::Key::ArrowLeft), preedit("ひらがな")],
        );
        assert_eq!(state.cursor, Caret::new(0, 2), "the IME took the arrow");
        key_frame(&ctx, &mut state, vec![commit("ひらがな")]);
        assert_eq!(state.text(), "abひらがな");
        assert_eq!(state.cursor, Caret::new(0, 6));
    }

    /// Backspace is the one key every IME eats while composing: it shortens
    /// the composition, and the text behind it must be left alone — including
    /// when the platform re-delivers the key after the composition is gone.
    #[test]
    fn backspace_while_composing_belongs_to_the_ime() {
        let ctx = egui::Context::default();
        let mut state = state_with("ab");
        key_frame(&ctx, &mut state, vec![preedit("ㅎ")]);
        key_frame(
            &ctx,
            &mut state,
            vec![
                key_press(egui::Key::Backspace),
                // The composition is now empty — the jamo is what got deleted.
                preedit(""),
                key_repeat(egui::Key::Backspace),
                key_release(egui::Key::Backspace),
            ],
        );
        assert_eq!(state.text(), "ab");
        assert_eq!(state.cursor, Caret::new(0, 2));
    }

    /// A composition that ends without committing must not leave the field
    /// thinking it is still composing — that would swallow keys for good.
    #[test]
    fn a_disabled_ime_clears_a_stuck_composition() {
        let ctx = egui::Context::default();
        let mut state = state_with("");
        key_frame(&ctx, &mut state, vec![preedit("ㅎ")]);
        key_frame(
            &ctx,
            &mut state,
            vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
        );
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::Enter)]);
        assert_eq!(state.text(), "\n");
    }

    /// Page Up/Down are the one motion the shared handler leaves to the host,
    /// since only it knows how many lines fit; `key_frame` shows two.
    #[test]
    fn page_keys_move_the_caret_by_a_screenful_of_lines() {
        let ctx = egui::Context::default();
        let mut state = state_with("a\nb\nc\nd\ne");
        state.cursor = Caret::zero();
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::PageDown)]);
        assert_eq!(state.cursor, Caret::new(2, 0));
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::PageDown)]);
        assert_eq!(state.cursor, Caret::new(4, 0));
        // Past the end clamps rather than wrapping or panicking.
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::PageDown)]);
        assert_eq!(state.cursor, Caret::new(4, 0));
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::PageUp)]);
        assert_eq!(state.cursor, Caret::new(2, 0));
    }

    #[test]
    fn select_all_spans_every_line_and_delete_clears_it() {
        let ctx = egui::Context::default();
        let mut state = state_with("ab\ncd");
        let cmd = egui::Modifiers {
            command: true,
            ..Default::default()
        };
        key_frame(&ctx, &mut state, vec![key_with(egui::Key::A, cmd)]);
        assert_eq!(state.selection_anchor, Some(Caret::zero()));
        assert_eq!(state.cursor, Caret::new(1, 2));
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::Backspace)]);
        assert_eq!(state.text(), "");
    }

    #[test]
    fn a_multi_line_paste_keeps_its_line_breaks() {
        let ctx = egui::Context::default();
        let mut state = state_with("");
        key_frame(
            &ctx,
            &mut state,
            vec![egui::Event::Paste("one\ntwo".into())],
        );
        assert_eq!(state.text(), "one\ntwo");
        assert_eq!(state.cursor, Caret::new(1, 3));
    }

    #[test]
    fn undo_and_redo_restore_multi_line_edits() {
        let ctx = egui::Context::default();
        let mut state = state_with("ab");
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::Enter)]);
        key_frame(&ctx, &mut state, vec![egui::Event::Text("c".into())]);
        assert_eq!(state.text(), "ab\nc");

        let cmd = egui::Modifiers {
            command: true,
            ..Default::default()
        };
        key_frame(&ctx, &mut state, vec![key_with(egui::Key::Z, cmd)]);
        assert_eq!(state.text(), "ab\n");
        key_frame(&ctx, &mut state, vec![key_with(egui::Key::Z, cmd)]);
        assert_eq!(state.text(), "ab");
        key_frame(
            &ctx,
            &mut state,
            vec![key_with(
                egui::Key::Z,
                egui::Modifiers {
                    command: true,
                    shift: true,
                    ..Default::default()
                },
            )],
        );
        assert_eq!(state.text(), "ab\n");
    }

    #[test]
    fn shift_arrow_selects_across_the_line_break() {
        let ctx = egui::Context::default();
        let mut state = state_with("ab\ncd");
        state.cursor = Caret::new(0, 2);
        state.selection_anchor = None;
        let shift = egui::Modifiers {
            shift: true,
            ..Default::default()
        };
        key_frame(
            &ctx,
            &mut state,
            vec![
                key_with(egui::Key::ArrowRight, shift),
                key_with(egui::Key::ArrowRight, shift),
            ],
        );
        assert_eq!(state.selection_anchor, Some(Caret::new(0, 2)));
        assert_eq!(state.cursor, Caret::new(1, 1));
        assert_eq!(
            caret::extract_text(&state.lines, Caret::new(0, 2), state.cursor),
            "\nc"
        );
    }

    /// Text with a collapsed selection anchor: shift-selecting and then
    /// unselecting (or a click that left the anchor where the caret is) leaves
    /// `selection_anchor == cursor`, which is not a selection.
    #[test]
    fn backspace_past_a_collapsed_anchor_leaves_no_phantom_selection() {
        let ctx = egui::Context::default();
        // "F" + U+0303 COMBINING TILDE, deleting the combining mark.
        let mut state = state_with("F\u{303}");
        state.selection_anchor = Some(state.cursor);
        key_frame(&ctx, &mut state, vec![key_press(egui::Key::Backspace)]);
        assert_eq!(state.text(), "F");
        assert_eq!(state.cursor, Caret::new(0, 1));
        assert_eq!(state.selection_range_sorted(), None);
        // This is what the status bar calls, and what used to panic with
        // "range end index 2 out of range for slice of length 1".
        assert_eq!(state.selection_codepoints_label(), None);
    }

    #[test]
    fn a_stale_selection_is_clamped_to_the_text() {
        let mut state = state_with("F");
        state.cursor = Caret::new(0, 0);
        state.selection_anchor = Some(Caret::new(9, 9));
        assert_eq!(
            state.selection_range_sorted(),
            Some((Caret::new(0, 0), Caret::new(0, 1)))
        );
        assert_eq!(
            state.selection_codepoints_label().as_deref(),
            Some("[0046]")
        );
    }

    #[test]
    fn the_selection_label_takes_its_context_from_the_selections_own_lines() {
        let mut state = state_with("abc\ndef");
        state.selection_anchor = Some(Caret::new(0, 1));
        state.cursor = Caret::new(1, 1);
        // 62 63 0A 64 selected; context is "a" before and "e" after.
        assert_eq!(
            state.selection_codepoints_label().as_deref(),
            Some("0061 [0062 0063 000A 0064] 0065 \u{2026}")
        );
    }

    /// Drives one frame of just the code point popup, as `show` does, with
    /// `events` delivered to it.
    fn popup_frame(
        ctx: &egui::Context,
        state: &mut ShapedPreviewState,
        host: egui::Id,
        events: Vec<egui::Event>,
    ) {
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.show_codepoint_popup(ui, host);
            });
        });
    }

    /// Closing the popup must hand the keyboard back to the preview field, the
    /// way it goes back to the editor canvas: otherwise typing after Escape or
    /// Enter goes nowhere at all.
    #[test]
    fn closing_the_codepoint_popup_returns_focus_to_the_preview() {
        for (label, closing_key) in [("cancel", egui::Key::Escape), ("commit", egui::Key::Enter)] {
            let ctx = egui::Context::default();
            let host = egui::Id::new("preview_field");
            let mut state = ShapedPreviewState::new();
            state.codepoint = Some((CodepointPopup::default(), egui::pos2(10.0, 10.0)));

            // Two frames to open: the first lays the field out, the second
            // gives it focus.
            popup_frame(&ctx, &mut state, host, vec![]);
            popup_frame(&ctx, &mut state, host, vec![]);
            assert!(state.codepoint.is_some(), "{label}: popup should be open");

            popup_frame(&ctx, &mut state, host, vec![key_press(closing_key)]);
            assert!(state.codepoint.is_none(), "{label}: popup should be closed");
            assert_eq!(
                ctx.memory(|m| m.focused()),
                Some(host),
                "{label}: the preview field should hold the keyboard again"
            );
        }
    }
}
