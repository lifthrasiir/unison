use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
enum EditState {
    None,
    Renaming {
        index: usize,
        text: String,
        error: bool,
        focus_set: bool,
    },
    NewFile {
        text: String,
        error: bool,
        focus_set: bool,
    },
}

pub enum SidebarAction {
    OpenFile(PathBuf),
    FileRenamed {
        old: PathBuf,
        new: PathBuf,
    },
    FileCreated(PathBuf),
    /// Throw the buffer away and take the file on disk instead.
    ReloadFromDisk(PathBuf),
}

/// How the host wants each row drawn, and which of them the row commands act
/// on. Paths the host does not list here are simply files on disk.
#[derive(Clone, Copy, Default)]
pub struct SidebarFiles<'a> {
    /// Files with unsaved edits, marked with a leading `*`.
    pub dirty: &'a [&'a std::path::Path],
    /// Files whose buffer is open, whether or not a pane is showing it.
    pub open: &'a [&'a std::path::Path],
    /// Files that changed on disk while their buffer had unsaved edits. Drawn
    /// in the warning colour: the sidebar is the only place a file no pane is
    /// showing can say so.
    pub changed_on_disk: &'a [&'a std::path::Path],
}

pub struct Sidebar {
    files: Vec<PathBuf>,
    directory: Option<PathBuf>,
    edit_state: EditState,
    /// Whether the panel width still follows the file names. Dragging the
    /// panel's edge turns it off, and opening another directory turns it back
    /// on. Reloads after a rename or a new file leave it as it is: they must
    /// not undo a width the user dragged to.
    auto_width: bool,
    /// Where the file list's vertical scroll bar landed last frame. Only the
    /// test that pins it to the panel's edge reads it.
    #[cfg(test)]
    last_scroll_bar_rect: Option<egui::Rect>,
}

