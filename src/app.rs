use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc;

use crate::document::{DocLine, Document, NamePartsMap};
use crate::document_io;
use crate::editor::EditorState;
use crate::editor::doc_links::LinkTargetKind;
use crate::editor::document_view::debounced_scroll_step;
use crate::editor::ref_composite::ResolvedGlyph;
use crate::issues::{Issue, collect_issues};
use crate::preview::widget::ShapedPreviewState;
use crate::render::SharedContourCache;
use crate::specimen::SpecimenState;
use crate::sidebar::{Sidebar, SidebarAction};

type FontPair = (Vec<u8>, Vec<u8>);
type FontBuildMessage = (u64, Option<FontPair>);
type DerivedDataMessage = (u64, HashMap<String, ResolvedGlyph>, crate::editor::ref_composite::AlternativesIndex, NamePartsMap, Vec<Issue>);

fn collect_effective_docs<'a>(
    open_documents: &'a [OpenDocument],
    font_base_docs: &'a [Document],
) -> Vec<&'a Document> {
    let mut all_docs: Vec<&Document> = open_documents
        .iter()
        .map(|open_doc| &open_doc.document)
        .collect();
    for base_doc in font_base_docs {
        let dominated = open_documents
            .iter()
            .any(|open_doc| open_doc.document.path == base_doc.path);
        if !dominated {
            all_docs.push(base_doc);
        }
    }
    all_docs.sort_by(|a, b| a.path.cmp(&b.path));
    all_docs
}

pub struct UniformApp {
    font_dir: Option<PathBuf>,
    open_documents: Vec<OpenDocument>,
    active_doc_idx: Option<usize>,
    sidebar: Sidebar,
    escape_mode: bool,
    status_message: Option<(String, std::time::Instant)>,
    font_base_docs: Vec<Document>,
    font_data: Option<FontPair>,
    font_applied: Option<bool>,
    font_data_gen: u64,
    last_font_gen: u64,
    font_rebuild_at: Option<std::time::Instant>,
    font_build_rx: mpsc::Receiver<FontBuildMessage>,
    font_build_tx: mpsc::Sender<FontBuildMessage>,
    font_build_gen: u64,
    contour_cache: SharedContourCache,
    named_glyphs: HashMap<String, ResolvedGlyph>,
    alt_index: crate::editor::ref_composite::AlternativesIndex,
    name_parts: NamePartsMap,
    color_aliases: crate::render::ttf_builder::ColorAliasMap,
    named_glyphs_gen: u64,
    derived_data_tx: mpsc::Sender<DerivedDataMessage>,
    derived_data_rx: mpsc::Receiver<DerivedDataMessage>,
    derived_rebuild_at: Option<std::time::Instant>,
    zoom_level: u32,
    last_export_path: Option<PathBuf>,
    close_confirmed: bool,
    hex_input: Option<String>,
    bottom_panel_height: f32,
    bottom_panel_tab: Option<usize>,
    preview_font_size: f32,
    preview_font_size_slider: f32,
    shaped_preview: ShapedPreviewState,
    specimen: SpecimenState,
    issues: Vec<Issue>,
    issues_gen: u64,
    file_parse_errors: Vec<(PathBuf, String)>,
}

pub struct OpenDocument {
    pub document: Document,
    pub lines: Vec<DocLine>,
    pub editor_state: EditorState,
}

impl OpenDocument {
    /// Flush pending line-level edits into the `Document` model, if any.
    fn flush_pending_changes(&mut self) {
        if self.editor_state.has_pending_document_sync() {
            crate::editor::document_view::flush_document_changes(
                &mut self.lines,
                &mut self.document,
                &mut self.editor_state,
            );
        }
    }
}

pub fn uniform_font_id(ctx: &egui::Context, size: f32) -> egui::FontId {
    let bitmap_family = egui::FontFamily::Name("UniformBitmap".into());
    let has_uniform = ctx.fonts(|f| f.families().contains(&bitmap_family));
    if !has_uniform {
        return egui::FontId::new(size, egui::FontFamily::Proportional);
    }
    let family = if size <= 16.0 {
        bitmap_family
    } else {
        egui::FontFamily::Name("UniformVector".into())
    };
    egui::FontId::new(size, family)
}

fn take_current_font_build(
    rx: &mpsc::Receiver<FontBuildMessage>,
    current_gen: u64,
) -> Option<Option<FontPair>> {
    let mut received = None;
    while let Ok((build_gen, pair)) = rx.try_recv() {
        if build_gen == current_gen {
            received = Some(pair);
        }
    }
    received
}

fn take_latest_derived_data(
    rx: &mpsc::Receiver<DerivedDataMessage>,
) -> Option<DerivedDataMessage> {
    let mut received = None;
    while let Ok(msg) = rx.try_recv() {
        received = Some(msg);
    }
    received
}

