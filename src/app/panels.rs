//! The surrounding panels: sidebar, status bar, bottom preview panel, editor
//! panel and the issues tab.

use super::*;
use super::background::BackgroundTaskPhase;
use super::docs::collect_effective_docs;

pub(super) fn min_bottom_panel_height(screen_height: f32) -> f32 {
    270.0_f32.min(screen_height * 0.5)
}

fn show_issues_tab(
    ui: &mut egui::Ui,
    issues: &[&Issue],
    click: &mut Option<(PathBuf, usize)>,
) {
    if issues.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label("No issues");
        });
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (issue_idx, issue) in issues.iter().enumerate() {
            let icon = match issue.severity {
                crate::issues::Severity::Error => "\u{2716}",
                crate::issues::Severity::Warning => "\u{26A0}",
            };
            let icon_color = match issue.severity {
                crate::issues::Severity::Error => egui::Color32::from_rgb(220, 60, 60),
                crate::issues::Severity::Warning => egui::Color32::from_rgb(200, 180, 50),
            };
            let file_name = issue
                .file
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let location = format!("{file_name}:{}", issue.file_line);

            let row_id = ui.id().with(("issue_row", issue_idx));
            let resp = ui.horizontal(|ui| {
                ui.label(egui::RichText::new(icon).color(icon_color).size(16.0));
                ui.label(egui::RichText::new(&issue.message).size(16.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(&location)
                            .size(16.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                });
            });

            let click_resp = ui.interact(
                resp.response.rect,
                row_id,
                egui::Sense::click(),
            );
            if click_resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if click_resp.clicked() {
                *click = Some((issue.file.clone(), issue.line));
            }
        }
    });
}

impl UniformApp {
    fn ensure_min_panel_height(&mut self, screen_height: f32) {
        let min_h = min_bottom_panel_height(screen_height);
        if self.bottom_panel_height < min_h {
            self.bottom_panel_height = min_h;
            self.bottom_panel_height_override = true;
        }
    }

    pub(super) fn show_sidebar_panel(&mut self, ctx: &egui::Context, editor_focused: bool) {
        let mut sidebar_actions = Vec::new();
        egui::SidePanel::left("sidebar")
            .default_width(200.0)
            .show(ctx, |ui| {
                let dirty_paths: Vec<&std::path::Path> = self.open_documents
                    .iter()
                    .filter(|d| d.document.dirty)
                    .map(|d| d.document.path.as_path())
                    .collect();
                // Field-level accesses so the `sidebar` borrow stays disjoint
                // from the `open_documents` one.
                let active_path = self
                    .panes
                    .active_doc_idx()
                    .and_then(|i| self.open_documents.get(i))
                    .map(|d| d.document.path.as_path());
                sidebar_actions = self.sidebar.show(ui, active_path, &dirty_paths, editor_focused);
            });

        for action in sidebar_actions {
            match action {
                SidebarAction::OpenFile(path) => {
                    self.open_file(path);
                }
                SidebarAction::FileRenamed { old, new } => {
                    for doc in &mut self.open_documents {
                        if doc.document.path == old {
                            doc.document.path = new.clone();
                        }
                    }
                    for doc in &mut self.font_base_docs {
                        if doc.path == old {
                            doc.path = new.clone();
                        }
                    }
                    self.set_status(format!(
                        "Renamed {} -> {}",
                        old.file_name().unwrap_or_default().to_string_lossy(),
                        new.file_name().unwrap_or_default().to_string_lossy(),
                    ));
                }
                SidebarAction::FileCreated(path) => {
                    self.set_status(format!("Created {}", path.display()));
                    self.open_file(path);
                }
            }
        }
    }

    pub(super) fn show_status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.escape_mode {
                    ui.label(
                        egui::RichText::new("[ESCAPE MODE]")
                            .color(egui::Color32::YELLOW)
                            .strong(),
                    );
                }