impl Sidebar {
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            directory: None,
            edit_state: EditState::None,
            auto_width: true,
            #[cfg(test)]
            last_scroll_bar_rect: None,
        }
    }

    #[expect(unused)]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    pub fn set_directory(&mut self, dir: &Path) {
        self.directory = Some(dir.to_path_buf());
        self.reload_files();
        self.auto_width = true;
    }

    /// Writes `desired_width` into the side panel's stored width, so the panel
    /// shows every file name in full — until the user drags its edge, which
    /// hands the width over for good (see `auto_width`).
    ///
    /// Called before the panel is shown, so it has to go through the panel's own
    /// stored state; `SidePanel::default_width` applies on the *first* frame
    /// only, which is exactly the frame whose measurement is wrong.
    pub fn fit_panel_width(&mut self, ctx: &egui::Context, panel_id: impl Into<egui::Id>) {
        let panel_id = panel_id.into();
        // Read the drag the same way the panel does: from last frame's response,
        // before the panel reads it to move its own edge.
        let resizing = ctx
            .read_response(panel_id.with("__resize"))
            .is_some_and(|r| r.dragged());
        if resizing {
            self.auto_width = false;
        }
        if !self.auto_width {
            return;
        }
        let width = self.desired_width(ctx);
        ctx.data_mut(|d| {
            d.insert_persisted(
                panel_id,
                egui::containers::panel::PanelState {
                    // Only the width is ever read back.
                    rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 0.0)),
                },
            );
        });
    }

    /// Width that shows every file name in full: the widest row plus the dirty
    /// marker (measured for every row, so marking a file dirty later cannot
    /// truncate it), the selectable label's padding, and the scroll bar.
    pub fn desired_width(&self, ctx: &egui::Context) -> f32 {
        const MIN_WIDTH: f32 = 120.0;
        const MAX_WIDTH: f32 = 400.0;

        let style = ctx.style();
        let font_id = egui::TextStyle::Button.resolve(&style);
        let widest = self
            .files
            .iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let galley = ctx.fonts(|f| {
                    f.layout_no_wrap(format!("* {name}"), font_id.clone(), egui::Color32::WHITE)
                });
                galley.size().x
            })
            .fold(0.0f32, f32::max);

        let chrome = style.spacing.button_padding.x * 2.0
            + style.spacing.scroll.bar_width
            + style.spacing.scroll.bar_inner_margin
            + style.spacing.scroll.bar_outer_margin
            + 2.0 * egui::Frame::side_top_panel(&style).inner_margin.leftf();
        (widest + chrome).clamp(MIN_WIDTH, MAX_WIDTH)
    }

    /// Takes a listing the host already has. The file watcher reads the
    /// directory on its scan thread, so refreshing the panel after an external
    /// change costs the UI thread no `read_dir` of its own — which over SMB is
    /// a round trip nobody should pay for between two frames.
    pub fn set_files(&mut self, files: Vec<PathBuf>) {
        self.files = files;
        self.files
            .retain(|path| crate::document_io::is_source_file(path));
        Self::sort_files(&mut self.files);
    }

    /// Whether a rename or a new-file field is up. The listing must not be
    /// re-read under one: the rename field is addressed by row index.
    pub fn is_editing(&self) -> bool {
        !matches!(self.edit_state, EditState::None)
    }

    fn reload_files(&mut self) {
        self.files.clear();
        if let Some(dir) = &self.directory
            && let Ok(entries) = std::fs::read_dir(dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if crate::document_io::is_source_file(&path) {
                    self.files.push(path);
                }
            }
        }
        // Sort by the name without its extension: every file here is `.unf`, so
        // the extension only makes `num-roman.unf` sort before `num.unf` (`-` <
        // `.`) — an ordering nothing on screen explains.
        Self::sort_files(&mut self.files);
    }

    fn sort_files(files: &mut [PathBuf]) {
        let key = |p: &Path| {
            p.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        };
        files.sort_by(|a, b| key(a).cmp(&key(b)).then_with(|| a.cmp(b)));
    }

    pub fn start_rename(&mut self, path: &Path) {
        if let Some(idx) = self.files.iter().position(|f| f == path) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            self.edit_state = EditState::Renaming {
                index: idx,
                text: name,
                error: false,
                focus_set: false,
            };
        }
    }

    pub fn start_new_file(&mut self) {
        self.edit_state = EditState::NewFile {
            text: String::new(),
            error: false,
            focus_set: false,
        };
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        active_path: Option<&Path>,
        files: SidebarFiles<'_>,
        editor_focused: bool,
    ) -> Vec<SidebarAction> {
        let mut actions = Vec::new();

        // `SidePanel` stores its *content's* rect as the width for the next
        // frame, and a scroll area of labels is only as wide as its widest
        // label — so without this the panel shrinks to the labels every frame
        // and the width it was given (dragged or fitted) is lost.
        ui.set_min_width(ui.available_width());

        let show_empty_label =
            self.files.is_empty() && !matches!(self.edit_state, EditState::NewFile { .. });

        if matches!(self.edit_state, EditState::None) && !editor_focused {
            let f2 = ui.input(|i| i.key_pressed(egui::Key::F2));
            if f2
                && ui.rect_contains_pointer(ui.max_rect())
                && let Some(active) = active_path
            {
                self.start_rename(active);
            }
        }

        // Horizontal auto-shrink off: a scroll area of labels is only as wide as
        // its widest label, and the scroll bar sits at the *area's* right edge —
        // so a panel wider than the names would draw the bar next to the longest
        // name instead of at the panel's edge.
        let scroll_out = egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if show_empty_label {
                    ui.label("No .unf files found");
                }

                let mut cancel_edit = false;
                let mut i = 0;
                while i < self.files.len() {
                    let path = self.files[i].clone();

                    if let EditState::Renaming { index, .. } = &self.edit_state
                        && *index == i
                    {
                        let result = self.show_edit_field(ui);
                        match result {
                            EditFieldResult::Pending => {}
                            EditFieldResult::Cancel => cancel_edit = true,
                            EditFieldResult::Confirm(raw_name) => {
                                let Some(new_name) = Self::sanitize_filename(&raw_name) else {
                                    self.set_edit_error("Invalid file name.");
                                    i += 1;
                                    continue;
                                };
                                let dir = path.parent().unwrap();
                                let new_path = dir.join(&new_name);
                                if new_path == path {
                                    cancel_edit = true;
                                } else if new_path.exists() {
                                    self.set_edit_error("A file with that name already exists.");
                                } else {
                                    match std::fs::rename(&path, &new_path) {
                                        Ok(()) => {
                                            actions.push(SidebarAction::FileRenamed {
                                                old: path,
                                                new: new_path,
                                            });
                                            cancel_edit = true;
                                            self.reload_files();
                                            continue;
                                        }
                                        Err(e) => {
                                            self.set_edit_error(&format!(
                                                "Failed to rename file: {e}"
                                            ));
                                        }
                                    }
                                }
                                i += 1;
                                continue;
                            }
                        }
                        i += 1;
                        continue;
                    }

                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();

                    let is_active = active_path == Some(path.as_path());
                    let is_dirty = files.dirty.contains(&path.as_path());
                    let is_open = files.open.contains(&path.as_path());
                    let changed_on_disk = files.changed_on_disk.contains(&path.as_path());

                    let label = if is_dirty {
                        format!("* {name}")
                    } else {
                        name.clone()
                    };
                    // Colour rather than another marker: the panel's width is
                    // fitted to `"* {name}"`, so a wider prefix would truncate.
                    let label = if changed_on_disk {
                        egui::RichText::new(label).color(ui.visuals().warn_fg_color)
                    } else {
                        egui::RichText::new(label)
                    };

                    let resp = ui.selectable_label(is_active, label);
                    if resp.clicked() {
                        actions.push(SidebarAction::OpenFile(path.clone()));
                    }
                    if is_open {
                        resp.context_menu(|ui| {
                            let label = if is_dirty {
                                "Reload from disk, discarding changes..."
                            } else {
                                "Reload from disk"
                            };
                            if ui.button(label).clicked() {
                                actions.push(SidebarAction::ReloadFromDisk(path.clone()));
                                ui.close_menu();
                            }
                        });
                    }

                    i += 1;
                }

                if matches!(self.edit_state, EditState::NewFile { .. }) {
                    let result = self.show_edit_field(ui);
                    match result {
                        EditFieldResult::Pending => {}
                        EditFieldResult::Cancel => cancel_edit = true,
                        EditFieldResult::Confirm(raw_name) => {
                            if let (Some(new_name), Some(dir)) =
                                (Self::sanitize_filename(&raw_name), self.directory.clone())
                            {
                                let new_path = dir.join(&new_name);
                                if new_path.exists() {
                                    self.set_edit_error("A file with that name already exists.");
                                } else {
                                    match std::fs::write(&new_path, "") {
                                        Ok(()) => {
                                            actions.push(SidebarAction::FileCreated(new_path));
                                            cancel_edit = true;
                                            self.reload_files();
                                        }
                                        Err(e) => {
                                            self.set_edit_error(&format!(
                                                "Failed to create file: {e}"
                                            ));
                                        }
                                    }
                                }
                            } else {
                                self.set_edit_error("Invalid file name.");
                            }
                        }
                    }
                }

                if matches!(self.edit_state, EditState::None) {
                    let remaining = ui.available_size();
                    if remaining.y > 0.0 {
                        let r = ui.allocate_response(remaining, egui::Sense::click());
                        if r.double_clicked() {
                            self.start_new_file();
                        }
                    }
                }

                if cancel_edit {
                    self.edit_state = EditState::None;
                }
            });
        #[cfg(test)]
        {
            // `id.with(1)` is the vertical scroll bar's interaction id.
            self.last_scroll_bar_rect = ui
                .ctx()
                .read_response(scroll_out.id.with(1))
                .map(|r| r.rect);
        }
        let _ = scroll_out;

        actions
    }

    fn set_edit_error(&mut self, msg: &str) {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Uniform")
            .set_description(msg)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        match &mut self.edit_state {
            EditState::Renaming {
                error, focus_set, ..
            }
            | EditState::NewFile {
                error, focus_set, ..
            } => {
                *error = true;
                *focus_set = false;
            }
            EditState::None => {}
        }
    }

    fn sanitize_filename(name: &str) -> Option<String> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        if name.contains('/') || name.contains('\\') || name.contains('\0') {
            return None;
        }
        if name.starts_with('.') {
            return None;
        }
        let name = if !name.ends_with(".unf") {
            format!("{name}.unf")
        } else {
            name.to_string()
        };
        Some(name)
    }

    fn show_edit_field(&mut self, ui: &mut egui::Ui) -> EditFieldResult {
        let (text, error, is_new, focus_set) = match &mut self.edit_state {
            EditState::Renaming {
                text,
                error,
                focus_set,
                ..
            } => (text, error, false, focus_set),
            EditState::NewFile {
                text,
                error,
                focus_set,
            } => (text, error, true, focus_set),
            EditState::None => return EditFieldResult::Cancel,
        };

        let id = egui::Id::new(if is_new {
            "sidebar_new_file"
        } else {
            "sidebar_rename"
        });

        let needs_focus = !*focus_set;

        let stroke = if *error {
            egui::Stroke::new(1.0, egui::Color32::RED)
        } else {
            egui::Stroke::new(1.0, ui.visuals().widgets.active.bg_stroke.color)
        };
        let frame = egui::Frame::NONE.inner_margin(2.0).stroke(stroke);

        let mut result = EditFieldResult::Pending;

        frame.show(ui, |ui| {
            let response = ui.add(
                egui::TextEdit::singleline(text)
                    .id(id)
                    .frame(false)
                    .desired_width(ui.available_width()),
            );

            if needs_focus {
                response.request_focus();
                *focus_set = true;
            } else if response.lost_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    result = EditFieldResult::Cancel;
                } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let trimmed = text.trim().to_string();
                    if trimmed.is_empty() {
                        result = EditFieldResult::Cancel;
                    } else {
                        result = EditFieldResult::Confirm(trimmed);
                    }
                } else {
                    result = EditFieldResult::Cancel;
                }
            }
        });

        result
    }
}