impl UniformApp {
    pub fn new(cc: &eframe::CreationContext<'_>, font_dir: Option<PathBuf>) -> Self {
        let _ = cc;

        let (font_base_docs, file_parse_errors) = font_dir
            .as_ref()
            .map(|d| crate::render::ttf_builder::load_docs_from_directory_checked(d))
            .unwrap_or_default();

        let contour_cache = crate::render::new_contour_cache();
        let font_data = if font_base_docs.is_empty() {
            None
        } else {
            let refs: Vec<&Document> = font_base_docs.iter().collect();
            crate::render::build_font_pair_cached(&refs, &contour_cache)
        };

        let (font_build_tx, font_build_rx) = mpsc::channel();
        let (derived_data_tx, derived_data_rx) = mpsc::channel();
        let mut app = Self {
            font_dir: font_dir.clone(),
            open_documents: Vec::new(),
            active_doc_idx: None,
            sidebar: Sidebar::new(),
            escape_mode: false,
            status_message: None,
            font_base_docs,
            font_data,
            font_applied: None,
            font_data_gen: 0,
            last_font_gen: 0,
            font_rebuild_at: None,
            font_build_rx,
            font_build_tx,
            font_build_gen: 0,
            contour_cache,
            named_glyphs: HashMap::new(),
            alt_index: Default::default(),
            name_parts: NamePartsMap::new(),
            color_aliases: Default::default(),
            named_glyphs_gen: u64::MAX,
            derived_data_tx,
            derived_data_rx,
            derived_rebuild_at: None,
            zoom_level: 1,
            last_export_path: None,
            close_confirmed: false,
            hex_input: None,
            bottom_panel_height: 200.0,
            bottom_panel_tab: Some(0),
            preview_font_size: 32.0,
            preview_font_size_slider: 32.0,
            shaped_preview: ShapedPreviewState::new(),
            specimen: SpecimenState::new(),
            issues: Vec::new(),
            issues_gen: u64::MAX,
            file_parse_errors,
        };

        if let Some(dir) = &font_dir {
            app.sidebar.set_directory(dir);
        }

        app
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), std::time::Instant::now()));
    }

    fn flush_open_document(&mut self, idx: usize) {
        let Some(doc) = self.open_documents.get_mut(idx) else {
            return;
        };
        doc.flush_pending_changes();
    }

    fn flush_all_open_documents(&mut self) {
        for doc in &mut self.open_documents {
            doc.flush_pending_changes();
        }
    }

    fn open_file(&mut self, path: PathBuf) {
        if let Some(idx) = self.active_doc_idx {
            self.flush_open_document(idx);
        }
        if let Some(idx) = self
            .open_documents
            .iter()
            .position(|d| d.document.path == path)
        {
            self.active_doc_idx = Some(idx);
            return;
        }

        match document_io::parse_document(&path) {
            Ok(doc) => {
                let mut buf = Vec::new();
                document_io::serialize_document(&doc, &mut buf).ok();
                let canonical = String::from_utf8(buf).unwrap_or_default();
                let mut lines = document_io::parse_doclines(&canonical);
                if lines.is_empty() {
                    lines.push(crate::document::DocLine::Text(String::new()));
                }

                let mut doc = doc;
                if let Ok((fresh_doc, _)) =
                    document_io::derive_document(&lines, path.clone())
                {
                    doc = fresh_doc;
                }
                // Replacing the directory snapshot with an opened file is a
                // new source revision even when the path is unchanged. The
                // file may have changed on disk since the folder was loaded.
                doc.edit_gen = self
                    .font_base_docs
                    .iter()
                    .find(|base| base.path == path)
                    .map_or(1, |base| base.edit_gen.wrapping_add(1));

                let open_doc = OpenDocument {
                    document: doc,
                    lines,
                    editor_state: EditorState::new(),
                };
                self.open_documents.push(open_doc);
                self.active_doc_idx = Some(self.open_documents.len() - 1);
                self.set_status(format!("Opened {}", path.display()));
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"));
            }
        }
    }

    fn current_font_gen(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for doc in self.collect_all_docs() {
            doc.path.hash(&mut hasher);
            doc.edit_gen.hash(&mut hasher);
        }
        hasher.finish()
    }

    fn collect_all_docs(&self) -> Vec<&Document> {
        collect_effective_docs(&self.open_documents, &self.font_base_docs)
    }

    fn rebuild_font(&self, ctx: &egui::Context) {
        let build_gen = self.font_build_gen;
        let owned_docs: Vec<Document> = self.collect_all_docs().into_iter().cloned().collect();
        let tx = self.font_build_tx.clone();
        let ctx = ctx.clone();
        let cache = self.contour_cache.clone();
        std::thread::spawn(move || {
            let refs: Vec<&Document> = owned_docs.iter().collect();
            let pair = crate::render::build_font_pair_cached(&refs, &cache);
            let _ = tx.send((build_gen, pair));
            ctx.request_repaint();
        });
    }

    fn goto_glyph(&mut self, _ctx: &egui::Context, name: &str, kind: &LinkTargetKind) {
        use crate::document::{DocumentItem, GlyphName};
        use crate::editor::doc_links::find_link_target_in_doc;

        let target_path = {
            let all_docs = self.collect_all_docs();
            all_docs.iter().find_map(|doc| {
                let has_match = match kind {
                    LinkTargetKind::Glyph => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::Glyph { name: GlyphName(n), .. } if n == name)
                    }),
                    LinkTargetKind::NameParts => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::NameParts { name: n, .. } if n == name)
                    }),
                    LinkTargetKind::Remap => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::Remap { feature: f, .. } if f == name)
                    }),
                    LinkTargetKind::Color => doc.items.iter().any(|item| {
                        matches!(item, DocumentItem::Color { name: n, .. } if n == name)
                    }),
                };
                has_match.then(|| doc.path.clone())
            })
        };

        let Some(path) = target_path else { return };

        self.open_file(path.clone());

        let idx = match self.open_documents.iter().position(|d| d.document.path == path) {
            Some(i) => i,
            None => return,
        };
        self.active_doc_idx = Some(idx);

        let doc = &mut self.open_documents[idx];
        if let Some(line_idx) = find_link_target_in_doc(&doc.lines, name, kind) {
            doc.editor_state.goto_line(line_idx);
        }
    }

    fn execute_rename(&mut self, action: &crate::editor::document_view::RenameAction) {
        use crate::editor::doc_links::RenameKind;

        let files: Vec<PathBuf> = self.sidebar.files().to_vec();
        let saved_active = self.active_doc_idx;
        let mut changed_count = 0usize;

        // First pass: check which unopened files would be affected and open them
        let mut to_open = Vec::new();
        for file_path in &files {
            let already_open = self.open_documents.iter().any(|d| &d.document.path == file_path);
            if !already_open
                && let Ok(content) = std::fs::read_to_string(file_path) {
                    let text_lines: Vec<DocLine> = content.lines().map(|l| DocLine::Text(l.to_string())).collect();
                    let new_lines = rename_in_lines(&text_lines, &action.old_name, &action.new_name, &action.kind);
                    if new_lines != text_lines {
                        to_open.push(file_path.clone());
                    }
                }
        }
        for path in &to_open {
            self.open_file(path.clone());
        }

        // Second pass: apply rename to all open documents
        for doc in &mut self.open_documents {
            let old_lines: Vec<DocLine> = doc.lines.clone();
            let new_lines = rename_in_lines(&doc.lines, &action.old_name, &action.new_name, &action.kind);
            if new_lines != old_lines {
                doc.editor_state.undo.break_coalesce();
                doc.editor_state.undo.push_lines(
                    0,
                    old_lines,
                    new_lines.clone(),
                    doc.editor_state.cursor,
                    doc.editor_state.cursor,
                );
                doc.lines = new_lines;
                match crate::document_io::derive_document(&doc.lines, doc.document.path.clone()) {
                    Ok((new_doc, _)) => {
                        let next_gen = doc.document.edit_gen + 1;
                        doc.document = new_doc;
                        doc.document.dirty = true;
                        doc.document.edit_gen = next_gen;
                    }
                    Err(_) => {
                        doc.document.dirty = true;
                        doc.document.edit_gen += 1;
                    }
                }
                changed_count += 1;
            }
        }

        // Restore active tab
        self.active_doc_idx = saved_active;

        if changed_count > 0 {
            self.rebuild_named_glyphs_sync();
            let kind_str = match action.kind {
                RenameKind::Glyph => "glyph",
                RenameKind::NameParts => "name-parts",
                RenameKind::Point => "point",
                RenameKind::Color => "color",
            };
            self.set_status(format!(
                "Renamed {} '{}' → '{}' ({} file{})",
                kind_str,
                action.old_name,
                action.new_name,
                changed_count,
                if changed_count == 1 { "" } else { "s" },
            ));
        }
    }

    fn rebuild_named_glyphs_sync(&mut self) {
        let all_docs = self.collect_all_docs();
        let name_parts = crate::document::collect_name_parts(&all_docs);
        let (named_glyphs, alt_index) =
            crate::editor::ref_composite::resolve_named_glyphs_with_parts(
                &all_docs,
                &name_parts,
            );
        self.named_glyphs = named_glyphs;
        self.alt_index = alt_index;
        self.name_parts = name_parts;
    }

    fn rebuild_derived_data(&self, ctx: &egui::Context) {
        let build_gen = self.font_build_gen;
        let owned_docs: Vec<Document> = self.collect_all_docs().into_iter().cloned().collect();
        let file_parse_errors = self.file_parse_errors.clone();
        let tx = self.derived_data_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let refs: Vec<&Document> = owned_docs.iter().collect();
            let name_parts = crate::document::collect_name_parts(&refs);
            let (named_glyphs, alt_index) =
                crate::editor::ref_composite::resolve_named_glyphs_with_parts(
                    &refs,
                    &name_parts,
                );
            let mut issues = collect_issues(&refs);
            for (path, msg) in &file_parse_errors {
                issues.insert(0, Issue {
                    severity: crate::issues::Severity::Error,
                    message: msg.clone(),
                    file: path.clone(),
                    line: 0,
                    file_line: 1,
                });
            }
            let _ = tx.send((build_gen, named_glyphs, alt_index, name_parts, issues));
            ctx.request_repaint();
        });
    }

    fn apply_font(&mut self, ctx: &egui::Context) {
        let want_custom = !self.escape_mode && self.font_data.is_some();
        if self.font_applied == Some(want_custom) {
            return;
        }

        let mut fonts = egui::FontDefinitions::default();
        let system_family = egui::FontFamily::Name("System".into());
        let bitmap_family = egui::FontFamily::Name("UniformBitmap".into());
        let vector_family = egui::FontFamily::Name("UniformVector".into());

        let system_fonts = fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();

        let (mut bitmap_list, mut vector_list) = if let Some((bitmap_ttf, vector_ttf)) =
            &self.font_data
        {
            fonts.font_data.insert(
                "uniform_bitmap".into(),
                egui::FontData::from_owned(bitmap_ttf.clone()).into(),
            );
            fonts.font_data.insert(
                "uniform_vector".into(),
                egui::FontData::from_owned(vector_ttf.clone()).into(),
            );
            (
                vec!["uniform_bitmap".to_string()],
                vec!["uniform_vector".to_string()],
            )
        } else {
            (Vec::new(), Vec::new())
        };
        bitmap_list.extend(system_fonts.clone());
        vector_list.extend(system_fonts.clone());

        fonts
            .families
            .insert(bitmap_family, bitmap_list.clone());
        fonts
            .families
            .insert(vector_family, vector_list);

        if want_custom {
            fonts
                .families
                .insert(egui::FontFamily::Proportional, bitmap_list.clone());
            fonts
                .families
                .insert(egui::FontFamily::Monospace, bitmap_list);
        }

        fonts.families.insert(system_family, system_fonts);
        ctx.set_fonts(fonts);

        let mut style = (*ctx.style()).clone();
        for (_, font_id) in style.text_styles.iter_mut() {
            font_id.size = 16.0;
        }
        ctx.set_style(style);

        self.font_applied = Some(want_custom);
    }

    fn has_unsaved_changes(&self) -> bool {
        self.open_documents.iter().any(|d| {
            d.document.dirty || d.editor_state.has_pending_document_sync()
        })
    }

    fn save_all(&mut self) -> bool {
        for doc in &mut self.open_documents {
            doc.flush_pending_changes();
            if !doc.document.dirty {
                continue;
            }
            let mut buf = Vec::new();
            if let Err(e) = document_io::serialize_doclines(&doc.lines, &mut buf)
                .and_then(|()| {
                    document_io::write_and_sync(&doc.document.path, &buf)
                })
            {
                self.status_message =
                    Some((format!("Save error: {e}"), std::time::Instant::now()));
                return false;
            }
            doc.document.dirty = false;
            doc.editor_state.undo.mark_saved();
        }
        true
    }

    fn confirm_close_and_maybe_save(&mut self) -> bool {
        if !self.has_unsaved_changes() {
            return true;
        }

        let save = "Save";
        let dont_save = "Don't Save";
        let cancel = "Cancel";

        let result = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Do you want to save changes before closing?")
            .set_description("Your unsaved changes will be lost if you close without saving.")
            .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                save.into(),
                dont_save.into(),
                cancel.into(),
            ))
            .show();

        match &result {
            rfd::MessageDialogResult::Yes => {
                self.save_all()
            }
            rfd::MessageDialogResult::Custom(s) if s == save => {
                self.save_all()
            }
            rfd::MessageDialogResult::No => true,
            rfd::MessageDialogResult::Custom(s) if s == dont_save => true,
            _ => false,
        }
    }

    fn export_to_path(&mut self, path: PathBuf) {
        self.flush_all_open_documents();
        let all_docs = self.collect_all_docs();
        let Some(font_bytes) = crate::render::build_font_from_documents(&all_docs) else {
            self.set_status("Export failed: could not build font".to_string());
            return;
        };
        let is_woff2 = path.extension().and_then(|e| e.to_str()) == Some("woff2");
        let output_bytes = if is_woff2 {
            match crate::render::ttf_to_woff2(&font_bytes) {
                Ok(b) => b,
                Err(e) => {
                    self.set_status(format!("Export error: {e}"));
                    return;
                }
            }
        } else {
            font_bytes
        };
        match std::fs::write(&path, &output_bytes) {
            Ok(()) => {
                self.last_export_path = Some(path.clone());
                self.set_status(format!(
                    "Exported {} ({} bytes)",
                    path.display(),
                    output_bytes.len(),
                ));
            }
            Err(e) => {
                self.set_status(format!("Export error: {e}"));
            }
        }
    }

    fn export_with_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Export Font")
            .add_filter("TrueType Font", &["ttf"])
            .add_filter("WOFF2 Font", &["woff2"]);
        if let Some(ref last) = self.last_export_path {
            if let Some(dir) = last.parent() {
                dialog = dialog.set_directory(dir);
            }
            if let Some(name) = last.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy().to_string());
            }
        } else if let Some(ref dir) = self.font_dir {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.save_file() {
            self.export_to_path(path);
        }
    }

    fn save_active(&mut self) {
        if let Some(idx) = self.active_doc_idx
            && let Some(doc) = self.open_documents.get_mut(idx) {
                doc.flush_pending_changes();
                let mut buf = Vec::new();
                let result = document_io::serialize_doclines(&doc.lines, &mut buf)
                    .and_then(|()| {
                        document_io::write_and_sync(&doc.document.path, &buf)
                    });
                let path_display = doc.document.path.display().to_string();
                match result {
                    Ok(()) => {
                        doc.document.dirty = false;
                        doc.editor_state.undo.mark_saved();
                        self.status_message =
                            Some((format!("Saved {path_display}"), std::time::Instant::now()));
                    }
                    Err(e) => {
                        self.status_message =
                            Some((format!("Save error: {e}"), std::time::Instant::now()));
                    }
                }
            }
    }
}

