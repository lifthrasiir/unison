//! The surrounding panels: sidebar, status bar, bottom preview panel, editor
//! panel and the issues tab.

use super::background::BackgroundTaskPhase;
use super::docs::collect_effective_docs;
use super::search::{SearchHit, SearchResults};
use super::*;
use crate::issues::Severity;

fn min_bottom_panel_height(screen_height: f32) -> f32 {
    270.0_f32.min(screen_height * 0.5)
}

pub(super) const SEARCH_TAB: usize = 3;

/// Reopening the bottom panel: the height it comes back at, and the override
/// that gets it there.
///
/// The override is unconditional on purpose. While the panel is collapsed egui
/// keeps persisting a `PanelState` no taller than the tab row, and it prefers
/// that state to `default_height`, so the remembered height has to be written
/// back on *every* reopen — not only when it falls below the minimum. Guarding
/// it by the minimum meant a panel dragged taller than that came back at
/// `min_height` instead.
fn open_panel_height(remembered: &mut f32, override_state: &mut bool, screen_height: f32) {
    *remembered = remembered.max(min_bottom_panel_height(screen_height));
    *override_state = true;
}

/// Shared by the panel itself and by `Sidebar::fit_panel_width`, which reaches
/// into the panel's stored width under this id.
pub(super) const SIDEBAR_PANEL_ID: &str = "sidebar";

/// What one frame of the bottom panel asked the host to do. Every case is a
/// navigation the panel itself cannot carry out, so all three are dispatched
/// after the frame's editors have run.
#[derive(Default)]
pub(super) struct BottomPanelResult {
    pub specimen_click: Option<crate::specimen::SpecimenClick>,
    /// `(file, docline index)` of a clicked diagnostic.
    pub issue_click: Option<(PathBuf, usize)>,
    /// Index into the current search results.
    pub search_click: Option<usize>,
}

/// One result row's source line, with the token that matched picked out.
///
/// The whole line is shown for context, so without this a row of a long
/// `remap` or `assert` line says nothing about *where* on it the name is — and
/// a line naming the same glyph twice is two rows that would otherwise look
/// identical. The span is the written token, backticks and an anchor's sign
/// included, so what is highlighted is what the search actually matched.
fn hit_text(ui: &egui::Ui, hit: &SearchHit) -> egui::text::LayoutJob {
    let font = egui::FontId::new(16.0, egui::FontFamily::Proportional);
    let plain = egui::TextFormat {
        font_id: font.clone(),
        color: ui.visuals().text_color(),
        ..Default::default()
    };
    let matched = egui::TextFormat {
        font_id: font,
        color: ui.visuals().strong_text_color(),
        background: ui.visuals().selection.bg_fill.gamma_multiply(0.5),
        ..Default::default()
    };

    let chars: Vec<char> = hit.text.chars().collect();
    let start = hit.highlight.0.min(chars.len());
    let end = hit.highlight.1.clamp(start, chars.len());
    let part = |range: std::ops::Range<usize>| chars[range].iter().collect::<String>();

    let mut job = egui::text::LayoutJob::default();
    job.append(&part(0..start), 0.0, plain.clone());
    job.append(&part(start..end), 0.0, matched);
    job.append(&part(end..chars.len()), 0.0, plain);
    job
}

/// Rows of "where this name is written", in the diagnostics list's format: the
/// source line on the left, the place it came from on the right.
fn show_search_tab(ui: &mut egui::Ui, search: Option<&SearchResults>, click: &mut Option<usize>) {
    let Some(search) = search else {
        ui.centered_and_justified(|ui| {
            ui.label("Ctrl/Cmd+click a name to list every place it is written");
        });
        return;
    };

    ui.label(
        egui::RichText::new(search.title())
            .size(16.0)
            .color(ui.visuals().weak_text_color()),
    );
    if search.hits.is_empty() {
        return;
    }
    ui.separator();

    // The list is declarations first (`collect_hits` sorts it), so the one
    // place the two groups meet gets a rule between them. Nothing is drawn
    // when a group is empty: a kind with no declaration site — an anchor, a
    // feature tag — would otherwise carry a heading claiming it has one.
    let split = search
        .hits
        .iter()
        .position(|h| !h.is_decl)
        .filter(|&i| i > 0 && i < search.hits.len());

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (hit_idx, hit) in search.hits.iter().enumerate() {
            if split == Some(hit_idx) {
                ui.add_space(2.0);
                ui.separator();
                ui.add_space(2.0);
            }
            let file_name = hit
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let location = format!("{file_name}:{}", hit.file_line);

            let row_id = ui.id().with(("search_row", hit_idx));
            let resp = ui.horizontal(|ui| {
                ui.label(hit_text(ui, hit));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(&location)
                            .size(16.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                });
            });

            let click_resp = ui.interact(resp.response.rect, row_id, egui::Sense::click());
            if click_resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if click_resp.clicked() {
                *click = Some(hit_idx);
            }
        }
    });
}