                if let Some(hex) = &self.hex_input {
                    ui.label(
                        egui::RichText::new(format!("U+{hex}"))
                            .color(egui::Color32::from_rgb(100, 200, 255))
                            .strong(),
                    );
                } else if self.shaped_preview.is_focused()
                    && let Some(label) = self.shaped_preview.selection_codepoints_label() {
                        ui.label(label);
                    }
                else if let Some(status) = &self.specimen.hover_status {
                    ui.label(status);
                }
                else if let Some((msg, time)) = &self.status_message
                    && time.elapsed().as_secs() < 5 {
                        ui.label(msg);
                    }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // The zoom level is per pane, so the one shown is the
                    // focused pane's, alongside that pane's file name.
                    let zoom_level = self.panes.focused().zoom_level;
                    if zoom_level > 1 {
                        ui.label(format!("{zoom_level}x"));
                    }
                    if let Some(idx) = self.active_doc_idx()
                        && let Some(doc) = self.open_documents.get(idx) {
                            let name = doc
                                .document
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default();
                            let line = doc.editor_state.cursor_source_line();
                            let dirty = if doc.document.dirty { " [modified]" } else { "" };
                            ui.label(format!("{name}:{line}{dirty}"));
                        }
                    for (label, phase) in [
                        ("Build", &self.bg_tasks.build),
                        ("Test", &self.bg_tasks.test),
                    ] {
                        if let Some(phase) = phase {
                            let (text, color) = match phase {
                                BackgroundTaskPhase::Running(start) => {
                                    let secs = start.elapsed().as_secs_f64();
                                    (format!("{label} {secs:.1}s"), egui::Color32::from_rgb(100, 180, 255))
                                }
                                BackgroundTaskPhase::Finished(_, dur) => {
                                    let secs = dur.as_secs_f64();
                                    (format!("{label} {secs:.1}s"), egui::Color32::from_rgb(100, 200, 100))
                                }
                            };
                            ui.label(egui::RichText::new(format!("{{{text}}}")).color(color));
                        }
                    }
                });
            });
        });
    }

    /// The bottom Preview/Specimen/Issues panel.  Returns any specimen glyph
    /// click and any issue-row click for post-frame navigation.
    pub(super) fn show_bottom_panel(
        &mut self,
        ctx: &egui::Context,
    ) -> (
        Option<crate::specimen::SpecimenClick>,
        Option<(PathBuf, usize)>,
    ) {
        let mut specimen_clicked_glyph: Option<crate::specimen::SpecimenClick> = None;
        let mut issues_click: Option<(PathBuf, usize)> = None;
        let mut preview_rect: Option<egui::Rect> = None;
        let bottom_panel_expanded = self.bottom_panel_tab.is_some();
        if self.bottom_panel_height_override {
            self.bottom_panel_height_override = false;
            let panel_id = egui::Id::new("bottom_panel");
            if let Some(mut state) =
                ctx.data_mut(|d| d.get_persisted::<egui::panel::PanelState>(panel_id))
            {
                let h = state.rect.height();
                if h < self.bottom_panel_height {
                    state.rect.set_height(self.bottom_panel_height);
                    ctx.data_mut(|d| d.insert_persisted(panel_id, state));
                }
            }
        }
        let mut bottom_panel = egui::TopBottomPanel::bottom("bottom_panel")
            .resizable(bottom_panel_expanded);
        if bottom_panel_expanded {
            bottom_panel = bottom_panel
                .default_height(self.bottom_panel_height)
                .min_height(100.0);
        }
        bottom_panel.show(ctx, |ui| {
                if bottom_panel_expanded {
                    self.bottom_panel_height = ui.available_height();
                }
                ui.horizontal(|ui| {
                    let screen_h = ui.ctx().input(|i| i.screen_rect.height());
                    for (idx, label) in [(0, "Preview"), (1, "Specimen")] {
                        let selected = self.bottom_panel_tab == Some(idx);
                        if ui.selectable_label(selected, label).clicked() {
                            if selected {
                                self.bottom_panel_tab = None;
                            } else {
                                self.bottom_panel_tab = Some(idx);
                                self.ensure_min_panel_height(screen_h);
                            }
                        }
                    }
                    let total_issues = self.issues.len() + self.assert_issues.len();
                    let issues_label = if total_issues == 0 {
                        "Issues".to_string()
                    } else {
                        let errors = self.issues.iter().chain(self.assert_issues.iter())
                            .filter(|i| i.severity == crate::issues::Severity::Error)
                            .count();
                        let warnings = total_issues - errors;
                        let mut parts = Vec::new();
                        if errors > 0 { parts.push(format!("{errors} error{}", if errors == 1 { "" } else { "s" })); }
                        if warnings > 0 { parts.push(format!("{warnings} warning{}", if warnings == 1 { "" } else { "s" })); }
                        format!("Issues ({})", parts.join(", "))
                    };
                    let issues_selected = self.bottom_panel_tab == Some(2);
                    if ui.selectable_label(issues_selected, issues_label).clicked() {
                        if issues_selected {
                            self.bottom_panel_tab = None;
                        } else {
                            self.bottom_panel_tab = Some(2);
                            self.ensure_min_panel_height(screen_h);
                        }
                    }
                });
                if self.bottom_panel_tab != Some(1) {
                    self.specimen.hover_status = None;
                }
                if self.bottom_panel_tab.is_none() {
                    return;
                }
                ui.separator();
                match self.bottom_panel_tab {
                    Some(0) => {
                        ui.horizontal(|ui| {
                            ui.label("Font size:");
                            let slider_resp = ui.add(
                                egui::Slider::new(
                                    &mut self.preview_font_size_slider,
                                    16.0..=128.0,
                                )
                                .show_value(false)
                                .step_by(16.0)
                                .fixed_decimals(0),
                            );
                            if slider_resp.changed() {
                                self.preview_font_size = self.preview_font_size_slider;
                            }
                            let drag_resp = ui.add(
                                egui::DragValue::new(&mut self.preview_font_size)
                                    .range(16.0..=128.0)
                                    .suffix("px")
                                    .fixed_decimals(0)
                                    .speed(1.0),
                            );
                            if drag_resp.changed() {
                                self.preview_font_size_slider =
                                    (self.preview_font_size / 16.0).round() * 16.0;
                            }
                            ui.separator();
                            self.shaped_preview.show_engine_combo(ui);
                            ui.separator();
                            ui.checkbox(&mut self.shaped_preview.color_font, "Color");
                        });
                        ui.separator();
                        self.shaped_preview.show(
                            ui,
                            self.font_data.as_ref(),
                            self.font_data_gen,
                            self.preview_font_size,
                        );
                        preview_rect = self.shaped_preview.last_rect();
                    }
                    Some(1) => {
                        if self.specimen.needs_rebuild(self.font_build_gen) {
                            let all_docs = collect_effective_docs(
                                &self.open_documents,
                                &self.font_base_docs,
                            );
                            self.specimen.rebuild_if_needed(
                                &all_docs, &self.name_parts, &self.font_name_to_gid, self.font_build_gen,
                            );
                        }
                        specimen_clicked_glyph = self.specimen.show(
                            ui,
                            self.font_data.as_ref(),
                            self.font_data_gen,
                        );
                    }
                    Some(2) => {
                        let mut all_issues: Vec<&Issue> = self.issues.iter().collect();
                        all_issues.extend(self.assert_issues.iter());
                        show_issues_tab(ui, &all_issues, &mut issues_click);
                    }
                    _ => {}
                }
            });

        self.preview_view_rect = preview_rect;
        (specimen_clicked_glyph, issues_click)
    }

    /// One pane's contents: the editor for its document, or the placeholder.
    /// Returns the pane's screen rect for zoom routing, which stays `None` for
    /// the placeholder — that is not an editor, so hovering it must not make
    /// Cmd/Ctrl + wheel zoom anything.
    fn show_pane(
        &mut self,
        ui: &mut egui::Ui,
        pane_idx: usize,
        nav_request: &mut Option<(usize, crate::editor::document_view::NavRequest)>,
        rename_request: &mut Option<crate::editor::document_view::RenameAction>,
    ) -> Option<egui::Rect> {
        let pane = self.panes.get(pane_idx)?;
        let zoom_level = pane.zoom_level;
        let Some(doc_idx) = pane.doc_idx else {
            ui.centered_and_justified(|ui| {
                if self.font_dir.is_some() {
                    ui.label("Select a file from the sidebar");
                } else {
                    ui.label("Usage: uniform <font-directory>");
                }
            });
            return None;
        };
        let doc = self.open_documents.get_mut(doc_idx)?;

        let rect = ui.max_rect();
        let font_size = 16.0 * zoom_level as f32;
        let editor_font_id = if self.escape_mode {
            egui::FontId::new(font_size, egui::FontFamily::Monospace)
        } else {
            uniform_font_id(ui.ctx(), font_size)
        };
        let env = crate::editor::document_view::EditorEnv {
            named_glyphs: &self.named_glyphs,
            name_parts: &self.name_parts,
            alt_index: &self.alt_index,
            color_aliases: &self.color_aliases,
            derived_gen: self.derived_gen,
            font_gen: self.font_data_gen,
            zoom_level,
            font_id: &editor_font_id,
        };
        let result = crate::editor::document_view::DocumentEditor::new(
            &mut doc.document,
            &mut doc.lines,
            &mut doc.editor_state,
            env,
        )
        .show(ui);
        if let Some(nav) = result.nav {
            *nav_request = Some((doc_idx, nav));
        }
        if let Some(rename) = result.rename {
            *rename_request = Some(rename);
        }
        Some(rect)
    }

    /// The central editor panel: one pane, or two side by side with a
    /// draggable divider.  Returns any navigation/rename request produced by a
    /// document view for post-frame dispatch (the navigation one tagged with
    /// the document it came from), plus the pane a divider drop asked to close.
    pub(super) fn show_editor_panel(
        &mut self,
        ctx: &egui::Context,
    ) -> (
        Option<(usize, crate::editor::document_view::NavRequest)>,
        Option<crate::editor::document_view::RenameAction>,
        Option<usize>,
    ) {
        let mut nav_request = None;
        let mut rename_request = None;
        let mut divider_closed_pane = None;
        let mut pane_rects = [None, None];
        egui::CentralPanel::default().show(ctx, |ui| {
            if !self.panes.is_split() {
                pane_rects[0] =
                    self.show_pane(ui, 0, &mut nav_request, &mut rename_request);
                return;
            }

            let full = ui.max_rect();
            let (left, divider, right) = split_layout(full, self.panes.split_ratio);
            let drag = self.show_divider(ui, divider, full);
            divider_closed_pane = drag.closed;

            for (idx, rect) in [(0, left), (1, right)] {
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .layout(*ui.layout()),
                );
                child.set_clip_rect(rect);
                pane_rects[idx] =
                    self.show_pane(&mut child, idx, &mut nav_request, &mut rename_request);
            }

            // With one sidebar for two panes, which pane an opened file lands
            // in is the focused one, so the focus has to be visible.
            let focused_rect = if self.panes.focus() == 0 { left } else { right };
            ui.painter().rect_stroke(
                focused_rect.shrink(0.5),
                0.0,
                egui::Stroke::new(1.0, ui.visuals().selection.bg_fill),
                egui::StrokeKind::Inside,
            );

            // The layout keeps both panes at their minimum width, so a drag
            // that has reached the closing zone would otherwise look like a
            // divider that simply stopped moving. Shade the pane it would
            // close instead.
            if let Some(idx) = drag.would_close {
                let doomed = if idx == 0 { left } else { right };
                ui.painter().rect_filled(
                    doomed,
                    0.0,
                    ui.visuals().extreme_bg_color.gamma_multiply(0.7),
                );
            }
        });

        for (idx, rect) in pane_rects.into_iter().enumerate() {
            if let Some(pane) = self.panes.get_mut(idx) {
                pane.view_rect = rect;
            }
        }
        (nav_request, rename_request, divider_closed_pane)
    }

    /// The draggable divider between two panes.
    fn show_divider(
        &mut self,
        ui: &mut egui::Ui,
        divider: egui::Rect,
        full: egui::Rect,
    ) -> DividerDrag {
        // The visible divider is a hairline; the grab area around it is not.
        let resp = ui.interact(
            divider.expand2(egui::vec2(3.0, 0.0)),
            ui.id().with("pane_divider"),
            egui::Sense::drag(),
        );
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if resp.dragged()
            && let Some(pos) = ui.ctx().input(|i| i.pointer.latest_pos())
            && full.width() > 0.0
        {
            self.panes.split_ratio = ((pos.x - full.left()) / full.width()).clamp(0.0, 1.0);
        }
        let at_edge = self.panes.pane_closed_by_ratio(self.panes.split_ratio);
        let closed = if resp.drag_stopped() { at_edge } else { None };
        if resp.drag_stopped() && closed.is_none() {
            // Snap the divider back inside the minimum widths it was allowed
            // to visually ignore during the drag.
            self.panes.split_ratio = clamp_ratio(self.panes.split_ratio, full.width());
        }

        let stroke = if resp.hovered() || resp.dragged() {
            ui.visuals().widgets.hovered.bg_fill
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke.color
        };
        ui.painter().rect_filled(divider, 0.0, stroke);
        DividerDrag {
            closed,
            would_close: resp.dragged().then_some(at_edge).flatten(),
        }
    }
}