impl eframe::App for UniformApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        {
            let title = if let Some(idx) = self.active_doc_idx {
                let doc = &self.open_documents[idx];
                let path = doc.document.path.display().to_string();
                if doc.document.dirty {
                    format!("{path}* - Uniform")
                } else {
                    format!("{path} - Uniform")
                }
            } else if let Some(dir) = &self.font_dir {
                format!("{} - Uniform", dir.display())
            } else {
                "Uniform".to_string()
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }

        let mut escape_toggled = false;
        ctx.input(|i| {
            if i.key_pressed(egui::Key::F12) {
                self.escape_mode = !self.escape_mode;
                escape_toggled = true;
            }
        });

        // Alt + hex digit codepoint input
        {
            let mut hex_char_to_inject: Option<char> = None;
            let hex_input = &mut self.hex_input;
            ctx.input_mut(|input| {
                let alt_held = input.modifiers.alt;
                input.events.retain(|event| {
                    match event {
                        egui::Event::Key {
                            key, pressed: true, modifiers, ..
                        } if modifiers.alt && !modifiers.command && !modifiers.ctrl => {
                            if let Some(hex) = key_to_hex_char(*key) {
                                let buf = hex_input.get_or_insert_with(String::new);
                                if buf.len() < 6 {
                                    buf.push(hex);
                                }
                                return false;
                            }
                            if hex_input.is_some() {
                                *hex_input = None;
                                return false;
                            }
                            true
                        }
                        egui::Event::Key {
                            key: _, pressed: false, modifiers, ..
                        } if !alt_held && hex_input.is_some() => {
                            let _ = modifiers;
                            if let Some(hex_str) = hex_input.take()
                                && let Some(ch) = validate_hex_codepoint(&hex_str) {
                                    hex_char_to_inject = Some(ch);
                                }
                            true
                        }
                        egui::Event::Text(_) if hex_input.is_some() => false,
                        _ => true,
                    }
                });
                if !alt_held && hex_input.is_some()
                    && let Some(hex_str) = hex_input.take()
                        && let Some(ch) = validate_hex_codepoint(&hex_str) {
                            hex_char_to_inject = Some(ch);
                        }
                if let Some(ch) = hex_char_to_inject {
                    input.events.push(egui::Event::Text(ch.to_string()));
                }
            });
        }

