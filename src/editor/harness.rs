//! Headless GUI test harness for the document editor.
//!
//! Drives the real `show_document` inside a plain `egui::Context` (no
//! window, no GPU) so that tests can feed synthetic input events — key
//! presses, text, IME, mouse clicks on computed coordinates — and then
//! assert on both the document/editor state and the *rendered layout*
//! (visual lines, grid rows, gutter line numbers).
//!
//! `show_document` publishes a [`ViewSnapshot`] of every frame's layout via
//! `capture_snapshot` (compiled only under `cfg(test)`), which is what the
//! query helpers here read.
//!
//! This is how editor behaviour is meant to be tested — scenarios go in
//! `editor/view_tests.rs`. Refactoring "for testability" has repeatedly left the
//! frame loop itself untested, and the regressions that matter are scenario-level
//! (a grid demoted to text mid-header-edit, focus lost after a drag, a wheel tick
//! reaching the wrong surface): invisible to anything below the loop.
//!
//! Gotcha the harness already handles: synthetic clicks need spacing in time, or
//! egui reads two of them as a double-click.
//!
//! What it papers over: resolution. Every frame that rederives the document
//! re-resolves the whole document set synchronously, so the resolved glyphs a
//! scenario reads are always current. The app's are not — they come from a
//! debounced background thread ([`crate::app::UniformApp`]), so anything about
//! *when* a shape becomes visible has to be tested below this harness.

use std::collections::HashMap;
use std::sync::Arc;

use crate::document::{DocLine, Document, NamePartsMap, PixelGrid, collect_name_parts};
use crate::document_io::{derive_document, parse_doclines};
use crate::editor::annotations::{AnnotatedText, InlineAnnotation};
use crate::editor::caret::Caret;
use crate::editor::document_view::{
    DocumentEditor, EditorEnv, GlyphMetrics, GridStrip, LEFT_PAD, VLineKind, VisualLine,
    gutter_line_number,
};
use crate::editor::ref_composite::{
    AlternativesIndex, ResolvedGlyph, resolve_named_glyphs_with_parts,
};
use crate::editor::{EditorId, EditorState, Slot};

