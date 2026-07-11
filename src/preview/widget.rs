use crate::edit_menu::{EditAction, EditMenuCaps};
use crate::preview::cluster::{self, ClusterSpan};
use crate::preview::rasterizer::GlyphCache;
use crate::preview::{self, ShapedGlyph, ShaperBackend};

const COALESCE_MS: u128 = 800;

struct UndoSnapshot {
    text: String,
    caret_pos: usize,
}

pub struct ShapedPreviewState {
    pub text: String,
    pub caret_pos: usize,
    pub selection_anchor: Option<usize>,
    pub backends: Vec<Box<dyn ShaperBackend>>,
    pub selected_backend: usize,
    pub glyph_cache: GlyphCache,
    shaped_result: Option<ShapedResult>,
    last_error: Option<String>,
    preedit: String,
    hex_input: Option<String>,
    undo_stack: Vec<UndoSnapshot>,
    redo_stack: Vec<UndoSnapshot>,
    last_edit_time: std::time::Instant,
    has_focus: bool,
}

struct ShapedResult {
    text: String,
    font_gen: u64,
    backend_idx: usize,
    glyphs: Vec<ShapedGlyph>,
    clusters: Vec<ClusterSpan>,
    px_size: f32,
    preedit_char_range: Option<(usize, usize)>,
}