        // Cmd/Ctrl + scroll wheel to adjust zoom level
        // (skip when hovering on the editing grid — ctrl+scroll cycles layers there)
        {
            let cmd_held = ctx.input(|i| i.modifiers.command);
            let grid_hover = self.active_doc_idx
                .and_then(|i| self.open_documents.get(i))
                .is_some_and(|d| d.editor_state.is_grid_hover());
            if cmd_held && !grid_hover
                && let Some(step) = debounced_scroll_step(ctx) {
                    let old_zoom = self.zoom_level;
                    if step < 0 {
                        self.zoom_level = (self.zoom_level + 1).min(8);
                    } else {
                        self.zoom_level = (self.zoom_level - 1).max(1);
                    }
                    if self.zoom_level != old_zoom
                        && let Some(idx) = self.active_doc_idx
                            && let Some(doc) = self.open_documents.get_mut(idx) {
                                doc.editor_state.notify_zoom_change(old_zoom);
                            }
                    ctx.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
                }
        }

        let font_gen = self.current_font_gen();
        if font_gen != self.last_font_gen {
            self.last_font_gen = font_gen;
            self.font_build_gen = self.font_build_gen.wrapping_add(1);
            self.font_rebuild_at =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(300));
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
        if let Some(at) = self.font_rebuild_at
            && std::time::Instant::now() >= at {
                self.rebuild_font(ctx);
                self.font_rebuild_at = None;
            }

        {
            if let Some(pair) =
                take_current_font_build(&self.font_build_rx, self.font_build_gen)
            {
                self.font_data = pair;
                self.font_data_gen = self.font_build_gen;
                self.font_applied = None;
                self.shaped_preview.invalidate_font(self.font_data_gen);
            }
        }

        if self.font_build_gen != self.named_glyphs_gen
            && self.derived_rebuild_at.is_none()
        {
            self.derived_rebuild_at =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(300));
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
        if let Some(at) = self.derived_rebuild_at
            && std::time::Instant::now() >= at
        {
            self.rebuild_derived_data(ctx);
            self.derived_rebuild_at = None;
        }

        if let Some((data_gen, named_glyphs, alt_index, name_parts, issues)) =
            take_latest_derived_data(&self.derived_data_rx)
        {
            self.named_glyphs = named_glyphs;
            self.alt_index = alt_index;
            self.name_parts = name_parts;
            self.named_glyphs_gen = data_gen;
            self.issues = issues;
            self.issues_gen = data_gen;
            let all_docs = self.collect_all_docs();
            let doc_refs: Vec<&Document> = all_docs.to_vec();
            self.color_aliases = crate::render::ttf_builder::collect_color_aliases(&doc_refs);
        }

        let theme_before = ctx.options(|o| o.theme_preference);
        let (mod_name, shift_name, exit_shortcut) = if cfg!(target_os = "macos") {
            ("⌘", "⇧", "⌘Q")
        } else {
            ("Ctrl+", "Shift+", "Alt+F4")
        };

        let mut menu_new_file = false;
        let mut menu_open_folder = false;
        let mut menu_rename = false;
        let mut menu_rename_symbol = false;
        let mut menu_export = false;
        let mut menu_export_new = false;
        let mut menu_exit = false;
        let mut ctrl_s_pressed = false;
        let mut ctrl_shift_s_pressed = false;

        use crate::edit_menu::{EditAction, EditMenuCaps};

        #[derive(Clone, Copy, PartialEq)]
        enum EditTarget { Editor, Preview }

        let mut edit_action = EditAction::None;
        let edit_target = if self.shaped_preview.is_focused() {
            EditTarget::Preview
        } else {
            EditTarget::Editor
        };