/// Which severities the issue list shows.
///
/// Per severity rather than a single "minimum severity": the four are not one
/// scale. A [`Severity::Todo`] queue tens of thousands long and a handful of
/// errors are two different lists that happen to share a panel, and reading
/// either one means hiding the other — which is also why a right-click on a
/// button is *solo* rather than another toggle.
///
/// Notes are the one thing off to begin with: they ask for no action, so they
/// are what someone opting in goes looking for rather than what the panel opens
/// on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(super) struct IssueFilter {
    pub errors: bool,
    pub warnings: bool,
    pub todos: bool,
    pub notes: bool,
}

impl Default for IssueFilter {
    fn default() -> Self {
        Self {
            errors: true,
            warnings: true,
            todos: true,
            notes: false,
        }
    }
}

impl IssueFilter {
    fn slot(&mut self, severity: Severity) -> &mut bool {
        match severity {
            Severity::Error => &mut self.errors,
            Severity::Warning => &mut self.warnings,
            Severity::Todo => &mut self.todos,
            Severity::Note => &mut self.notes,
        }
    }

    pub(super) fn shows(self, severity: Severity) -> bool {
        match severity {
            Severity::Error => self.errors,
            Severity::Warning => self.warnings,
            Severity::Todo => self.todos,
            Severity::Note => self.notes,
        }
    }

    fn toggle(&mut self, severity: Severity) {
        let slot = self.slot(severity);
        *slot = !*slot;
    }

    /// Right-click: this severity and nothing else.
    fn only(&mut self, severity: Severity) {
        *self = Self {
            errors: false,
            warnings: false,
            todos: false,
            notes: false,
        };
        *self.slot(severity) = true;
    }
}

/// The mark that stands for a severity wherever one is drawn: the filter
/// buttons, the tab label and every row.
///
/// `✖`/`⚠` come from the fallback font, `□`/`○` from the font being edited —
/// which is on purpose only in that the four have to stay apart at 16px in
/// whichever font answers for them.
fn severity_icon(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "\u{2716}",
        Severity::Warning => "\u{26A0}",
        Severity::Todo => "\u{25A1}",
        Severity::Note => "\u{25CB}",
    }
}

fn severity_color(ui: &egui::Ui, severity: Severity) -> egui::Color32 {
    match severity {
        Severity::Error => egui::Color32::from_rgb(220, 60, 60),
        Severity::Warning => egui::Color32::from_rgb(200, 180, 50),
        Severity::Todo => egui::Color32::from_rgb(90, 160, 230),
        Severity::Note => ui.visuals().weak_text_color(),
    }
}

/// How many issues of each severity there are, in [`Severity::ALL`] order.
pub(super) fn severity_counts(issues: &[&Issue]) -> [usize; Severity::ALL.len()] {
    let mut counts = [0usize; Severity::ALL.len()];
    for issue in issues {
        if let Some(i) = Severity::ALL.iter().position(|s| *s == issue.severity) {
            counts[i] += 1;
        }
    }
    counts
}

/// A count as it appears on the *tab*, which has a whole panel's worth of other
/// tabs to sit beside: three digits at most, so that a queue going from 20,000
/// to 19,999 does not resize the tab strip under the pointer.
pub(super) fn compact_count(n: usize) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..10_000 => format!("{:.1}k", n as f64 / 1_000.0),
        10_000..1_000_000 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// The Issues tab's own label: the icons carry the counts, since the exact