// ---------------------------------------------------------------------------
// Per-frame layout snapshot
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(crate) enum SnapKind {
    Text {
        text: String,
        col_offset: usize,
        /// The line as rendered, inline annotations spliced in.
        display: String,
        annotations: Vec<InlineAnnotation>,
        /// Where this segment's `// …` comment starts, if any; painted in the
        /// comment color regardless of the line's own color.
        comment_col: Option<usize>,
    },
    GridRow {
        #[allow(dead_code)]
        item_idx: usize,
        row: i16,
        left: i16,
        #[allow(dead_code)]
        right: i16,
        metrics: Option<GlyphMetrics>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct SnapLine {
    pub doc_line: usize,
    /// Absolute screen y of this visual line's top edge.
    pub y: f32,
    pub height: f32,
    /// Line number drawn in the gutter, if any.
    pub gutter: Option<usize>,
    pub kind: SnapKind,
}

#[derive(Clone, Debug)]
pub(crate) struct ViewSnapshot {
    /// Absolute screen x where text/grids start (right of the gutter).
    pub origin_x: f32,
    /// Width the gutter reserved for fold markers, `0.0` when it reserved
    /// none. Subtract it to compare the line-number field alone.
    pub marker_width: f32,
    #[allow(dead_code)]
    pub row_height: f32,
    pub grid_cell: f32,
    pub widget_id: egui::Id,
    /// The band the grids are drawn in, with the frame's horizontal offset.
    pub strip: GridStrip,
    pub vlines: Vec<SnapLine>,
}

impl ViewSnapshot {
    /// Screen x of column `left` for a grid row of `left..right`.
    pub fn grid_row_x(&self, left: i16, right: i16) -> f32 {
        self.strip.grid_x((right - left) as f32 * self.grid_cell)
    }
}

fn snapshot_id(editor: EditorId) -> egui::Id {
    editor.key(Slot::TestViewSnapshot)
}

fn ref_rects_id(editor: EditorId) -> egui::Id {
    editor.key(Slot::TestRefRects)
}

/// Called from `draw_inline_tools_panel` (test builds only) to publish the
/// on-screen rect of a ref-layer thumbnail, so tests can click it precisely
/// without re-deriving the panel's layout math by hand.
pub(crate) fn capture_ref_rect(
    ctx: &egui::Context,
    editor: EditorId,
    edit_idx: usize,
    ref_idx: usize,
    rect: egui::Rect,
) {
    ctx.data_mut(|d| {
        let mut map = d
            .get_temp::<HashMap<(usize, usize), egui::Rect>>(ref_rects_id(editor))
            .unwrap_or_default();
        map.insert((edit_idx, ref_idx), rect);
        d.insert_temp(ref_rects_id(editor), map);
    });
}

fn palette_rects_id(editor: EditorId) -> egui::Id {
    editor.key(Slot::TestPaletteRects)
}

/// Called from `draw_inline_palette` (test builds only) to publish the
/// on-screen rect of one shape-palette cell, together with the fill it was
/// drawn with (filled = the solid colour, unfilled = the dimmed one).
pub(crate) fn capture_palette_rect(
    ctx: &egui::Context,
    editor: EditorId,
    cell: usize,
    rect: egui::Rect,
    filled: bool,
) {
    ctx.data_mut(|d| {
        let mut map = d
            .get_temp::<HashMap<usize, (egui::Rect, bool)>>(palette_rects_id(editor))
            .unwrap_or_default();
        map.insert(cell, (rect, filled));
        d.insert_temp(palette_rects_id(editor), map);
    });
}

fn color_spans_id(editor: EditorId) -> egui::Id {
    editor.key(Slot::TestColorSpans)
}

/// Called from the document paint loop (test builds only) to publish the
/// color-token backgrounds one visual line painted, in absolute document
/// columns clipped to that line. A token a soft wrap moved onto a later
/// segment still gets one, so a test can tell "drawn elsewhere" from "dropped".
pub(crate) fn capture_color_spans(
    ctx: &egui::Context,
    editor: EditorId,
    doc_line: usize,
    spans: &[(usize, usize)],
) {
    if spans.is_empty() {
        return;
    }
    ctx.data_mut(|d| {
        let mut all = d
            .get_temp::<Vec<(usize, usize, usize)>>(color_spans_id(editor))
            .unwrap_or_default();
        all.extend(spans.iter().map(|(s, e)| (doc_line, *s, *e)));
        d.insert_temp(color_spans_id(editor), all);
    });
}

fn edit_border_id(editor: EditorId) -> egui::Id {
    editor.key(Slot::TestEditBorder)
}

/// Called from the document paint loop (test builds only) to publish the
/// edit-mode border rect it painted, so a test can tell "painted somewhere
/// off-screen" from "not painted at all".
pub(crate) fn capture_edit_border(ctx: &egui::Context, editor: EditorId, rect: egui::Rect) {
    ctx.data_mut(|d| d.insert_temp(edit_border_id(editor), Some(rect)));
}

fn resize_buttons_id(editor: EditorId) -> egui::Id {
    editor.key(Slot::TestResizeButtons)
}

/// One rectangle as the frame painted it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PaintedRect {
    pub rect: egui::Rect,
    pub fill: egui::Color32,
    pub stroke: egui::Stroke,
    pub clip: egui::Rect,
}

/// One Apply/Cancel button as it was painted: where it is, and the two
/// colours a theme has to keep apart.
pub(crate) type ResizeButton = (
    crate::editor::glyph_resize::PanelAction,
    egui::Rect,
    egui::Color32,
    egui::Color32,
);

/// Called from `glyph_resize::draw_panel` (test builds only) to publish the
/// Apply/Cancel buttons, so a test clicks them where they really are and can
/// see what they were painted with.
pub(crate) fn capture_resize_buttons(
    ctx: &egui::Context,
    editor: EditorId,
    rects: &[ResizeButton],
) {
    ctx.data_mut(|d| d.insert_temp(resize_buttons_id(editor), rects.to_vec()));
}

fn fold_markers_id(editor: EditorId) -> egui::Id {
    editor.key(Slot::TestFoldMarkers)
}

/// One fold marker as it was painted: the group's header line, the cell the
/// click test uses, and which way its triangle points.
pub(crate) type FoldMarkerRect = (usize, egui::Rect, bool);

/// Called from `paint_fold_markers` (test builds only), so a test clicks a
/// marker where it really is instead of re-deriving the gutter's geometry.
pub(crate) fn capture_fold_markers(
    ctx: &egui::Context,
    editor: EditorId,
    markers: &[FoldMarkerRect],
) {
    ctx.data_mut(|d| d.insert_temp(fold_markers_id(editor), markers.to_vec()));
}

fn fold_marker_hover_id(editor: EditorId) -> egui::Id {
    editor.key(Slot::TestFoldMarkerHover)
}

/// Called from `paint_fold_markers` (test builds only) with the group the
/// pointer shaded, so a test can see that a hover outside the editor's own
/// viewport shades nothing.
pub(crate) fn capture_fold_marker_hover(
    ctx: &egui::Context,
    editor: EditorId,
    hovered: Option<usize>,
) {
    ctx.data_mut(|d| d.insert_temp(fold_marker_hover_id(editor), hovered));
}

/// Called from `show_document` (test builds only) to publish the layout the
/// frame is about to paint.
#[allow(clippy::too_many_arguments)]
pub(crate) fn capture_snapshot(
    ctx: &egui::Context,
    editor: EditorId,
    vlines: &[VisualLine],
    lines: &[DocLine],
    source_offsets: &[usize],
    origin: egui::Pos2,
    row_height: f32,
    grid_cell: f32,
    widget_id: egui::Id,
    strip: &GridStrip,
    marker_width: f32,
) {
    let mut y = origin.y;
    let mut snaps = Vec::with_capacity(vlines.len());
    for vl in vlines {
        let h = vl.height(row_height, grid_cell);
        let kind = match &vl.kind {
            VLineKind::Text(t) => SnapKind::Text {
                text: t.clone(),
                col_offset: vl.col_offset,
                display: vl
                    .annotated_text()
                    .map_or_else(|| t.clone(), |a| a.display_string()),
                annotations: vl.annotations.clone(),
                comment_col: vl.comment_col,
            },
            VLineKind::GridRow {
                item_idx,
                row,
                extent,
                metrics,
                ..
            } => SnapKind::GridRow {
                item_idx: *item_idx,
                row: *row,
                left: extent.left,
                right: extent.right,
                metrics: *metrics,
            },
        };
        snaps.push(SnapLine {
            doc_line: vl.doc_line,
            y,
            height: h,
            gutter: gutter_line_number(vl, lines, source_offsets),
            kind,
        });
        y += h;
    }
    let snapshot = ViewSnapshot {
        origin_x: origin.x,
        marker_width,
        row_height,
        grid_cell,
        widget_id,
        strip: strip.clone(),
        vlines: snaps,
    };
    ctx.data_mut(|d| d.insert_temp(snapshot_id(editor), Arc::new(snapshot)));
    // The paint loop appends to this as it goes, so the frame starts clean.
    ctx.data_mut(|d| d.insert_temp(color_spans_id(editor), Vec::<(usize, usize, usize)>::new()));
    ctx.data_mut(|d| d.insert_temp(edit_border_id(editor), None::<egui::Rect>));
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

pub(crate) struct EditorHarness {
    pub ctx: egui::Context,
    pub doc: Document,
    pub lines: Vec<DocLine>,
    pub state: EditorState,
    pub named_glyphs: HashMap<String, ResolvedGlyph>,
    pub alt_index: AlternativesIndex,
    pub name_parts: NamePartsMap,
    pub meta: crate::meta::FontMetrics,
    /// Off by default: the metric box widens the drawn grid, and every layout
    /// assertion written before it existed expects the un-widened extents.
    pub show_metrics: bool,
    /// Stands in for a menu-bar menu being open over the editor, which is the
    /// one kind of focus loss a pixel selection survives.
    pub menu_open: bool,
    pub zoom: u32,
    pub font_id: egui::FontId,
    time: f64,
    snapshot: Option<Arc<ViewSnapshot>>,
    pub last_copied_text: Option<String>,
    /// Every shape the last frame emitted, before tessellation — the only way
    /// to ask what was actually *drawn* rather than what the code meant to
    /// draw, clip rects included.
    last_shapes: Vec<egui::epaint::ClippedShape>,
    /// The navigation request the primary editor reported on the most recent
    /// frame that produced one — what the host would act on and record.
    pub last_nav: Option<crate::editor::document_view::NavRequest>,
    /// The resize the primary editor applied on the most recent frame that
    /// produced one — what the host would then carry across the font.
    last_resize: Option<crate::editor::glyph_resize::ResizeAction>,
    /// While set, a menu-like `egui::Area` is drawn above the editor over this
    /// rect, so a test can send a click that lands on a popup covering the
    /// grid instead of on the grid itself.
    pub menu_overlay: Option<egui::Rect>,
    /// While set, the single-pane editor is drawn into a band this tall at the
    /// top of the screen instead of the whole of it, so a test can put the
    /// pointer *below* the editor — where the app has its preview panel.
    pub viewport_height: Option<f32>,
    /// A second editor drawn beside the primary one in the *same* context and
    /// the same frame. Only [`EditorHarness::split`] creates it; every other
    /// test keeps the single-pane layout untouched.
    pub second: Option<Pane>,
}

/// A secondary editor's own document and state, so a test can assert that two
/// live editors keep their scroll offsets, carets and layouts to themselves.
pub(crate) struct Pane {
    pub doc: Document,
    pub lines: Vec<DocLine>,
    pub state: EditorState,
    pub named_glyphs: HashMap<String, ResolvedGlyph>,
    pub alt_index: AlternativesIndex,
    pub name_parts: NamePartsMap,
    pub meta: crate::meta::FontMetrics,
}

impl Pane {
    fn new(source: &str) -> Self {
        let lines = parse_doclines(source);
        let (doc, _) = derive_document(&lines, "second.unf".into()).expect("derive_document");
        let mut pane = Self {
            doc,
            lines,
            state: EditorState::new(),
            named_glyphs: HashMap::new(),
            alt_index: AlternativesIndex::default(),
            name_parts: NamePartsMap::new(),
            meta: Default::default(),
        };
        pane.rebuild_derived();
        pane
    }

    fn rebuild_derived(&mut self) {
        let docs: Vec<&Document> = vec![&self.doc];
        let name_parts = collect_name_parts(&docs);
        let (named_glyphs, alt_index) = resolve_named_glyphs_with_parts(&docs, &name_parts);
        self.meta = crate::meta::FontMeta::collect(&docs).metrics;
        self.named_glyphs = named_glyphs;
        self.alt_index = alt_index;
        self.name_parts = name_parts;
    }
}

impl EditorHarness {
    /// Build a harness from `.unf` source text, mirroring how the app opens
    /// a document (parse → derive → resolve glyphs), and render an initial
    /// frame so layout queries work immediately.
    pub fn new(source: &str) -> Self {
        let lines = parse_doclines(source);
        let (doc, _) = derive_document(&lines, "test.unf".into()).expect("derive_document");
        let mut h = Self {
            ctx: egui::Context::default(),
            doc,
            lines,
            state: EditorState::new(),
            named_glyphs: HashMap::new(),
            alt_index: AlternativesIndex::default(),
            name_parts: NamePartsMap::new(),
            meta: Default::default(),
            show_metrics: false,
            menu_open: false,
            zoom: 1,
            font_id: egui::FontId::monospace(16.0),
            time: 0.0,
            snapshot: None,
            last_copied_text: None,
            last_shapes: Vec::new(),
            last_nav: None,
            last_resize: None,
            menu_overlay: None,
            viewport_height: None,
            second: None,
        };
        h.rebuild_derived();
        h.frame();
        h.frame();
        h
    }

    /// Adds a second editor on `source`, drawn beside the primary one in the
    /// same context, and settles both.
    pub fn split(&mut self, source: &str) {
        self.second = Some(Pane::new(source));
        self.frame();
        self.frame();
    }

    fn rebuild_derived(&mut self) {
        let docs: Vec<&Document> = vec![&self.doc];
        let name_parts = collect_name_parts(&docs);
        let (named_glyphs, alt_index) = resolve_named_glyphs_with_parts(&docs, &name_parts);
        self.meta = crate::meta::FontMeta::collect(&docs).metrics;
        self.named_glyphs = named_glyphs;
        self.alt_index = alt_index;
        self.name_parts = name_parts;
    }

    /// Advance the harness clock without running a frame. Successive wheel
    /// ticks in the same direction are debounced on real time
    /// (`COARSE_SCROLL_COOLDOWN`), so a test sending more than one must space
    /// them out the way real ticks are.
    pub fn advance_time(&mut self, seconds: f64) {
        self.time += seconds;
    }

    /// Run one frame of the real editor with the given input events.
    pub fn frame_with(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
        self.time += 1.0 / 60.0;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1000.0, 2400.0),
            )),
            time: Some(self.time),
            modifiers,
            events,
            ..Default::default()
        };
        let prev_gen = self.doc.edit_gen;
        let prev_second_gen = self.second.as_ref().map(|p| p.doc.edit_gen);
        let ctx = self.ctx.clone();
        let mut nav_result = None;
        let mut resize_result = None;
        let menu_overlay = self.menu_overlay;
        let viewport_height = self.viewport_height;
        let full_output = ctx.run(raw, |cx| {
            if let Some(rect) = menu_overlay {
                egui::Area::new(egui::Id::new("test_menu_overlay"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(rect.min)
                    .show(cx, |ui| {
                        ui.allocate_response(rect.size(), egui::Sense::click());
                    });
            }
            egui::CentralPanel::default().show(cx, |ui| {
                let colors = crate::render::ttf_builder::ColorAliasMap::default();
                let Some(second) = &mut self.second else {
                    let mut draw = |ui: &mut egui::Ui| {
                        let result = DocumentEditor::new(
                            &mut self.doc,
                            &mut self.lines,
                            &mut self.state,
                            EditorEnv {
                                named_glyphs: &self.named_glyphs,
                                name_parts: &self.name_parts,
                                alt_index: &self.alt_index,
                                color_aliases: &colors,
                                meta: self.meta,
                                show_metrics: self.show_metrics,
                                menu_open: self.menu_open,
                                derived_gen: 0,
                                font_gen: 0,
                                zoom_level: self.zoom,
                                font_id: &self.font_id,
                            },
                        )
                        .show(ui);
                        nav_result = result.nav;
                        resize_result = result.resize;
                    };
                    match viewport_height {
                        // A band at the top of the screen, so what is below it
                        // is outside the editor the way the preview panel is.
                        Some(h) => {
                            let size = egui::vec2(ui.available_width(), h);
                            ui.allocate_ui(size, |ui| draw(ui));
                        }
                        None => draw(ui),
                    }
                    return;
                };
                // Split layout: each editor gets half the width, so the two
                // occupy disjoint screen space and only their `egui` ids can
                // collide.
                let pane_size = egui::vec2(ui.available_width() * 0.5, ui.available_height());
                ui.horizontal_top(|ui| {
                    ui.allocate_ui(pane_size, |ui| {
                        let result = DocumentEditor::new(
                            &mut self.doc,
                            &mut self.lines,
                            &mut self.state,
                            EditorEnv {
                                named_glyphs: &self.named_glyphs,
                                name_parts: &self.name_parts,
                                alt_index: &self.alt_index,
                                color_aliases: &colors,
                                meta: self.meta,
                                show_metrics: self.show_metrics,
                                menu_open: self.menu_open,
                                derived_gen: 0,
                                font_gen: 0,
                                zoom_level: self.zoom,
                                font_id: &self.font_id,
                            },
                        )
                        .show(ui);
                        nav_result = result.nav;
                        resize_result = result.resize;
                    });
                    ui.allocate_ui(pane_size, |ui| {
                        let _ = DocumentEditor::new(
                            &mut second.doc,
                            &mut second.lines,
                            &mut second.state,
                            EditorEnv {
                                named_glyphs: &second.named_glyphs,
                                name_parts: &second.name_parts,
                                alt_index: &second.alt_index,
                                color_aliases: &colors,
                                meta: second.meta,
                                show_metrics: self.show_metrics,
                                menu_open: self.menu_open,
                                derived_gen: 0,
                                font_gen: 0,
                                zoom_level: self.zoom,
                                font_id: &self.font_id,
                            },
                        )
                        .show(ui);
                    });
                });
            });
        });
        if nav_result.is_some() {
            self.last_nav = nav_result;
        }
        if resize_result.is_some() {
            self.last_resize = resize_result;
        }
        for cmd in &full_output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = cmd {
                self.last_copied_text = Some(text.clone());
            }
        }
        self.last_shapes = full_output.shapes.clone();
        self.snapshot = self.snapshot_of(&self.state);
        if self.doc.edit_gen != prev_gen {
            // The app rebuilds resolved glyphs whenever a document rederives.
            self.rebuild_derived();
        }
        if let Some(second) = &mut self.second
            && prev_second_gen != Some(second.doc.edit_gen)
        {
            second.rebuild_derived();
        }
    }

    /// Run one idle frame (no input).
    pub fn frame(&mut self) {
        self.frame_with(Vec::new(), egui::Modifiers::NONE);
    }

    // -- input ------------------------------------------------------------

    /// Give the editor widget keyboard focus without clicking.
    #[allow(dead_code)]
    pub fn focus(&mut self) {
        let wid = self.snap().widget_id;
        self.ctx.memory_mut(|m| m.request_focus(wid));
        self.frame();
    }

    /// Take keyboard focus away from the editor widget, as if the user
    /// focused another panel.
    pub fn blur(&mut self) {
        let wid = self.snap().widget_id;
        self.ctx.memory_mut(|m| m.surrender_focus(wid));
        self.frame();
    }

    pub fn key(&mut self, key: egui::Key) {
        self.key_mod(key, egui::Modifiers::NONE);
    }

    pub fn key_mod(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        self.frame_with(
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            modifiers,
        );
        self.frame_with(
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers,
            }],
            modifiers,
        );
        self.frame();
    }

    /// Type text as if entered from the keyboard.
    pub fn type_text(&mut self, text: &str) {
        self.frame_with(
            vec![egui::Event::Text(text.to_string())],
            egui::Modifiers::NONE,
        );
        self.frame();
    }

    /// Send a Copy event (Cmd+C / Ctrl+C).
    pub fn copy(&mut self) {
        self.last_copied_text = None;
        self.frame_with(vec![egui::Event::Copy], egui::Modifiers::COMMAND);
        self.frame();
    }

    /// Send a Cut event (Cmd+X / Ctrl+X).
    pub fn cut(&mut self) {
        self.last_copied_text = None;
        self.frame_with(vec![egui::Event::Cut], egui::Modifiers::COMMAND);
        self.frame();
    }

    /// Paste text as if from the clipboard (Cmd+V / Ctrl+V).
    pub fn paste(&mut self, text: &str) {
        self.frame_with(
            vec![egui::Event::Paste(text.to_string())],
            egui::Modifiers::NONE,
        );
        self.frame();
    }

    /// Click the primary mouse button at an absolute screen position.
    pub fn click_at(&mut self, pos: egui::Pos2) {
        self.click_at_mod(pos, egui::Modifiers::NONE);
    }

    pub fn click_at_mod(&mut self, pos: egui::Pos2, modifiers: egui::Modifiers) {
        // Space successive clicks out in time so they don't register as
        // double/triple clicks.
        self.time += 1.0;
        self.frame_with(
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers,
                },
            ],
            modifiers,
        );
        self.frame_with(
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers,
            }],
            modifiers,
        );
        self.frame();
    }

    /// Click with press and release inside a *single* frame, as a fast click
    /// on a slow frame produces.  Widgets then see the whole click while any
    /// popup that the press dismisses is still open.
    pub fn click_at_same_frame(&mut self, pos: egui::Pos2) {
        self.time += 1.0; // don't let it read as a double click
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.frame_with(
            vec![egui::Event::PointerMoved(pos), button(true), button(false)],
            egui::Modifiers::NONE,
        );
        self.frame();
    }

    /// Click on a text line at a character column.
    pub fn click_text(&mut self, line: usize, col: usize) {
        let pos = self.text_pos(line, col);
        self.click_at(pos);
    }

    /// Click the center of a grid cell. `grid_doc_line` is the DocLine index
    /// of the `DocLine::Grid`; `row`/`col` are grid coordinates.
    pub fn click_grid_cell(&mut self, grid_doc_line: usize, row: i16, col: i16) {
        let pos = self.grid_cell_pos(grid_doc_line, row, col);
        self.click_at(pos);
    }

    /// Click the center of a grid cell with modifiers held.
    pub fn click_grid_cell_mod(
        &mut self,
        grid_doc_line: usize,
        row: i16,
        col: i16,
        modifiers: egui::Modifiers,
    ) {
        let pos = self.grid_cell_pos(grid_doc_line, row, col);
        self.click_at_mod(pos, modifiers);
    }

    /// Drag from one grid cell to another (primary button).
    pub fn drag_grid(&mut self, grid_doc_line: usize, from: (i16, i16), to: (i16, i16)) {
        self.drag_grid_mod(grid_doc_line, from, to, egui::Modifiers::NONE);
    }

    /// Drag from one grid cell to another with modifiers held throughout.
    pub fn drag_grid_mod(
        &mut self,
        grid_doc_line: usize,
        from: (i16, i16),
        to: (i16, i16),
        modifiers: egui::Modifiers,
    ) {
        let from_pos = self.grid_cell_pos(grid_doc_line, from.0, from.1);
        let to_pos = self.grid_cell_pos(grid_doc_line, to.0, to.1);

        self.time += 1.0;
        // Press at start position
        self.frame_with(
            vec![
                egui::Event::PointerMoved(from_pos),
                egui::Event::PointerButton {
                    pos: from_pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers,
                },
            ],
            modifiers,
        );

        // Move to destination over a few frames
        let steps = 4;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let pos = egui::pos2(
                from_pos.x + (to_pos.x - from_pos.x) * t,
                from_pos.y + (to_pos.y - from_pos.y) * t,
            );
            self.frame_with(vec![egui::Event::PointerMoved(pos)], modifiers);
        }

        // Release
        self.frame_with(
            vec![egui::Event::PointerButton {
                pos: to_pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers,
            }],
            modifiers,
        );
        self.frame();
    }

    /// Press the primary button at a position and keep it held.
    pub fn press_at(&mut self, pos: egui::Pos2) {
        self.time += 1.0;
        self.frame_with(
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            egui::Modifiers::NONE,
        );
    }

    /// Move the pointer for one frame without changing button state.
    pub fn move_pointer(&mut self, pos: egui::Pos2) {
        self.frame_with(vec![egui::Event::PointerMoved(pos)], egui::Modifiers::NONE);
    }

    /// Release the primary button at a position.
    pub fn release_at(&mut self, pos: egui::Pos2) {
        self.frame_with(
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            egui::Modifiers::NONE,
        );
    }

    /// Alt + wheel over a screen position, one coarse tick. `up` is the
    /// direction the wheel is pushed. The coarse debounce is shared and lasts
    /// longer than one frame, so a few idle frames are run first to let a
    /// preceding tick expire.
    pub fn alt_wheel_at(&mut self, pos: egui::Pos2, up: bool) {
        self.wheel_at_mod(pos, up, egui::Modifiers::ALT);
    }

    /// One coarse wheel tick over a screen position with arbitrary modifiers.
    pub fn wheel_at_mod(&mut self, pos: egui::Pos2, up: bool, modifiers: egui::Modifiers) {
        for _ in 0..5 {
            self.frame();
        }
        let dy = if up { 1.0 } else { -1.0 };
        self.frame_with(
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, dy),
                    modifiers,
                },
            ],
            modifiers,
        );
        self.frame();
    }

    /// Right-click at a screen position.
    pub fn right_click_at(&mut self, pos: egui::Pos2) {
        self.time += 1.0;
        self.frame_with(
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            egui::Modifiers::NONE,
        );
        self.frame_with(
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            egui::Modifiers::NONE,
        );
        self.frame();
    }

    // -- coordinate lookup --------------------------------------------------

    pub fn snap(&self) -> &ViewSnapshot {
        self.snapshot
            .as_deref()
            .expect("no frame has been rendered yet")
    }

    /// x offset of document column `col`, inline annotations included, so
    /// clicking lands where the editor actually draws that column.
    fn char_x(&self, text: &str, annotations: &[InlineAnnotation], col: usize) -> f32 {
        let prefix = AnnotatedText::new(text, annotations).display_prefix(col);
        if prefix.is_empty() {
            return 0.0;
        }
        let font_id = self.font_id.clone();
        self.ctx.fonts(|f| {
            f.layout_no_wrap(prefix, font_id, egui::Color32::WHITE)
                .rect
                .width()
        })
    }

    /// Screen position just right of the caret slot at `line:col`, suitable
    /// for clicking to place the caret there.
    pub fn text_pos(&self, line: usize, col: usize) -> egui::Pos2 {
        let snap = self.snap();
        for vl in &snap.vlines {
            if vl.doc_line != line {
                continue;
            }
            if let SnapKind::Text {
                text,
                col_offset,
                annotations,
                ..
            } = &vl.kind
            {
                let len = text.chars().count();
                if col >= *col_offset && col <= col_offset + len {
                    let x = snap.origin_x
                        + LEFT_PAD
                        + self.char_x(text, annotations, col - col_offset)
                        + 1.0;
                    return egui::pos2(x, vl.y + vl.height * 0.5);
                }
            }
        }
        panic!("no text visual line covering doc line {line} col {col}");
    }

    /// The color-token backgrounds the last frame painted, as `(doc_line,
    /// col_start, col_end)` in absolute document columns.
    pub fn color_backgrounds(&self) -> Vec<(usize, usize, usize)> {
        self.ctx
            .data(|d| d.get_temp::<Vec<(usize, usize, usize)>>(color_spans_id(self.state.id())))
            .unwrap_or_default()
    }

    /// The edit-mode border rect the last frame painted, if any.
    pub fn edit_border_rect(&self) -> Option<egui::Rect> {
        self.ctx
            .data(|d| d.get_temp::<Option<egui::Rect>>(edit_border_id(self.state.id())))
            .flatten()
    }

    /// Screen position of the resize mode's Apply or Cancel button (available
    /// once the panel has rendered a frame in `EditMode::GlyphResize`).
    pub fn resize_button_pos(
        &self,
        action: crate::editor::glyph_resize::PanelAction,
    ) -> egui::Pos2 {
        self.resize_button(action).1.center()
    }

    /// The `(fill, text)` colours the last frame painted that button with.
    pub fn resize_button_colors(
        &self,
        action: crate::editor::glyph_resize::PanelAction,
    ) -> (egui::Color32, egui::Color32) {
        let b = self.resize_button(action);
        (b.2, b.3)
    }

    fn resize_button(&self, action: crate::editor::glyph_resize::PanelAction) -> ResizeButton {
        self.ctx
            .data(|d| d.get_temp::<Vec<ResizeButton>>(resize_buttons_id(self.state.id())))
            .and_then(|v| v.iter().find(|b| b.0 == action).copied())
            .expect("resize panel not captured -- was the editor in resize mode?")
    }

    /// Every rectangle the last frame painted, in paint order — which is what
    /// makes "drawn but painted over" visible to a test at all.
    pub fn painted_rects(&self) -> Vec<PaintedRect> {
        fn walk(shape: &egui::Shape, clip: egui::Rect, out: &mut Vec<PaintedRect>) {
            match shape {
                egui::Shape::Rect(r) => out.push(PaintedRect {
                    rect: r.rect,
                    fill: r.fill,
                    stroke: r.stroke.into(),
                    clip,
                }),
                egui::Shape::Vec(v) => {
                    for s in v {
                        walk(s, clip, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for cs in &self.last_shapes {
            walk(&cs.shape, cs.clip_rect, &mut out);
        }
        out
    }

    /// Switch the context's theme, as the View menu does, and settle a frame.
    pub fn set_theme(&mut self, theme: egui::Theme) {
        self.ctx.set_theme(theme);
        self.frame();
        self.frame();
    }

    /// The resize the last frame handed to the host, if any.
    pub fn take_resize(&mut self) -> Option<crate::editor::glyph_resize::ResizeAction> {
        self.last_resize.take()
    }

    /// Screen rect of a ref-layer thumbnail in the inline tools panel (only
    /// available once that panel has rendered a frame with the glyph at
    /// `edit_idx` being edited).
    pub fn ref_thumbnail_rect(&self, edit_idx: usize, ref_idx: usize) -> egui::Rect {
        let map = self.ctx.data(|d| {
            d.get_temp::<HashMap<(usize, usize), egui::Rect>>(ref_rects_id(self.state.id()))
        });
        map.and_then(|m| m.get(&(edit_idx, ref_idx)).copied())
            .expect("ref thumbnail rect not captured -- was the inline tools panel rendered?")
    }

    /// Screen position of the center of a ref-layer thumbnail in the inline
    /// tools panel (only available once that panel has rendered a frame with
    /// the glyph at `edit_idx` being edited).
    pub fn ref_thumbnail_pos(&self, edit_idx: usize, ref_idx: usize) -> egui::Pos2 {
        self.ref_thumbnail_rect(edit_idx, ref_idx).center()
    }

    /// Screen position of the center of shape-palette cell `cell` (available
    /// once the inline tools panel has rendered a frame in GlyphEdit mode).
    pub fn palette_cell_pos(&self, cell: usize) -> egui::Pos2 {
        self.palette_cell_draw(cell).0.center()
    }

    /// Whether shape-palette cell `cell` was drawn filled on the last frame.
    pub fn palette_cell_filled(&self, cell: usize) -> bool {
        self.palette_cell_draw(cell).1
    }

    fn palette_cell_draw(&self, cell: usize) -> (egui::Rect, bool) {
        let map = self.ctx.data(|d| {
            d.get_temp::<HashMap<usize, (egui::Rect, bool)>>(palette_rects_id(self.state.id()))
        });
        map.and_then(|m| m.get(&cell).copied())
            .expect("palette cell rect not captured -- was the shape palette rendered?")
    }

    /// Screen position of the center of a grid cell.
    pub fn grid_cell_pos(&self, grid_doc_line: usize, row: i16, col: i16) -> egui::Pos2 {
        let snap = self.snap();
        for vl in &snap.vlines {
            if vl.doc_line != grid_doc_line {
                continue;
            }
            if let SnapKind::GridRow {
                row: r,
                left,
                right,
                ..
            } = &vl.kind
                && *r == row
            {
                let x = snap.grid_row_x(*left, *right)
                    + (col - left) as f32 * snap.grid_cell
                    + snap.grid_cell / 2.0;
                return egui::pos2(x, vl.y + snap.grid_cell / 2.0);
            }
        }
        panic!("no grid visual line for doc line {grid_doc_line} row {row}");
    }

    /// Absolute screen y just past the bottom edge of the last visual line —
    /// the empty band a document shorter than the viewport leaves behind.
    pub fn content_bottom(&self) -> f32 {
        let last = self
            .snap()
            .vlines
            .last()
            .expect("no visual lines were rendered");
        last.y + last.height
    }

    // -- queries ------------------------------------------------------------

    pub fn cursor(&self) -> Caret {
        self.state.cursor
    }

    /// Text content of a DocLine (panics on grids).
    pub fn text(&self, line: usize) -> &str {
        match &self.lines[line] {
            DocLine::Text(s) => s,
            other => panic!("doc line {line} is not text: {other:?}"),
        }
    }

    /// The pixel grid stored at a DocLine (panics if not a grid).
    pub fn grid(&self, line: usize) -> &PixelGrid {
        match &self.lines[line] {
            DocLine::Grid(g) => g,
            other => panic!("doc line {line} is not a grid: {other:?}"),
        }
    }

    /// Number of grid-row visual lines rendered for a grid DocLine.
    pub fn grid_row_count(&self, grid_doc_line: usize) -> usize {
        self.snap()
            .vlines
            .iter()
            .filter(|vl| {
                vl.doc_line == grid_doc_line && matches!(vl.kind, SnapKind::GridRow { .. })
            })
            .count()
    }

    /// The metric box the grid rows of `grid_doc_line` are drawn with, and the
    /// row range those rows span. `None` when the overlay is off.
    pub fn metrics_of(&self, grid_doc_line: usize) -> (Option<GlyphMetrics>, Vec<i16>) {
        let mut metrics = None;
        let mut rows = Vec::new();
        for vl in &self.snap().vlines {
            if vl.doc_line != grid_doc_line {
                continue;
            }
            if let SnapKind::GridRow {
                row, metrics: m, ..
            } = &vl.kind
            {
                metrics = *m;
                rows.push(*row);
            }
        }
        (metrics, rows)
    }

    /// Turn the metric overlay on (or off) and settle the view.
    pub fn set_show_metrics(&mut self, on: bool) {
        self.show_metrics = on;
        self.frame();
        self.frame();
    }

    /// Gutter number of the first visual line of a DocLine, as rendered.
    pub fn gutter_of(&self, doc_line: usize) -> Option<usize> {
        self.snap()
            .vlines
            .iter()
            .find(|vl| vl.doc_line == doc_line)
            .and_then(|vl| vl.gutter)
    }

    /// All rendered gutter numbers in visual order.
    pub fn gutter_numbers(&self) -> Vec<usize> {
        self.snap()
            .vlines
            .iter()
            .filter_map(|vl| vl.gutter)
            .collect()
    }

    /// The fold markers the last frame painted, by group header line.
    pub fn fold_markers(&self) -> Vec<FoldMarkerRect> {
        self.ctx
            .data(|d| d.get_temp::<Vec<FoldMarkerRect>>(fold_markers_id(self.state.id())))
            .unwrap_or_default()
    }

    /// The group whose marker the last frame drew shaded, if any.
    pub fn hovered_fold_marker(&self) -> Option<usize> {
        self.ctx
            .data(|d| d.get_temp::<Option<usize>>(fold_marker_hover_id(self.state.id())))
            .flatten()
    }

    /// Folds or unfolds the innermost group at `doc_line`, the way Ctrl/Cmd+.
    /// and the Edit-menu entry do — without going through the gutter's
    /// geometry, which is what `click_fold_marker` is for.
    pub fn toggle_fold(&mut self, doc_line: usize) {
        crate::editor::folding::toggle_at(&mut self.lines, &mut self.state, doc_line);
        self.frame();
        self.frame();
    }

    /// Clicks the fold marker of the group headed by `doc_line`.
    pub fn click_fold_marker(&mut self, doc_line: usize) {
        let rect = self
            .fold_markers()
            .into_iter()
            .find(|(header, ..)| *header == doc_line)
            .unwrap_or_else(|| panic!("no fold marker for line {doc_line}"))
            .1;
        self.click_at(rect.center());
    }

    /// Whether the editor canvas widget currently holds keyboard focus.
    pub fn editor_has_focus(&self) -> bool {
        let wid = self.snap().widget_id;
        self.ctx.memory(|m| m.has_focus(wid))
    }

    /// Scroll the document to an exact offset, the way a goto does, and settle
    /// one frame — so a test can put the page anywhere without walking the
    /// caret there.
    pub fn scroll_to(&mut self, y: f32) {
        let id = self.state.key(Slot::ScrollTarget);
        self.ctx.data_mut(|d| d.insert_temp(id, y));
        self.frame();
    }

    /// Current vertical scroll offset (pixels) as reported by egui.
    pub fn scroll_y(&self) -> f32 {
        self.scroll_y_of(&self.state)
    }

    /// The same, for whichever editor `state` belongs to.
    pub fn scroll_y_of(&self, state: &EditorState) -> f32 {
        self.ctx
            .data(|d| d.get_temp::<f32>(state.key(Slot::ScrollY)))
            .unwrap_or(0.0)
    }

    /// The layout snapshot published by whichever editor `state` belongs to.
    fn snapshot_of(&self, state: &EditorState) -> Option<Arc<ViewSnapshot>> {
        self.ctx
            .data(|d| d.get_temp::<Arc<ViewSnapshot>>(snapshot_id(state.id())))
    }

    /// Layout snapshot of the secondary pane created by
    /// [`EditorHarness::split`].
    pub fn second_snap(&self) -> Arc<ViewSnapshot> {
        let second = self
            .second
            .as_ref()
            .expect("no secondary pane; call split()");
        self.snapshot_of(&second.state)
            .expect("secondary pane published no snapshot")
    }
}