        let editor_focused = self.active_doc_idx
            .and_then(|i| self.open_documents.get(i))
            .is_some_and(|d| d.editor_state.is_active());

        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.add(egui::Button::new("New file...").shortcut_text(format!("{mod_name}N"))).clicked() {
                        menu_new_file = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Open folder...").shortcut_text(format!("{mod_name}{shift_name}O"))).clicked() {
                        menu_open_folder = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    let has_active = self.active_doc_idx.is_some();
                    if ui
                        .add_enabled(has_active, egui::Button::new("Save").shortcut_text(format!("{mod_name}S")))
                        .clicked()
                    {
                        ctrl_s_pressed = true;
                        ui.close_menu();
                    }
                    if ui
                        .add(egui::Button::new("Save all").shortcut_text(format!("{mod_name}{shift_name}S")))
                        .clicked()
                    {
                        ctrl_shift_s_pressed = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(has_active && !editor_focused, egui::Button::new("Rename file...").shortcut_text("F2"))
                        .clicked()
                    {
                        menu_rename = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    let export_label = if let Some(ref p) = self.last_export_path {
                        format!(
                            "Export to {}",
                            p.file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "last font".into()),
                        )
                    } else {
                        "Export to last font".into()
                    };
                    if ui
                        .add_enabled(
                            self.last_export_path.is_some(),
                            egui::Button::new(export_label)
                                .shortcut_text(format!("{mod_name}E")),
                        )
                        .clicked()
                    {
                        menu_export = true;
                        ui.close_menu();
                    }
                    if ui
                        .add(
                            egui::Button::new("Export to new font...")
                                .shortcut_text(format!("{mod_name}{shift_name}E")),
                        )
                        .clicked()
                    {
                        menu_export_new = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Exit").shortcut_text(exit_shortcut)).clicked() {
                        menu_exit = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Edit", |ui| {
                    let caps = match edit_target {
                        EditTarget::Preview => self.shaped_preview.edit_menu_caps(),
                        EditTarget::Editor => {
                            self.active_doc_idx
                                .and_then(|i| self.open_documents.get(i))
                                .map(|d| d.editor_state.edit_menu_caps())
                                .unwrap_or(EditMenuCaps {
                                    can_undo: false,
                                    can_redo: false,
                                    has_selection: false,
                                    can_edit: false,
                                })
                        }
                    };
                    edit_action = crate::edit_menu::show_edit_menu_items(ui, &caps, true);
                    ui.separator();
                    if ui
                        .add_enabled(editor_focused, egui::Button::new("Rename symbol...").shortcut_text("F2"))
                        .clicked()
                    {
                        menu_rename_symbol = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("View", |ui| {
                    let (font_label, preview_family) = if self.escape_mode {
                        ("Use dogfooded font", egui::FontFamily::Name("UniformBitmap".into()))
                    } else {
                        ("Use system font", egui::FontFamily::Name("System".into()))
                    };
                    let label = egui::RichText::new(font_label).family(preview_family);
                    if ui.add(egui::Button::new(label).shortcut_text("F12")).clicked() {
                        self.escape_mode = !self.escape_mode;
                        escape_toggled = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.menu_button("Color Scheme", |ui| {
                        if ui
                            .radio(theme_before == egui::ThemePreference::System, "System")
                            .clicked()
                        {
                            ctx.set_theme(egui::ThemePreference::System);
                            ui.close_menu();
                        }
                        if ui
                            .radio(theme_before == egui::ThemePreference::Dark, "Dark")
                            .clicked()
                        {
                            ctx.set_theme(egui::ThemePreference::Dark);
                            ui.close_menu();
                        }
                        if ui
                            .radio(theme_before == egui::ThemePreference::Light, "Light")
                            .clicked()
                        {
                            ctx.set_theme(egui::ThemePreference::Light);
                            ui.close_menu();
                        }
                    });
                });
            });
        });
        if ctx.options(|o| o.theme_preference) != theme_before {
            self.font_applied = None;
        }

        ctx.input(|i| {
            if i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::N) {
                menu_new_file = true;
            }
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::O) {
                menu_open_folder = true;
            }
            if i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::S) {
                ctrl_s_pressed = true;
            }
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::S) {
                ctrl_shift_s_pressed = true;
            }
            if i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::E) {
                menu_export = true;
            }
            if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::E) {
                menu_export_new = true;
            }
            if cfg!(target_os = "macos") {
                if i.modifiers.command && i.key_pressed(egui::Key::Q) {
                    menu_exit = true;
                }
            } else if i.modifiers.alt && i.key_pressed(egui::Key::F4) {
                menu_exit = true;
            }
        });

        if menu_new_file && self.font_dir.is_some() {
            self.sidebar.start_new_file();
        }

        if menu_open_folder
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
            && self.confirm_close_and_maybe_save() {
                self.font_dir = Some(dir.clone());
                self.open_documents.clear();
                self.active_doc_idx = None;
                self.sidebar.set_directory(&dir);
                let (base_docs, parse_errors) = crate::render::ttf_builder::load_docs_from_directory_checked(&dir);
                self.font_base_docs = base_docs;
                self.file_parse_errors = parse_errors;
                let refs: Vec<&Document> = self.font_base_docs.iter().collect();
                self.contour_cache.lock().unwrap().clear();
                self.font_data = crate::render::build_font_pair_cached(&refs, &self.contour_cache);
                self.font_build_gen = self.font_build_gen.wrapping_add(1);
                self.font_data_gen = self.font_build_gen;
                self.font_applied = None;
                self.shaped_preview.invalidate_font(self.font_data_gen);
                self.font_rebuild_at = None;
                self.last_font_gen = self.current_font_gen();
                self.rebuild_named_glyphs_sync();
                self.named_glyphs_gen = self.font_build_gen;
                {
                    let all_docs = self.collect_all_docs();
                    self.issues = collect_issues(&all_docs);
                }
                self.issues_gen = self.font_build_gen;
                self.set_status(format!("Opened folder {}", dir.display()));
            }

        if menu_rename
            && let Some(idx) = self.active_doc_idx
                && let Some(doc) = self.open_documents.get(idx)  {
                    self.sidebar.start_rename(&doc.document.path);
                }

        if menu_rename_symbol
            && let Some(idx) = self.active_doc_idx
                && let Some(doc) = self.open_documents.get_mut(idx) {
                    doc.editor_state.start_rename_at_cursor(&doc.lines);
                }

        if escape_toggled {
            self.font_applied = None;
        }
        self.apply_font(ctx);

        if ctrl_s_pressed {
            self.save_active();
        }
        if ctrl_shift_s_pressed
            && self.save_all() {
                self.set_status("Saved all files".to_string());
            }

        if menu_export {
            if let Some(path) = self.last_export_path.clone() {
                self.export_to_path(path);
            } else {
                self.export_with_dialog();
            }
        }
        if menu_export_new {
            self.export_with_dialog();
        }

        let mut sidebar_actions = Vec::new();
        let mut goto_glyph_request: Option<crate::editor::document_view::GotoGlyph> = None;
        let mut rename_request: Option<crate::editor::document_view::RenameAction> = None;
        egui::SidePanel::left("sidebar")
            .default_width(200.0)
            .show(ctx, |ui| {
                let dirty_paths: Vec<&std::path::Path> = self.open_documents
                    .iter()
                    .filter(|d| d.document.dirty)
                    .map(|d| d.document.path.as_path())
                    .collect();
                sidebar_actions = self.sidebar.show(
                    ui,
                    self.active_doc_idx
                        .and_then(|i| self.open_documents.get(i))
                        .map(|d| d.document.path.as_path()),
                    &dirty_paths,
                    editor_focused,
                );
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
                    if self.zoom_level > 1 {
                        ui.label(format!("{}x", self.zoom_level));
                    }
                    if let Some(idx) = self.active_doc_idx
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
                });
            });
        });

        let mut specimen_clicked_glyph: Option<String> = None;
        let mut issues_click: Option<(PathBuf, usize)> = None;
        let bottom_panel_expanded = self.bottom_panel_tab.is_some();
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
                    for (idx, label) in [(0, "Preview"), (1, "Specimen")] {
                        let selected = self.bottom_panel_tab == Some(idx);
                        if ui.selectable_label(selected, label).clicked() {
                            self.bottom_panel_tab = if selected { None } else { Some(idx) };
                        }
                    }
                    let issues_label = if self.issues.is_empty() {
                        "Issues".to_string()
                    } else {
                        let errors = self.issues.iter()
                            .filter(|i| i.severity == crate::issues::Severity::Error)
                            .count();
                        let warnings = self.issues.len() - errors;
                        let mut parts = Vec::new();
                        if errors > 0 { parts.push(format!("{errors} error{}", if errors == 1 { "" } else { "s" })); }
                        if warnings > 0 { parts.push(format!("{warnings} warning{}", if warnings == 1 { "" } else { "s" })); }
                        format!("Issues ({})", parts.join(", "))
                    };
                    let issues_selected = self.bottom_panel_tab == Some(2);
                    if ui.selectable_label(issues_selected, issues_label).clicked() {
                        self.bottom_panel_tab = if issues_selected { None } else { Some(2) };
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
                    }
                    Some(1) => {
                        let all_docs: Vec<&Document> = {
                            let mut docs: Vec<&Document> = Vec::new();
                            for od in &self.open_documents {
                                docs.push(&od.document);
                            }
                            for bd in &self.font_base_docs {
                                let dominated = self.open_documents
                                    .iter()
                                    .any(|od| od.document.path == bd.path);
                                if !dominated {
                                    docs.push(bd);
                                }
                            }
                            docs.sort_by(|a, b| a.path.cmp(&b.path));
                            docs
                        };
                        self.specimen.rebuild_if_needed(
                            &all_docs, &self.name_parts, self.font_build_gen,
                        );
                        specimen_clicked_glyph = self.specimen.show(
                            ui,
                            self.font_data.as_ref(),
                        );
                    }
                    Some(2) => {
                        show_issues_tab(ui, &self.issues, &mut issues_click);
                    }
                    _ => {}
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.open_documents.is_empty() {
                ui.centered_and_justified(|ui| {
                    if self.font_dir.is_some() {
                        ui.label("Select a file from the sidebar");
                    } else {
                        ui.label("Usage: uniform <font-directory>");
                    }
                });
            } else if let Some(idx) = self.active_doc_idx
                && let Some(doc) = self.open_documents.get_mut(idx) {
                    let font_size = 16.0 * self.zoom_level as f32;
                    let editor_font_id = if self.escape_mode {
                        egui::FontId::new(font_size, egui::FontFamily::Monospace)
                    } else {
                        uniform_font_id(ui.ctx(), font_size)
                    };
                    let result = crate::editor::document_view::show_document(
                        ui,
                        &mut doc.document,
                        &mut doc.lines,
                        &mut doc.editor_state,
                        &self.named_glyphs,
                        &self.name_parts,
                        &self.alt_index,
                        &self.color_aliases,
                        self.zoom_level,
                        &editor_font_id,
                    );
                    if let Some(goto) = result.goto {
                        goto_glyph_request = Some(goto);
                    }
                    if let Some(rename) = result.rename {
                        rename_request = Some(rename);
                    }
                }
        });

        if let Some(goto) = goto_glyph_request {
            self.goto_glyph(ctx, &goto.name, &goto.kind);
        }

        if let Some(glyph_name) = specimen_clicked_glyph {
            self.goto_glyph(ctx, &glyph_name, &LinkTargetKind::Glyph);
        }

        if let Some((path, line)) = issues_click {
            self.open_file(path.clone());
            if let Some(idx) = self.open_documents.iter().position(|d| d.document.path == path) {
                self.active_doc_idx = Some(idx);
                self.open_documents[idx].editor_state.goto_line(line);
            }
        }

        if let Some(rename) = rename_request {
            self.execute_rename(&rename);
        }

        if edit_action != EditAction::None {
            match edit_target {
                EditTarget::Preview => {
                    self.shaped_preview.apply_edit_action(edit_action, ctx);
                }
                EditTarget::Editor => {
                    if let Some(idx) = self.active_doc_idx
                        && let Some(doc) = self.open_documents.get_mut(idx) {
                            let changed = doc.editor_state.apply_edit_action(
                                edit_action,
                                &mut doc.lines,
                                ctx,
                            );
                            if changed {
                                crate::editor::document_view::flush_document_changes(
                                    &mut doc.lines,
                                    &mut doc.document,
                                    &mut doc.editor_state,
                                );
                            }
                        }
                }
            }
        }

        // Decide whether to close only after this frame's editor input has
        // updated the source buffer and dirty state.
        if menu_exit && self.confirm_close_and_maybe_save() {
            self.close_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if ctx.input(|i| i.viewport().close_requested())
            && !self.close_confirmed
            && !self.confirm_close_and_maybe_save()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }
}