/// What one frame of divider interaction asked for.
struct DividerDrag {
    /// A drop at an edge closed this pane.
    closed: Option<usize>,
    /// A drag currently held at an edge would close this pane if released.
    would_close: Option<usize>,
}

/// Keeps a divider ratio far enough from either edge that both panes stay
/// usable. The drag itself is not clamped, so a drop at an edge can still be
/// read as "close that pane".
fn clamp_ratio(ratio: f32, width: f32) -> f32 {
    if width <= super::panes::MIN_PANE_WIDTH * 2.0 {
        return 0.5;
    }
    let margin = super::panes::MIN_PANE_WIDTH / width;
    ratio.clamp(margin, 1.0 - margin)
}

/// Splits `full` into the left pane, the divider and the right pane.
fn split_layout(full: egui::Rect, ratio: f32) -> (egui::Rect, egui::Rect, egui::Rect) {
    const DIVIDER_WIDTH: f32 = 4.0;
    let usable = (full.width() - DIVIDER_WIDTH).max(0.0);
    let ratio = clamp_ratio(ratio, full.width());
    let left_w = (usable * ratio).round();
    let left = egui::Rect::from_min_size(full.min, egui::vec2(left_w, full.height()));
    let divider = egui::Rect::from_min_size(
        egui::pos2(left.right(), full.top()),
        egui::vec2(DIVIDER_WIDTH, full.height()),
    );
    let right = egui::Rect::from_min_max(
        egui::pos2(divider.right(), full.top()),
        full.max,
    );
    (left, divider, right)
}