/// numbers (and the names) are on the filter buttons inside the tab.
pub(super) fn issues_tab_label(counts: [usize; Severity::ALL.len()]) -> String {
    let mut label = "Issues".to_string();
    for (sev, count) in Severity::ALL.iter().zip(counts) {
        if count > 0 {
            label.push(' ');
            label.push_str(severity_icon(*sev));
            label.push_str(&compact_count(count));
        }
    }
    label
}

/// The row of severity buttons above the list: left-click toggles one,
/// right-click keeps only that one.
fn show_issue_filter(ui: &mut egui::Ui, filter: &mut IssueFilter, counts: [usize; 4]) {
    ui.horizontal_wrapped(|ui| {
        for (sev, count) in Severity::ALL.iter().zip(counts) {
            let sev = *sev;
            let mut text = egui::text::LayoutJob::default();
            let font = egui::FontId::new(16.0, egui::FontFamily::Proportional);
            text.append(
                severity_icon(sev),
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: severity_color(ui, sev),
                    ..Default::default()
                },
            );
            text.append(
                &format!(" {count} {}", sev.plural()),
                4.0,
                egui::TextFormat {
                    font_id: font,
                    color: if filter.shows(sev) {
                        ui.visuals().text_color()
                    } else {
                        ui.visuals().weak_text_color()
                    },
                    ..Default::default()
                },
            );
            let resp = ui
                .selectable_label(filter.shows(sev), text)
                .on_hover_text("Click to show or hide; right-click to show only these");
            if resp.clicked() {
                filter.toggle(sev);
            }
            if resp.secondary_clicked() {
                filter.only(sev);
            }
        }
    });
}