use crate::editor::{key_to_hex_char, validate_hex_codepoint};

fn show_issues_tab(
    ui: &mut egui::Ui,
    issues: &[Issue],
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

fn rename_in_lines(
    lines: &[DocLine],
    old_name: &str,
    new_name: &str,
    kind: &crate::editor::doc_links::RenameKind,
) -> Vec<DocLine> {
    use crate::editor::doc_links::RenameKind;

    lines.iter().map(|line| {
        let DocLine::Text(s) = line else { return line.clone() };
        let trimmed = s.trim_start();

        let new_text = match kind {
            RenameKind::Glyph => rename_glyph_in_line(trimmed, s, old_name, new_name),
            RenameKind::NameParts => rename_name_parts_in_line(trimmed, s, old_name, new_name),
            RenameKind::Point => rename_point_in_line(trimmed, s, old_name, new_name),
            RenameKind::Color => rename_color_in_line(trimmed, s, old_name, new_name),
        };

        match new_text {
            Some(t) => DocLine::Text(t),
            None => line.clone(),
        }
    }).collect()
}

fn rename_glyph_in_line(trimmed: &str, full: &str, old_name: &str, new_name: &str) -> Option<String> {
    let leading = &full[..full.len() - trimmed.len()];

    // glyph NAME ...
    if let Some(rest) = trimmed.strip_prefix("glyph ") {
        let name = rest.split_whitespace().next().unwrap_or("");
        if name.is_empty() {
            return None;
        }
        let after_name = &rest[name.len()..]; // preserves whitespace

        let ws_parts: Vec<&str> = rest.split_whitespace().collect();
        if let Some(eq_token) = ws_parts.iter().position(|&part| part == "=")
            && let Some(&alias) = ws_parts.get(eq_token + 1) {
            // glyph NAME [flags...] = ALIAS [more...]
            if name == old_name {
                return Some(format!("{leading}glyph {new_name}{after_name}"));
            }
            if alias == old_name {
                // Find the alias in the after_name portion and replace it
                let eq_pos = after_name.find('=').unwrap();
                let before_eq = &after_name[..eq_pos + 1]; // " ="
                let after_eq = &after_name[eq_pos + 1..]; // " ALIAS [more]"
                let alias_start = after_eq.find(alias).unwrap();
                let before_alias = &after_eq[..alias_start];
                let after_alias = &after_eq[alias_start + alias.len()..];
                return Some(format!("{leading}glyph {name}{before_eq}{before_alias}{new_name}{after_alias}"));
            }
            return None;
        }

        if name == old_name {
            return Some(format!("{leading}glyph {new_name}{after_name}"));
        }
        return None;
    }

    // ref NAME COL ROW [negated]
    if let Some(rest) = trimmed.strip_prefix("ref ") {
        let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
        if parts[0] == old_name {
            let after = if parts.len() > 1 { format!(" {}", parts[1]) } else { String::new() };
            return Some(format!("{leading}ref {new_name}{after}"));
        }
        return None;
    }

    // map CHAR = NAME
    if let Some(rest) = trimmed.strip_prefix("map ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() == 3 && parts[1] == "=" && parts[2] == old_name {
            return Some(format!("{leading}map {} = {new_name}", parts[0]));
        }
        return None;
    }

    // remap tokens: replace old_name with new_name as whole tokens
    if let Some(rest) = trimmed.strip_prefix("remap ") {
        let mut changed = false;
        let new_tokens: Vec<String> = rest.split_whitespace().map(|tok| {
            let has_colon = tok.ends_with(':');
            let clean = tok.trim_end_matches(':');
            if clean == old_name {
                changed = true;
                if has_colon { format!("{new_name}:") } else { new_name.to_string() }
            } else {
                tok.to_string()
            }
        }).collect();
        if changed {
            return Some(format!("{leading}remap {}", new_tokens.join(" ")));
        }
        return None;
    }

    // exclude-from-sample NAME
    if let Some(rest) = trimmed.strip_prefix("exclude-from-sample ") {
        let token = rest.split_whitespace().next()?;
        if token == old_name {
            let after = &rest[token.len()..];
            return Some(format!("{leading}exclude-from-sample {new_name}{after}"));
        }
        return None;
    }

    None
}