impl ShapedPreviewState {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            caret_pos: 0,
            selection_anchor: None,
            backends: preview::available_backends(),
            selected_backend: 0,
            glyph_cache: GlyphCache::new(),
            shaped_result: None,
            last_error: None,
            preedit: String::new(),
            hex_input: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_time: std::time::Instant::now() - std::time::Duration::from_secs(10),
            has_focus: false,
        }
    }

    pub fn is_focused(&self) -> bool {
        self.has_focus
    }

    pub fn invalidate_font(&mut self, font_gen: u64) {
        self.glyph_cache.invalidate_if_changed(font_gen);
        if let Some(ref result) = self.shaped_result
            && result.font_gen != font_gen {
                self.shaped_result = None;
            }
    }

    fn ensure_shaped(&mut self, font_data: &[u8], font_gen: u64, px_size: f32) {
        let (display_text, preedit_range) = if self.preedit.is_empty() {
            (self.text.clone(), None)
        } else {
            let byte_pos = char_to_byte(&self.text, self.caret_pos);
            let preedit_len = self.preedit.chars().count();
            let display = format!(
                "{}{}{}",
                &self.text[..byte_pos],
                self.preedit,
                &self.text[byte_pos..]
            );
            (display, Some((self.caret_pos, self.caret_pos + preedit_len)))
        };

        let needs_reshape = match &self.shaped_result {
            Some(r) => {
                r.text != display_text
                    || r.font_gen != font_gen
                    || r.backend_idx != self.selected_backend
                    || r.px_size != px_size
            }
            None => true,
        };

        if !needs_reshape {
            return;
        }

        if display_text.is_empty() {
            self.shaped_result = Some(ShapedResult {
                text: String::new(),
                font_gen,
                backend_idx: self.selected_backend,
                glyphs: Vec::new(),
                clusters: Vec::new(),
                px_size,
                preedit_char_range: None,
            });
            self.last_error = None;
            return;
        }

        let backend = &self.backends[self.selected_backend];
        match backend.shape(font_data, &display_text, 1024, &[]) {
            Ok(glyphs) => {
                let total_chars = display_text.chars().count();
                let mut clusters = cluster::build_clusters(&glyphs, px_size);
                cluster::finalize_clusters(&mut clusters, total_chars);
                self.shaped_result = Some(ShapedResult {
                    text: display_text,
                    font_gen,
                    backend_idx: self.selected_backend,
                    glyphs,
                    clusters,
                    px_size,
                    preedit_char_range: preedit_range,
                });
                self.last_error = None;
            }
            Err(e) => {
                self.last_error = Some(e.0);
                self.shaped_result = None;
            }
        }
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

    fn selection_range_sorted(&self) -> Option<(usize, usize)> {
        let anchor = self.selection_anchor?;
        let lo = anchor.min(self.caret_pos);
        let hi = anchor.max(self.caret_pos);
        if lo == hi {
            None
        } else {
            Some((lo, hi))
        }
    }

    pub fn selection_codepoints_label(&self) -> Option<String> {
        let (lo, hi) = self.selection_range_sorted()?;
        let chars: Vec<char> = self.text.chars().collect();
        let total = chars.len();

        let mut parts = Vec::new();

        if lo >= 2 {
            parts.push("\u{2026} ".to_string());
        }
        if lo >= 1 {
            parts.push(format!("{:04X} ", chars[lo - 1] as u32));
        }

        parts.push("[".to_string());
        for i in lo..hi {
            if i > lo {
                parts.push(" ".to_string());
            }
            parts.push(format!("{:04X}", chars[i] as u32));
        }
        parts.push("]".to_string());

        if hi < total {
            parts.push(format!(" {:04X}", chars[hi] as u32));
        }
        if hi + 1 < total {
            parts.push(" \u{2026}".to_string());
        }

        Some(parts.concat())
    }

    fn selected_text(&self) -> Option<String> {
        let (lo, hi) = self.selection_range_sorted()?;
        let start = char_to_byte(&self.text, lo);
        let end = char_to_byte(&self.text, hi);
        Some(self.text[start..end].to_string())
    }

    fn save_for_undo(&mut self) {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_edit_time).as_millis();
        self.last_edit_time = now;

        if elapsed < COALESCE_MS && !self.undo_stack.is_empty() {
            self.redo_stack.clear();
            return;
        }

        self.undo_stack.push(UndoSnapshot {
            text: self.text.clone(),
            caret_pos: self.caret_pos,
        });
        self.redo_stack.clear();
    }

    fn break_coalesce(&mut self) {
        self.last_edit_time = std::time::Instant::now() - std::time::Duration::from_secs(10);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
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
        if let Some(snapshot) = self.undo_stack.pop() {
            self.redo_stack.push(UndoSnapshot {
                text: self.text.clone(),
                caret_pos: self.caret_pos,
            });
            self.text = snapshot.text;
            self.caret_pos = snapshot.caret_pos;
            self.selection_anchor = None;
            self.shaped_result = None;
            self.break_coalesce();
        }
    }

    fn perform_redo(&mut self) {
        if let Some(snapshot) = self.redo_stack.pop() {
            self.undo_stack.push(UndoSnapshot {
                text: self.text.clone(),
                caret_pos: self.caret_pos,
            });
            self.text = snapshot.text;
            self.caret_pos = snapshot.caret_pos;
            self.selection_anchor = None;
            self.shaped_result = None;
            self.break_coalesce();
        }
    }

    pub fn apply_edit_action(&mut self, action: EditAction, ctx: &egui::Context) {
        match action {
            EditAction::None => {}
            EditAction::Undo => self.perform_undo(),
            EditAction::Redo => self.perform_redo(),
            EditAction::Cut => {
                if let Some(sel) = self.selected_text() {
                    ctx.copy_text(sel);
                    self.save_for_undo();
                    self.delete_selection();
                }
            }
            EditAction::Copy => {
                if let Some(sel) = self.selected_text() {
                    ctx.copy_text(sel);
                }
            }
            EditAction::Paste => {
                if let Ok(mut clip) = arboard::Clipboard::new()
                    && let Ok(text) = clip.get_text()
                        && !text.is_empty() {
                            self.save_for_undo();
                            self.delete_selection();
                            let byte_pos = char_to_byte(&self.text, self.caret_pos);
                            let clean: String = text.replace(['\n', '\r'], "");
                            self.text.insert_str(byte_pos, &clean);
                            self.caret_pos += clean.chars().count();
                            self.shaped_result = None;
                        }
            }
            EditAction::Delete => {
                if self.selection_range_sorted().is_some() {
                    self.save_for_undo();
                    self.delete_selection();
                }
            }
            EditAction::SelectAll => {
                self.selection_anchor = Some(0);
                self.caret_pos = self.text.chars().count();
            }
        }
    }

    fn delete_selection(&mut self) {
        if let Some((lo, hi)) = self.selection_range_sorted() {
            let start = char_to_byte(&self.text, lo);
            let end = char_to_byte(&self.text, hi);
            self.text.drain(start..end);
            self.caret_pos = lo;
            self.selection_anchor = None;
            self.shaped_result = None;
        }
    }

    fn insert_at_caret(&mut self, s: &str) {
        self.save_for_undo();
        self.delete_selection();
        let byte_pos = char_to_byte(&self.text, self.caret_pos);
        self.text.insert_str(byte_pos, s);
        self.caret_pos += s.chars().count();
        self.shaped_result = None;
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
            ui.label("No font built yet.");
            return;
        };

        self.ensure_shaped(&font_pair.1, font_gen, px_size);

        if let Some(ref err) = self.last_error {
            self.has_focus = false;
            ui.colored_label(egui::Color32::RED, format!("Shaping error: {err}"));
            return;
        }

        let remaining = ui.available_size();
        let widget_bg = ui.visuals().extreme_bg_color;
        let widget_stroke = ui.visuals().widgets.noninteractive.bg_stroke;

        let (response, painter) =
            ui.allocate_painter(remaining, egui::Sense::click().union(egui::Sense::drag()));

        let rect = response.rect;

        painter.rect(rect, 2.0, widget_bg, widget_stroke, egui::epaint::StrokeKind::Inside);

        let origin_x = rect.left() + 16.0;
        let baseline_y = rect.top() + px_size + 8.0;

        let focus = response.has_focus();
        self.has_focus = focus;

        if focus || response.clicked() {
            response.request_focus();
        }

        if focus {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(response.id, egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: false,
                    tab: true,
                });
            });
        }

        if focus {
            self.handle_input(ui);
            let display_caret = self.caret_pos + self.preedit.chars().count();
            let ime_rect = egui::Rect::from_min_size(
                egui::pos2(
                    origin_x
                        + self
                            .shaped_result
                            .as_ref()
                            .map_or(0.0, |r| cluster::caret_x(&r.clusters, display_caret)),
                    baseline_y - px_size,
                ),
                egui::vec2(16.0, px_size + 4.0),
            );
            ui.ctx().output_mut(|o| o.ime = Some(egui::output::IMEOutput {
                rect: ime_rect,
                cursor_rect: ime_rect,
            }));
        }

        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos() {
                let click_x = pos.x - origin_x;
                if let Some(ref result) = self.shaped_result {
                    let display_idx = cluster::char_idx_from_x(&result.clusters, click_x);
                    self.caret_pos = display_to_committed(display_idx, result.preedit_char_range);
                    self.selection_anchor = None;
                }
            }

        if response.dragged()
            && let Some(pos) = response.interact_pointer_pos() {
                let click_x = pos.x - origin_x;
                if let Some(ref result) = self.shaped_result {
                    let display_idx = cluster::char_idx_from_x(&result.clusters, click_x);
                    let committed_idx = display_to_committed(display_idx, result.preedit_char_range);
                    if self.selection_anchor.is_none() {
                        self.selection_anchor = Some(self.caret_pos);
                    }
                    self.caret_pos = committed_idx;
                }
            }

        let text_color = if ui.visuals().dark_mode {
            egui::Color32::WHITE
        } else {
            egui::Color32::BLACK
        };

        if let Some(ref result) = self.shaped_result {
            let preedit_range = result.preedit_char_range;
            let preedit_len = preedit_range
                .map(|(ps, pe)| pe - ps)
                .unwrap_or(0);

            if let Some((sel_lo, sel_hi)) = self.selection_range_sorted() {
                let d_lo = committed_to_display(sel_lo, preedit_range, preedit_len);
                let d_hi = committed_to_display(sel_hi, preedit_range, preedit_len);
                let sel_x0 = origin_x + cluster::caret_x(&result.clusters, d_lo);
                let sel_x1 = origin_x + cluster::caret_x(&result.clusters, d_hi);
                let sel_rect = egui::Rect::from_min_max(
                    egui::pos2(sel_x0, baseline_y - px_size),
                    egui::pos2(sel_x1, baseline_y + 4.0),
                );
                painter.rect_filled(
                    sel_rect,
                    0.0,
                    ui.visuals().selection.bg_fill,
                );
            }

            if let Some((ps, pe)) = preedit_range {
                let preedit_x0 = origin_x + cluster::caret_x(&result.clusters, ps);
                let preedit_x1 = origin_x + cluster::caret_x(&result.clusters, pe);
                let preedit_rect = egui::Rect::from_min_max(
                    egui::pos2(preedit_x0, baseline_y - px_size),
                    egui::pos2(preedit_x1, baseline_y + 4.0),
                );
                painter.rect_filled(preedit_rect, 0.0, text_color);
            }

            let mut pen_x = origin_x;

            for g in &result.glyphs {
                let gx = pen_x + g.x_offset * px_size;
                let gy = baseline_y - g.y_offset * px_size;

                if let Some(cached) = self.glyph_cache.get_or_rasterize(
                    ui.ctx(),
                    &font_pair.1,
                    g.glyph_id,
                    px_size,
                ) {
                    let draw_x = gx + cached.bearing_x;
                    let draw_y = gy - cached.bearing_y;
                    let draw_rect = egui::Rect::from_min_size(
                        egui::pos2(draw_x, draw_y),
                        egui::vec2(cached.width, cached.height),
                    );

                    let glyph_color =
                        if preedit_range.is_some_and(|(ps, pe)| g.cluster >= ps && g.cluster < pe)
                        {
                            widget_bg
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

            if focus {
                let display_caret_pos =
                    committed_to_display(self.caret_pos, preedit_range, preedit_len);
                let caret_x_pos =
                    origin_x + cluster::caret_x(&result.clusters, display_caret_pos);
                let caret_color = ui.visuals().text_color();
                painter.line_segment(
                    [
                        egui::pos2(caret_x_pos, baseline_y - px_size),
                        egui::pos2(caret_x_pos, baseline_y + 4.0),
                    ],
                    egui::Stroke::new(1.5, caret_color),
                );

                if let Some(hex) = &self.hex_input {
                    let hex_label = format!("U+{hex}");
                    let hex_galley = ui.painter().layout_no_wrap(
                        hex_label,
                        egui::FontId::proportional(14.0),
                        caret_color,
                    );
                    let hex_rect = egui::Rect::from_min_size(
                        egui::pos2(caret_x_pos, baseline_y + 6.0),
                        hex_galley.size(),
                    );
                    painter.rect_filled(hex_rect, 2.0, widget_bg);
                    painter.rect_stroke(hex_rect, 2.0, widget_stroke, egui::epaint::StrokeKind::Outside);
                    painter.galley(hex_rect.min, hex_galley, caret_color);
                }
            }
        }

        painter.line_segment(
            [
                egui::pos2(rect.left(), baseline_y),
                egui::pos2(rect.right(), baseline_y),
            ],
            egui::Stroke::new(0.5, ui.visuals().weak_text_color().gamma_multiply(0.3)),
        );

        // Context menu
        let ctx_clone = ui.ctx().clone();
        response.context_menu(|ui| {
            let caps = self.edit_menu_caps();
            let action = crate::edit_menu::show_edit_menu_items(ui, &caps, false);
            self.apply_edit_action(action, &ctx_clone);
        });
    }

    fn handle_input(&mut self, ui: &egui::Ui) {
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

        let mut hex_char_to_inject: Option<char> = None;
        let events = ui.input(|i| i.events.clone());
        for event in &events {
            match event {
                egui::Event::Ime(egui::ImeEvent::Preedit(s)) => {
                    self.preedit = s.clone();
                }
                egui::Event::Ime(egui::ImeEvent::Commit(s)) => {
                    self.preedit.clear();
                    self.insert_at_caret(s);
                }
                egui::Event::Text(t) if self.hex_input.is_some() => {
                    let _ = t;
                }
                egui::Event::Text(t) => {
                    self.insert_at_caret(t);
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.alt && !modifiers.command && !modifiers.ctrl {
                        if let Some(hex) = key_to_hex_char(*key) {
                            let buf = self.hex_input.get_or_insert_with(String::new);
                            if buf.len() < 6 {
                                buf.push(hex);
                            }
                            continue;
                        }
                        if self.hex_input.is_some() {
                            self.hex_input = None;
                            continue;
                        }
                    }

                    let cmd = modifiers.command;
                    let shift = modifiers.shift;

                    match key {
                        egui::Key::Z if cmd => {}
                        egui::Key::Y if cmd => {}
                        egui::Key::ArrowLeft => {
                            if !shift
                                && let Some((lo, _)) = self.selection_range_sorted() {
                                    self.caret_pos = lo;
                                    self.selection_anchor = None;
                                    continue;
                                }
                            if shift && self.selection_anchor.is_none() {
                                self.selection_anchor = Some(self.caret_pos);
                            }
                            if self.caret_pos > 0 {
                                self.caret_pos -= 1;
                            }
                            if !shift {
                                self.selection_anchor = None;
                            }
                        }
                        egui::Key::ArrowRight => {
                            let len = self.text.chars().count();
                            if !shift
                                && let Some((_, hi)) = self.selection_range_sorted() {
                                    self.caret_pos = hi;
                                    self.selection_anchor = None;
                                    continue;
                                }
                            if shift && self.selection_anchor.is_none() {
                                self.selection_anchor = Some(self.caret_pos);
                            }
                            if self.caret_pos < len {
                                self.caret_pos += 1;
                            }
                            if !shift {
                                self.selection_anchor = None;
                            }
                        }
                        egui::Key::Home => {
                            if shift && self.selection_anchor.is_none() {
                                self.selection_anchor = Some(self.caret_pos);
                            }
                            self.caret_pos = 0;
                            if !shift {
                                self.selection_anchor = None;
                            }
                        }
                        egui::Key::End => {
                            if shift && self.selection_anchor.is_none() {
                                self.selection_anchor = Some(self.caret_pos);
                            }
                            self.caret_pos = self.text.chars().count();
                            if !shift {
                                self.selection_anchor = None;
                            }
                        }
                        egui::Key::A if cmd => {
                            self.selection_anchor = Some(0);
                            self.caret_pos = self.text.chars().count();
                        }
                        egui::Key::Backspace => {
                            if self.selection_range_sorted().is_some() {
                                self.save_for_undo();
                                self.delete_selection();
                            } else if self.caret_pos > 0 {
                                self.save_for_undo();
                                let start = char_to_byte(&self.text, self.caret_pos - 1);
                                let end = char_to_byte(&self.text, self.caret_pos);
                                self.text.drain(start..end);
                                self.caret_pos -= 1;
                                self.shaped_result = None;
                            }
                        }
                        egui::Key::Delete => {
                            if self.selection_range_sorted().is_some() {
                                self.save_for_undo();
                                self.delete_selection();
                            } else {
                                let len = self.text.chars().count();
                                if self.caret_pos < len {
                                    self.save_for_undo();
                                    let start = char_to_byte(&self.text, self.caret_pos);
                                    let end = char_to_byte(&self.text, self.caret_pos + 1);
                                    self.text.drain(start..end);
                                    self.shaped_result = None;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                egui::Event::Key {
                    pressed: false,
                    ..
                } => {
                    let alt_held = ui.input(|i| i.modifiers.alt);
                    if !alt_held && self.hex_input.is_some()
                        && let Some(hex_str) = self.hex_input.take()
                            && let Some(ch) = validate_hex_codepoint(&hex_str) {
                                hex_char_to_inject = Some(ch);
                            }
                }
                egui::Event::Copy => {
                    if let Some(sel) = self.selected_text() {
                        ui.ctx().copy_text(sel);
                    }
                }
                egui::Event::Cut => {
                    if let Some(sel) = self.selected_text() {
                        ui.ctx().copy_text(sel);
                        self.save_for_undo();
                        self.delete_selection();
                    }
                }
                egui::Event::Paste(s) => {
                    self.insert_at_caret(s);
                }
                _ => {}
            }
        }

        if let Some(ch) = hex_char_to_inject {
            self.insert_at_caret(&ch.to_string());
        }

        let alt_held = ui.input(|i| i.modifiers.alt);
        if !alt_held && self.hex_input.is_some()
            && let Some(hex_str) = self.hex_input.take()
                && let Some(ch) = validate_hex_codepoint(&hex_str) {
                    self.insert_at_caret(&ch.to_string());
                }
    }
}

fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

fn key_to_hex_char(key: egui::Key) -> Option<char> {
    match key {
        egui::Key::Num0 => Some('0'),
        egui::Key::Num1 => Some('1'),
        egui::Key::Num2 => Some('2'),
        egui::Key::Num3 => Some('3'),
        egui::Key::Num4 => Some('4'),
        egui::Key::Num5 => Some('5'),
        egui::Key::Num6 => Some('6'),
        egui::Key::Num7 => Some('7'),
        egui::Key::Num8 => Some('8'),
        egui::Key::Num9 => Some('9'),
        egui::Key::A => Some('A'),
        egui::Key::B => Some('B'),
        egui::Key::C => Some('C'),
        egui::Key::D => Some('D'),
        egui::Key::E => Some('E'),
        egui::Key::F => Some('F'),
        _ => None,
    }
}

fn validate_hex_codepoint(hex: &str) -> Option<char> {
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
}

fn committed_to_display(
    idx: usize,
    preedit_range: Option<(usize, usize)>,
    preedit_len: usize,
) -> usize {
    match preedit_range {
        Some((ps, _)) if idx >= ps => idx + preedit_len,
        _ => idx,
    }
}

fn display_to_committed(idx: usize, preedit_range: Option<(usize, usize)>) -> usize {
    match preedit_range {
        Some((ps, pe)) => {
            if idx <= ps {
                idx
            } else if idx < pe {
                ps
            } else {
                idx - (pe - ps)
            }
        }
        None => idx,
    }
}
