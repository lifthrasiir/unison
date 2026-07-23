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
    FileRenamed { old: PathBuf, new: PathBuf },
    FileCreated(PathBuf),
}

pub struct Sidebar {
    files: Vec<PathBuf>,
    directory: Option<PathBuf>,
    edit_state: EditState,
}

impl Sidebar {
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            directory: None,
            edit_state: EditState::None,
        }
    }

    #[expect(unused)]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    pub fn set_directory(&mut self, dir: &Path) {
        self.directory = Some(dir.to_path_buf());
        self.reload_files();
    }

    fn reload_files(&mut self) {
        self.files.clear();
        if let Some(dir) = &self.directory
            && let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|e| e == "unf") {
                        self.files.push(path);
                    }
                }
            }
        self.files.sort();
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
        dirty_paths: &[&Path],
        editor_focused: bool,
    ) -> Vec<SidebarAction> {
        let mut actions = Vec::new();

        let show_empty_label =
            self.files.is_empty() && !matches!(self.edit_state, EditState::NewFile { .. });

        if matches!(self.edit_state, EditState::None) && !editor_focused {
            let f2 = ui.input(|i| i.key_pressed(egui::Key::F2));
            if f2 && ui.rect_contains_pointer(ui.max_rect())
                && let Some(active) = active_path {
                    self.start_rename(active);
                }
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            if show_empty_label {
                ui.label("No .unf files found");
            }

            let mut cancel_edit = false;
            let mut i = 0;
            while i < self.files.len() {
                let path = self.files[i].clone();

                if let EditState::Renaming { index, .. } = &self.edit_state
                    && *index == i {
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
                                            self.set_edit_error(&format!("Failed to rename file: {e}"));
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
                let is_dirty = dirty_paths.contains(&path.as_path());

                let label = if is_dirty {
                    format!("* {name}")
                } else {
                    name
                };

                if ui.selectable_label(is_active, &label).clicked() {
                    actions.push(SidebarAction::OpenFile(path));
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
                                        self.set_edit_error(&format!("Failed to create file: {e}"));
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
            EditState::Renaming { error, focus_set, .. }
            | EditState::NewFile { error, focus_set, .. } => {
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
        let frame = egui::Frame::NONE
            .inner_margin(2.0)
            .stroke(stroke);

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