#[derive(PartialEq)]
enum EditFieldResult {
    Pending,
    Cancel,
    Confirm(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sidebar_with(names: &[&str]) -> Sidebar {
        let mut sb = Sidebar::new();
        sb.files = names.iter().map(PathBuf::from).collect();
        sb
    }

    fn input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            ..Default::default()
        }
    }

    /// Drives the panel the way `app::panels` does, and reports the width the
    /// panel stored for the next frame together with the width it wanted.
    fn run_frames(sb: &mut Sidebar, frames: usize) -> (f32, f32) {
        let ctx = egui::Context::default();
        let mut desired = 0.0;
        for _ in 0..frames {
            ctx.run(input(), |ctx| {
                sb.fit_panel_width(ctx, "sidebar");
                desired = sb.desired_width(ctx);
                egui::SidePanel::left("sidebar")
                    .default_width(200.0)
                    .show(ctx, |ui| {
                        sb.show(ui, None, SidebarFiles::default(), false);
                    });
            });
        }
        let stored = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("sidebar"))
            .expect("panel state")
            .rect
            .width();
        (stored, desired)
    }

    #[test]
    fn files_sort_by_name_without_the_extension() {
        let mut files: Vec<PathBuf> = ["num.unf", "num-roman.unf", "numx.unf", "latin.unf"]
            .iter()
            .map(PathBuf::from)
            .collect();
        Sidebar::sort_files(&mut files);
        let names: Vec<_> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        // With the extension in the key, `num-roman` would sort before `num`.
        assert_eq!(names, ["latin.unf", "num.unf", "num-roman.unf", "numx.unf"]);
    }

    #[test]
    fn panel_width_follows_the_longest_file_name() {
        let mut narrow = sidebar_with(&["a.unf", "b.unf"]);
        let mut wide = sidebar_with(&["a.unf", "a-considerably-longer-name.unf"]);
        let (narrow_w, narrow_want) = run_frames(&mut narrow, 3);
        let (wide_w, wide_want) = run_frames(&mut wide, 3);

        // Every frame re-applies the fit, so the width survives the frame in
        // which the panel stores its content rect.
        assert!(
            (narrow_w - narrow_want).abs() < 1.0,
            "{narrow_w} vs {narrow_want}"
        );
        assert!((wide_w - wide_want).abs() < 1.0, "{wide_w} vs {wide_want}");
        assert!(wide_w > narrow_w + 50.0, "{wide_w} vs {narrow_w}");
    }

    /// A panel wider than its file names must still keep the scroll bar at its
    /// right edge: a scroll area shrinks to its content, which would park the
    /// bar next to the longest name with empty panel to its right.
    #[test]
    fn the_scroll_bar_stays_at_the_panel_edge_when_the_panel_is_wider() {
        let names: Vec<String> = (0..80).map(|i| format!("f{i:02}.unf")).collect();
        let mut sb = sidebar_with(&names.iter().map(String::as_str).collect::<Vec<_>>());
        // A width the fit would never ask for: the names are all short.
        sb.auto_width = false;
        let ctx = egui::Context::default();
        let mut panel_right = 0.0;
        for _ in 0..3 {
            ctx.run(input(), |ctx| {
                let resp = egui::SidePanel::left("sidebar")
                    .default_width(360.0)
                    .show(ctx, |ui| {
                        sb.show(ui, None, SidebarFiles::default(), false);
                    });
                panel_right = resp.response.rect.right();
            });
        }

        let bar = sb.last_scroll_bar_rect.expect("scroll bar shown");
        // Slack for the panel frame's inner margin, which the bar sits inside.
        assert!(
            (bar.right() - panel_right).abs() < 12.0,
            "scroll bar at {} but the panel ends at {panel_right}",
            bar.right()
        );
    }

    /// Dragging the panel edge takes the width over from the fit and keeps it:
    /// the drag both turns `auto_width` off and has to survive the frames after
    /// the pointer is released.
    #[test]
    fn dragging_the_edge_takes_the_width_over_for_good() {
        let mut sb = sidebar_with(&["a.unf", "b.unf"]);
        let ctx = egui::Context::default();

        let mut frame = |events: Vec<egui::Event>, pointer: Option<egui::Pos2>| {
            let mut raw = input();
            raw.events = events;
            if let Some(p) = pointer {
                raw.events.insert(0, egui::Event::PointerMoved(p));
            }
            ctx.run(raw, |ctx| {
                sb.fit_panel_width(ctx, "sidebar");
                egui::SidePanel::left("sidebar").show(ctx, |ui| {
                    sb.show(ui, None, SidebarFiles::default(), false);
                });
            });
            egui::containers::panel::PanelState::load(&ctx, egui::Id::new("sidebar"))
                .expect("panel state")
                .rect
                .width()
        };

        let fitted = frame(vec![], None);
        let edge = |x: f32| egui::Pos2::new(x, 400.0);
        let press = |x: f32, pressed: bool| egui::Event::PointerButton {
            pos: edge(x),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };

        frame(vec![press(fitted, true)], Some(edge(fitted)));
        frame(vec![], Some(edge(300.0)));
        let dragged = frame(vec![], Some(edge(320.0)));
        assert!(
            (dragged - 320.0).abs() < 2.0,
            "drag did not move the edge: {dragged}"
        );

        frame(vec![press(320.0, false)], Some(edge(320.0)));
        // Two short names are ~60 px of content: neither the fit nor the panel
        // collapsing onto its content may pull the edge back.
        let after = frame(vec![], None);
        let after = frame(vec![], None).min(after);
        assert!((after - 320.0).abs() < 2.0, "width was taken back: {after}");
    }
}