fn rename_name_parts_in_line(trimmed: &str, full: &str, old_name: &str, new_name: &str) -> Option<String> {
    // old_name includes the $ prefix (e.g., "$init")
    // Replace all occurrences of the $var token in the line
    if !full.contains(old_name) {
        return None;
    }

    let leading = &full[..full.len() - trimmed.len()];

    // name-parts $NAME = ...  (definition)
    if let Some(rest) = trimmed.strip_prefix("name-parts ") {
        let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
        let def_name = parts[0];
        let mut result_parts = Vec::new();
        let mut changed = false;

        if def_name == old_name {
            result_parts.push(new_name.to_string());
            changed = true;
        } else {
            result_parts.push(def_name.to_string());
        }

        if parts.len() > 1 {
            // Replace $var references in the values portion
            let values_str = parts[1];
            let new_values = replace_dollar_var(values_str, old_name, new_name);
            if new_values != values_str {
                changed = true;
            }
            result_parts.push(new_values);
        }

        if changed {
            return Some(format!("{leading}name-parts {}", result_parts.join(" ")));
        }
        return None;
    }

    // For glyph headers: replace $var in the name portion
    if let Some(rest) = trimmed.strip_prefix("glyph ") {
        let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
        let name = parts[0];
        if name.contains(old_name) {
            let new_n = replace_dollar_var(name, old_name, new_name);
            if new_n != name {
                let after = if parts.len() > 1 { format!(" {}", parts[1]) } else { String::new() };
                return Some(format!("{leading}glyph {new_n}{after}"));
            }
        }
        // Also check alias part
        if parts.len() > 1 {
            let after = parts[1];
            if after.contains(old_name) {
                let new_after = replace_dollar_var(after, old_name, new_name);
                if new_after != after {
                    return Some(format!("{leading}glyph {name} {new_after}"));
                }
            }
        }
        return None;
    }

    // ref NAME: replace $var in name
    if let Some(rest) = trimmed.strip_prefix("ref ") {
        let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
        let name = parts[0];
        if name.contains(old_name) {
            let new_n = replace_dollar_var(name, old_name, new_name);
            if new_n != name {
                let after = if parts.len() > 1 { format!(" {}", parts[1]) } else { String::new() };
                return Some(format!("{leading}ref {new_n}{after}"));
            }
        }
        return None;
    }

    // map CHAR = NAME: replace $var in glyph name
    if let Some(rest) = trimmed.strip_prefix("map ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() == 3 && parts[1] == "=" && parts[2].contains(old_name) {
            let new_n = replace_dollar_var(parts[2], old_name, new_name);
            if new_n != parts[2] {
                return Some(format!("{leading}map {} = {new_n}", parts[0]));
            }
        }
        return None;
    }

    // remap tokens: replace $var in tokens
    if let Some(rest) = trimmed.strip_prefix("remap ") {
        let mut changed = false;
        let new_tokens: Vec<String> = rest.split_whitespace().map(|tok| {
            if tok.contains(old_name) {
                let new_tok = replace_dollar_var(tok, old_name, new_name);
                if new_tok != tok {
                    changed = true;
                    return new_tok;
                }
            }
            tok.to_string()
        }).collect();
        if changed {
            return Some(format!("{leading}remap {}", new_tokens.join(" ")));
        }
        return None;
    }

    None
}