#[cfg(test)]
mod pane_layout_tests {
    use super::*;

    fn rect(w: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(10.0, 0.0), egui::vec2(w, 100.0))
    }

    #[test]
    fn split_layout_tiles_the_area_without_gaps_or_overlap() {
        let full = rect(804.0);
        let (left, divider, right) = split_layout(full, 0.5);
        assert_eq!(left.left(), full.left());
        assert_eq!(left.right(), divider.left());
        assert_eq!(divider.right(), right.left());
        assert_eq!(right.right(), full.right());
        assert_eq!(left.width(), 400.0);
        assert_eq!(right.width(), 400.0);
    }

    #[test]
    fn a_dragged_ratio_never_collapses_a_pane_in_the_layout() {
        let full = rect(804.0);
        // The drag itself may reach the edge (that is how a drop closes a
        // pane), but the laid-out panes keep their minimum width.
        let (left, _, right) = split_layout(full, 0.0);
        assert!(left.width() >= super::super::panes::MIN_PANE_WIDTH - 1.0);
        assert!(right.width() > 0.0);
        let (left, _, right) = split_layout(full, 1.0);
        assert!(right.width() >= super::super::panes::MIN_PANE_WIDTH - 1.0);
        assert!(left.width() > 0.0);
    }

    #[test]
    fn a_too_narrow_area_falls_back_to_an_even_split() {
        assert_eq!(clamp_ratio(0.0, 100.0), 0.5);
        assert_eq!(clamp_ratio(1.0, 100.0), 0.5);
    }
}