fn show_issues_tab(
    ui: &mut egui::Ui,
    issues: &[&Issue],
    filter: &mut IssueFilter,
    click: &mut Option<(PathBuf, usize)>,
) {
    let counts = severity_counts(issues);
    show_issue_filter(ui, filter, counts);
    ui.separator();

    let filter = *filter;
    let shown: Vec<&&Issue> = issues.iter().filter(|i| filter.shows(i.severity)).collect();
    if shown.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(if issues.is_empty() {
                "No issues"
            } else {
                "Nothing to show: every severity with an issue in it is filtered out"
            });
        });
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (issue_idx, issue) in shown.iter().enumerate() {
            let icon = severity_icon(issue.severity);
            let icon_color = severity_color(ui, issue.severity);
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

            let click_resp = ui.interact(resp.response.rect, row_id, egui::Sense::click());
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
    pub(super) fn open_bottom_panel(&mut self, tab: usize, screen_height: f32) {
        self.bottom_panel_tab = Some(tab);
        open_panel_height(
            &mut self.bottom_panel_height,
            &mut self.bottom_panel_height_override,
            screen_height,
        );
    }

    pub(super) fn show_sidebar_panel(&mut self, ctx: &egui::Context, editor_focused: bool) {
        let mut sidebar_actions = Vec::new();
        // The sidebar fits itself to the file names until the user drags its
        // edge, and the fit is re-applied *every* frame until then rather than
        // once at startup: the UI font is the Unison bitmap font itself, which
        // `apply_font` only installs once the first background build finishes,
        // so the width measured on frame 1 is measured in the wrong font.
        self.sidebar.fit_panel_width(ctx, SIDEBAR_PANEL_ID);
        let panel = egui::SidePanel::left(SIDEBAR_PANEL_ID)
            .default_width(200.0)
            .show(ctx, |ui| {
                let dirty_paths: Vec<&std::path::Path> = self
                    .open_documents
                    .iter()
                    .filter(|d| d.document.dirty)
                    .map(|d| d.document.path.as_path())
                    .collect();
                let open_paths: Vec<&std::path::Path> = self
                    .open_documents
                    .iter()
                    .map(|d| d.document.path.as_path())
                    .collect();
                let changed_paths: Vec<&std::path::Path> = self
                    .open_documents
                    .iter()
                    .filter(|d| d.external_change)
                    .map(|d| d.document.path.as_path())
                    .collect();
                // Field-level accesses so the `sidebar` borrow stays disjoint
                // from the `open_documents` one.
                let active_path = self
                    .panes
                    .active_doc_idx()
                    .and_then(|i| self.open_documents.get(i))
                    .map(|d| d.document.path.as_path());
                let files = crate::sidebar::SidebarFiles {
                    dirty: &dirty_paths,
                    open: &open_paths,
                    changed_on_disk: &changed_paths,
                };
                sidebar_actions = self.sidebar.show(ui, active_path, files, editor_focused);
            });
        // Kept for the file watcher: a listing refreshed while the pointer is
        // over the panel would move rows out from under a click.
        self.sidebar_rect = panel.response.rect;

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
                SidebarAction::ReloadFromDisk(path) => {
                    self.reload_from_disk(&path);
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

                // An open code point popup outranks everything else here: its
                // Unicode name is what stops a mistyped code point from being
                // committed unnoticed, so it must not be crowded out.
                let codepoint_status = self
                    .shaped_preview
                    .codepoint_status(&self.char_props)
                    .or_else(|| {
                        self.active_doc()?
                            .editor_state
                            .codepoint_status(&self.char_props)
                    });
                if let Some(label) = codepoint_status {
                    // Plain text color: it reads as the status line's own text,
                    // like every other message that shares this slot.
                    ui.label(label);
                } else if self.shaped_preview.is_focused()
                    && let Some(label) = self.shaped_preview.selection_codepoints_label()
                {
                    ui.label(label);
                } else if let Some(status) = &self.specimen.hover_status {
                    ui.label(status);
                } else if let Some((msg, time)) = &self.status_message
                    && time.elapsed().as_secs() < 5
                {
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
                        && let Some(doc) = self.open_documents.get(idx)
                    {
                        let name = doc
                            .document
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let line = doc.editor_state.cursor_source_line();
                        let dirty = if doc.document.dirty {
                            " [modified]"
                        } else {
                            ""
                        };
                        ui.label(format!("{name}:{line}{dirty}"));
                    }
                    for (label, phase) in [
                        ("Build", &self.bg_tasks.build),
                        ("Test", &self.bg_tasks.test),
                        ("Optimize", &self.bg_tasks.optimize),
                    ] {
                        if let Some(phase) = phase {
                            let (text, color) = match phase {
                                BackgroundTaskPhase::Running(start) => {
                                    let secs = start.elapsed().as_secs_f64();
                                    (
                                        format!("{label} {secs:.1}s"),
                                        egui::Color32::from_rgb(100, 180, 255),
                                    )
                                }
                                BackgroundTaskPhase::Finished(_, dur) => {
                                    let secs = dur.as_secs_f64();
                                    (
                                        format!("{label} {secs:.1}s"),
                                        egui::Color32::from_rgb(100, 200, 100),
                                    )
                                }
                            };
                            ui.label(egui::RichText::new(format!("{{{text}}}")).color(color));
                        }
                    }
                });
            });
        });
    }

    /// The bottom Preview/Specimen/Issues/Search panel.  Returns the row
    /// clicks the host has to carry out once this frame's editors have run.
    pub(super) fn show_bottom_panel(&mut self, ctx: &egui::Context) -> BottomPanelResult {
        let mut result = BottomPanelResult::default();
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
        let mut bottom_panel =
            egui::TopBottomPanel::bottom("bottom_panel").resizable(bottom_panel_expanded);
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
                            self.open_bottom_panel(idx, screen_h);
                        }
                    }
                }
                let all_issues: Vec<&Issue> = self
                    .issues
                    .iter()
                    .chain(self.assert_issues.iter())
                    .collect();
                let issues_label = issues_tab_label(severity_counts(&all_issues));
                let issues_selected = self.bottom_panel_tab == Some(2);
                if ui.selectable_label(issues_selected, issues_label).clicked() {
                    if issues_selected {
                        self.bottom_panel_tab = None;
                    } else {
                        self.open_bottom_panel(2, screen_h);
                    }
                }
                let search_label = match &self.search {
                    Some(s) if !s.hits.is_empty() => {
                        format!("Search ({})", s.hits.len())
                    }
                    _ => "Search".to_string(),
                };
                let search_selected = self.bottom_panel_tab == Some(SEARCH_TAB);
                if ui.selectable_label(search_selected, search_label).clicked() {
                    if search_selected {
                        self.bottom_panel_tab = None;
                    } else {
                        self.open_bottom_panel(SEARCH_TAB, screen_h);
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
                            egui::Slider::new(&mut self.preview_font_size_slider, 16.0..=128.0)
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
                        // Only a multi-face source has a choice to offer;
                        // see `UniformApp::set_selected_face`.
                        if self.face_ids.len() > 1 {
                            ui.separator();
                            ui.label("Face:");
                            let current = self.selected_face().to_string();
                            let mut picked: Option<String> = None;
                            egui::ComboBox::from_id_salt("preview_face")
                                .selected_text(&current)
                                .show_ui(ui, |ui| {
                                    for id in &self.face_ids {
                                        if ui.selectable_label(*id == current, id).clicked() {
                                            picked = Some(id.clone());
                                        }
                                    }
                                });
                            if let Some(id) = picked {
                                let ctx = ui.ctx().clone();
                                self.set_selected_face(id, &ctx);
                            }
                        }
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
                    // Keyed on what the rebuild actually reads (the built
                    // font's gid map and the derived name parts), not on
                    // `font_build_gen` — that is bumped the moment a doc
                    // changes, so a specimen built on it would cache a
                    // half-built state and never refresh. See
                    // `SpecimenState::cached_gen`.
                    if self
                        .specimen
                        .needs_rebuild(self.font_data_gen, self.derived_gen)
                    {
                        let all_docs =
                            collect_effective_docs(&self.open_documents, &self.font_base_docs);
                        // The face the built font bytes are for; a
                        // slice-qualified `map` reads differently per face.
                        let face = self.selected_face().to_string();
                        self.specimen.rebuild_if_needed(
                            &all_docs,
                            &self.name_parts,
                            &self.font_name_to_gid,
                            (!face.is_empty()).then_some(face.as_str()),
                            &self.glyph_flags,
                            self.font_data_gen,
                            self.derived_gen,
                        );
                    }
                    result.specimen_click =
                        self.specimen
                            .show(ui, self.font_data.as_ref(), self.font_data_gen);
                }
                Some(2) => {
                    let mut all_issues: Vec<&Issue> = self.issues.iter().collect();
                    all_issues.extend(self.assert_issues.iter());
                    show_issues_tab(
                        ui,
                        &all_issues,
                        &mut self.issue_filter,
                        &mut result.issue_click,
                    );
                }
                Some(SEARCH_TAB) => {
                    show_search_tab(ui, self.search.as_ref(), &mut result.search_click);
                }
                _ => {}
            }
        });

        self.preview_view_rect = preview_rect;
        result
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
        resize_request: &mut Option<crate::editor::glyph_resize::ResizeAction>,
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
            exists_matches: &self.exists_matches,
            alt_index: &self.alt_index,
            color_aliases: &self.color_aliases,
            meta: self.font_meta,
            show_metrics: self.show_metrics,
            menu_open: self.menu_open,
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
        if let Some(resize) = result.resize {
            *resize_request = Some(resize);
        }
        Some(rect)
    }
}