fn replace_dollar_var(text: &str, old_var: &str, new_var: &str) -> String {
    // Replace $old_name with $new_name, being careful about word boundaries
    // old_var includes the $ prefix
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let old_chars: Vec<char> = old_var.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '$' && i + old_chars.len() <= chars.len() {
            let slice: String = chars[i..i + old_chars.len()].iter().collect();
            if slice == old_var {
                // Check that the next char is NOT alphanumeric/dash/underscore (word boundary)
                let next_idx = i + old_chars.len();
                let at_boundary = next_idx >= chars.len()
                    || !(chars[next_idx].is_alphanumeric() || chars[next_idx] == '-' || chars[next_idx] == '_');
                if at_boundary {
                    result.push_str(new_var);
                    i += old_chars.len();
                    continue;
                }
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

fn rename_point_in_line(trimmed: &str, full: &str, old_name: &str, new_name: &str) -> Option<String> {
    let leading = &full[..full.len() - trimmed.len()];

    // anchor [+|-]NAME COL ROW  (or legacy: point [+|-]NAME COL ROW)
    let (keyword, rest) = if let Some(r) = trimmed.strip_prefix("anchor ") {
        ("anchor", r)
    } else if let Some(r) = trimmed.strip_prefix("point ") {
        ("anchor", r)
    } else {
        return None;
    };
    let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
    let token = parts[0];

    let (prefix_char, bare_name) = if let Some(stripped) = token.strip_prefix('+') {
        ("+", stripped)
    } else if let Some(stripped) = token.strip_prefix('-') {
        ("-", stripped)
    } else {
        ("", token)
    };

    if bare_name != old_name {
        return None;
    }

    let after = if parts.len() > 1 { format!(" {}", parts[1]) } else { String::new() };
    Some(format!("{leading}{keyword} {prefix_char}{new_name}{after}"))
}

fn rename_color_in_line(trimmed: &str, full: &str, old_name: &str, new_name: &str) -> Option<String> {
    let leading = &full[..full.len() - trimmed.len()];
    let tokens = crate::document_io::tokenize_tokens(trimmed).ok()?;
    if tokens.is_empty() {
        return None;
    }

    let mut changed = false;
    let mut new_tokens = tokens.clone();

    match tokens[0].as_str() {
        "color" => {
            if tokens.len() >= 4 && tokens[2] == "=" {
                if tokens[1] == old_name {
                    new_tokens[1] = new_name.to_string();
                    changed = true;
                }
                if tokens[3] == old_name {
                    new_tokens[3] = new_name.to_string();
                    changed = true;
                }
            }
        }
        "ref" => {
            if let Some(fill_pos) = tokens.iter().position(|t| t == "fill")
                && let Some(color_val) = tokens.get(fill_pos + 1)
                    && color_val == old_name {
                        new_tokens[fill_pos + 1] = new_name.to_string();
                        changed = true;
                    }
        }
        _ => {}
    }

    if !changed {
        return None;
    }
    let quoted: Vec<String> = new_tokens.iter().map(|t| crate::document_io::quote_token(t)).collect();
    Some(format!("{leading}{}", quoted.join(" ")))
}

#[cfg(test)]
mod rename_tests {
    use super::*;
    use crate::document::DocLine;
    use crate::editor::doc_links::RenameKind;

    fn t(s: &str) -> DocLine { DocLine::Text(s.to_string()) }

    fn do_rename(lines: &[DocLine], old: &str, new: &str, kind: &RenameKind) -> Vec<String> {
        rename_in_lines(lines, old, new, kind)
            .into_iter()
            .filter_map(|l| if let DocLine::Text(s) = l { Some(s) } else { None })
            .collect()
    }

    #[test]
    fn rename_glyph_header() {
        let lines = vec![t("glyph foo 8 16")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph bar 8 16"]);
    }

    #[test]
    fn rename_glyph_ref() {
        let lines = vec![t("ref foo 0 0")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["ref bar 0 0"]);
    }

    #[test]
    fn rename_glyph_map() {
        let lines = vec![t("map A = foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["map A = bar"]);
    }

    #[test]
    fn rename_glyph_alias() {
        let lines = vec![t("glyph new-name = foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph new-name = bar"]);
    }

    #[test]
    fn rename_glyph_def_in_alias_form() {
        let lines = vec![t("glyph foo = other")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph bar = other"]);
    }

    #[test]
    fn rename_glyph_alias_after_flags() {
        let lines = vec![t("glyph new-name advance 8 = foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph new-name advance 8 = bar"]);
    }

    #[test]
    fn rename_glyph_remap() {
        let lines = vec![t("remap liga : a b : foo -> bar-lig : c")];
        let result = do_rename(&lines, "foo", "quux", &RenameKind::Glyph);
        assert_eq!(result, vec!["remap liga : a b : quux -> bar-lig : c"]);
    }

    #[test]
    fn rename_glyph_exclude() {
        let lines = vec![t("exclude-from-sample foo")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["exclude-from-sample bar"]);
    }

    #[test]
    fn rename_glyph_no_partial_match() {
        let lines = vec![t("glyph foobar 8 16"), t("ref foo-ext 0 0")];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph foobar 8 16", "ref foo-ext 0 0"]);
    }

    #[test]
    fn rename_name_parts_def() {
        let lines = vec![t("name-parts $init = a b c")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        assert_eq!(result, vec!["name-parts $vowel = a b c"]);
    }

    #[test]
    fn rename_name_parts_ref_in_glyph() {
        let lines = vec![t("glyph hangul-($init)-l 8 16")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        assert_eq!(result, vec!["glyph hangul-($vowel)-l 8 16"]);
    }

    #[test]
    fn rename_name_parts_ref_in_ref() {
        let lines = vec![t("ref hangul-init-$init 0 0")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        assert_eq!(result, vec!["ref hangul-init-$vowel 0 0"]);
    }

    #[test]
    fn rename_name_parts_in_values() {
        let lines = vec![t("name-parts $combo = $init $final")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        assert_eq!(result, vec!["name-parts $combo = $vowel $final"]);
    }

    #[test]
    fn rename_name_parts_no_partial() {
        let lines = vec![t("ref hangul-$initial 0 0")];
        let result = do_rename(&lines, "$init", "$vowel", &RenameKind::NameParts);
        // $initial should NOT be renamed to $vowelial
        assert_eq!(result, vec!["ref hangul-$initial 0 0"]);
    }

    #[test]
    fn rename_point_plus() {
        let lines = vec![t("point +above 4 1")];
        let result = do_rename(&lines, "above", "top", &RenameKind::Point);
        assert_eq!(result, vec!["anchor +top 4 1"]);
    }

    #[test]
    fn rename_point_minus() {
        let lines = vec![t("point -above 2 1")];
        let result = do_rename(&lines, "above", "top", &RenameKind::Point);
        assert_eq!(result, vec!["anchor -top 2 1"]);
    }

    #[test]
    fn rename_point_both_variants() {
        let lines = vec![t("point +above 4 1"), t("point -above 2 1")];
        let result = do_rename(&lines, "above", "top", &RenameKind::Point);
        assert_eq!(result, vec!["anchor +top 4 1", "anchor -top 2 1"]);
    }

    #[test]
    fn rename_leaves_unrelated_lines() {
        let lines = vec![
            t("glyph foo 8 16"),
            t("ref baz 0 0"),
            t("map X = foo"),
        ];
        let result = do_rename(&lines, "foo", "bar", &RenameKind::Glyph);
        assert_eq!(result, vec!["glyph bar 8 16", "ref baz 0 0", "map X = bar"]);
    }
}

#[cfg(test)]
mod font_build_tests {
    use super::*;

    #[test]
    fn stale_background_font_cannot_replace_current_result() {
        let (tx, rx) = mpsc::channel();
        tx.send((2, Some((vec![2], vec![20])))).unwrap();
        // An older, slower build finishes after the current one.
        tx.send((1, Some((vec![1], vec![10])))).unwrap();

        assert_eq!(
            take_current_font_build(&rx, 2),
            Some(Some((vec![2], vec![20]))),
        );
    }

    #[test]
    fn current_failed_build_clears_previous_font() {
        let (tx, rx) = mpsc::channel();
        tx.send((3, None)).unwrap();

        assert_eq!(take_current_font_build(&rx, 3), Some(None));
    }

    #[test]
    fn opening_document_replaces_base_in_path_order() {
        let base_a = Document::new("a.unf".into());
        let base_b = Document::new("b.unf".into());
        let open_b = OpenDocument {
            document: Document::new("b.unf".into()),
            lines: Vec::new(),
            editor_state: EditorState::new(),
        };

        let open = [open_b];
        let base = [base_b, base_a];
        let docs = collect_effective_docs(&open, &base);
        assert_eq!(
            docs.iter().map(|doc| doc.path.as_path()).collect::<Vec<_>>(),
            [std::path::Path::new("a.unf"), std::path::Path::new("b.unf")],
        );
        assert_eq!(docs.len(), 2, "the base copy of b.unf must be replaced");
    }
}