/// What one frame of the editor panel asked the host to do afterwards: a
/// navigation (tagged with the document it came from), a rename, a glyph
/// resize, and a pane a divider drop asked to close. All of them need the
/// whole app, which a document view does not have.
pub(super) struct EditorPanelResult {
    pub nav: Option<(usize, crate::editor::document_view::NavRequest)>,
    pub rename: Option<crate::editor::document_view::RenameAction>,
    pub resize: Option<crate::editor::glyph_resize::ResizeAction>,
    pub divider_closed_pane: Option<usize>,
}

impl UniformApp {
    /// The central editor panel: one pane, or two side by side with a
    /// draggable divider.  Returns whatever the document views asked the host
    /// to carry out after the frame, plus the pane a divider drop asked to
    /// close.
    pub(super) fn show_editor_panel(&mut self, ctx: &egui::Context) -> EditorPanelResult {
        let mut nav_request = None;
        let mut rename_request = None;
        let mut resize_request = None;
        let mut divider_closed_pane = None;
        let mut pane_rects = [None, None];
        egui::CentralPanel::default().show(ctx, |ui| {
            if !self.panes.is_split() {
                pane_rects[0] = self.show_pane(
                    ui,
                    0,
                    &mut nav_request,
                    &mut rename_request,
                    &mut resize_request,
                );
                return;
            }

            let full = ui.max_rect();
            let (left, divider, right) = split_layout(full, self.panes.split_ratio);
            let drag = self.show_divider(ui, divider, full);
            divider_closed_pane = drag.closed;

            for (idx, rect) in [(0, left), (1, right)] {
                let mut child =
                    ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(*ui.layout()));
                child.set_clip_rect(rect);
                pane_rects[idx] = self.show_pane(
                    &mut child,
                    idx,
                    &mut nav_request,
                    &mut rename_request,
                    &mut resize_request,
                );
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
        EditorPanelResult {
            nav: nav_request,
            rename: rename_request,
            resize: resize_request,
            divider_closed_pane,
        }
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
    let right = egui::Rect::from_min_max(egui::pos2(divider.right(), full.top()), full.max);
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

#[cfg(test)]
mod issue_filter_tests {
    use super::*;

    fn issue(severity: Severity) -> Issue {
        Issue {
            glyph: None,
            severity,
            message: String::new(),
            file: PathBuf::from("test.unf"),
            line: 0,
            file_line: 1,
        }
    }

    #[test]
    fn notes_are_the_only_severity_hidden_to_begin_with() {
        let f = IssueFilter::default();
        assert!(f.shows(Severity::Error));
        assert!(f.shows(Severity::Warning));
        assert!(f.shows(Severity::Todo));
        assert!(!f.shows(Severity::Note));
    }

    /// A left-click flips one severity and leaves the rest where they were; a
    /// right-click keeps that one and drops everything else. The two are how a
    /// 20k-long TODO queue and a three-error list share one panel.
    #[test]
    fn clicking_toggles_one_and_right_clicking_solos_it() {
        let mut f = IssueFilter::default();
        f.toggle(Severity::Todo);
        assert!(!f.shows(Severity::Todo));
        assert!(f.shows(Severity::Error));
        f.toggle(Severity::Todo);
        assert!(f.shows(Severity::Todo));

        f.only(Severity::Todo);
        assert!(f.shows(Severity::Todo));
        for other in [Severity::Error, Severity::Warning, Severity::Note] {
            assert!(!f.shows(other), "{other:?}");
        }
        // Including a severity that starts out hidden.
        f.only(Severity::Note);
        assert!(f.shows(Severity::Note));
        assert!(!f.shows(Severity::Todo));
    }

    #[test]
    fn counts_are_in_severity_order() {
        let issues = [
            issue(Severity::Warning),
            issue(Severity::Todo),
            issue(Severity::Error),
            issue(Severity::Todo),
        ];
        let refs: Vec<&Issue> = issues.iter().collect();
        assert_eq!(severity_counts(&refs), [1, 1, 2, 0]);
    }

    /// The tab has the rest of the tab strip to sit beside, so a count there is
    /// three digits at most and a severity nobody has is not mentioned at all.
    #[test]
    fn the_tab_label_is_icons_and_short_counts() {
        assert_eq!(issues_tab_label([0, 0, 0, 0]), "Issues");
        assert_eq!(
            issues_tab_label([2, 0, 20_000, 0]),
            format!(
                "Issues {}2 {}20k",
                severity_icon(Severity::Error),
                severity_icon(Severity::Todo)
            )
        );

        assert_eq!(compact_count(999), "999");
        assert_eq!(compact_count(1_000), "1.0k");
        assert_eq!(compact_count(1_234), "1.2k");
        assert_eq!(compact_count(19_999), "19k");
        assert_eq!(compact_count(2_400_000), "2.4M");
    }
}

#[cfg(test)]
mod bottom_panel_height_tests {
    use super::*;

    #[test]
    fn reopening_restores_a_panel_taller_than_the_minimum() {
        // While the panel is collapsed egui keeps persisting a `PanelState`
        // that is only as tall as the tab row, and it prefers that state to
        // `default_height`. So *every* reopen has to override it, not just the
        // ones that fall below the minimum height.
        let (mut h, mut override_state) = (600.0_f32, false);
        open_panel_height(&mut h, &mut override_state, 1000.0);
        assert_eq!(h, 600.0);
        assert!(override_state);
    }

    #[test]
    fn reopening_a_short_panel_grows_it_to_the_minimum() {
        let (mut h, mut override_state) = (40.0_f32, false);
        open_panel_height(&mut h, &mut override_state, 1000.0);
        assert_eq!(h, min_bottom_panel_height(1000.0));
        assert!(override_state);
    }
}
