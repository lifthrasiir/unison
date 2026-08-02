//! Zoom and preview font size: the levels, which surface a gesture targets,
//! and the wheel/key handlers.

use super::*;

/// Bounds of the editor's integral zoom level.
pub(super) const MIN_ZOOM_LEVEL: u32 = 1;

pub(super) const MAX_ZOOM_LEVEL: u32 = 8;

/// Moves `level` by `delta` steps, saturating at the zoom bounds.
pub(super) fn zoom_step(level: u32, delta: i32) -> u32 {
    (level as i32 + delta).clamp(MIN_ZOOM_LEVEL as i32, MAX_ZOOM_LEVEL as i32) as u32
}

/// Bounds, step and default of the shaped preview's font size, in pixels.
/// These match the slider and drag value in the preview tab's toolbar.
pub(super) const MIN_PREVIEW_FONT_SIZE: f32 = 16.0;

pub(super) const MAX_PREVIEW_FONT_SIZE: f32 = 128.0;

const PREVIEW_FONT_SIZE_STEP: f32 = 16.0;

pub(super) const DEFAULT_PREVIEW_FONT_SIZE: f32 = 32.0;

/// Moves `size` one step along the `PREVIEW_FONT_SIZE_STEP` grid in `delta`'s
/// direction, saturating at the bounds. The drag value admits off-grid sizes,
/// so a step first snaps onto the grid rather than carrying the offset along.
pub(super) fn preview_font_step(size: f32, delta: i32) -> f32 {
    let stepped = match delta.signum() {
        1 => ((size / PREVIEW_FONT_SIZE_STEP).floor() + 1.0) * PREVIEW_FONT_SIZE_STEP,
        -1 => ((size / PREVIEW_FONT_SIZE_STEP).ceil() - 1.0) * PREVIEW_FONT_SIZE_STEP,
        _ => size,
    };
    stepped.clamp(MIN_PREVIEW_FONT_SIZE, MAX_PREVIEW_FONT_SIZE)
}

/// Which surface a zoom gesture applies to. Zoom is not one global setting:
/// *each editor pane* has its own integral zoom level and the shaped preview
/// has its font size, so every zoom gesture picks a target first — the focused
/// surface for the keyboard chords, the hovered one for Cmd/Ctrl + wheel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ZoomTarget {
    /// The editor pane at this index.
    Editor(usize),
    Preview,
    /// Neither surface, so the zoom commands do nothing and the corresponding
    /// menu entries are disabled.
    None,
}

/// Picks the zoom target under `pointer`. A pane rect is `None` when that pane
/// shows the no-document placeholder, which is not an editor; the preview rect
/// is `None` while its tab is hidden.
fn zoom_target_at(
    pointer: Option<egui::Pos2>,
    pane_rects: &[Option<egui::Rect>],
    preview_rect: Option<egui::Rect>,
) -> ZoomTarget {
    let Some(pos) = pointer else {
        return ZoomTarget::None;
    };
    if preview_rect.is_some_and(|r| r.contains(pos)) {
        return ZoomTarget::Preview;
    }
    match pane_rects
        .iter()
        .position(|r| r.is_some_and(|r| r.contains(pos)))
    {
        Some(idx) => ZoomTarget::Editor(idx),
        None => ZoomTarget::None,
    }
}

impl UniformApp {
    /// The focused pane's zoom level, which is what the View menu's entries
    /// and the status bar report.
    pub(super) fn focused_zoom_level(&self) -> u32 {
        self.panes.focused().zoom_level
    }

    /// Sets pane `pane_idx`'s zoom level (clamped to
    /// [`MIN_ZOOM_LEVEL`]..=[`MAX_ZOOM_LEVEL`]) and lets the document in it
    /// recenter its scroll. Returns whether the level actually moved.
    pub(super) fn set_pane_zoom_level(&mut self, pane_idx: usize, level: u32) -> bool {
        let level = level.clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
        let Some(pane) = self.panes.get_mut(pane_idx) else {
            return false;
        };
        let old_zoom = pane.zoom_level;
        if level == old_zoom {
            return false;
        }
        pane.zoom_level = level;
        if let Some(doc) = self.pane_doc_mut(pane_idx) {
            doc.editor_state.notify_zoom_change(old_zoom);
        }
        true
    }

    /// Sets the shaped preview's font size (clamped to the slider's range) and
    /// keeps the snapped slider position in sync. Returns whether it moved.
    pub(super) fn set_preview_font_size(&mut self, size: f32) -> bool {
        let size = size.clamp(MIN_PREVIEW_FONT_SIZE, MAX_PREVIEW_FONT_SIZE);
        if size == self.preview_font_size {
            return false;
        }
        self.preview_font_size = size;
        self.preview_font_size_slider =
            (size / PREVIEW_FONT_SIZE_STEP).round() * PREVIEW_FONT_SIZE_STEP;
        true
    }

    /// The surface the zoom *keyboard* chords act on: whichever of the focused
    /// editor pane and the shaped preview holds the focus. The preview wins
    /// the same way it does for the edit menu.
    pub(super) fn focused_zoom_target(&self) -> ZoomTarget {
        if self.bottom_panel_tab == Some(0) && self.shaped_preview.is_focused() {
            ZoomTarget::Preview
        } else if self
            .active_doc()
            .is_some_and(|d| d.editor_state.is_active())
        {
            ZoomTarget::Editor(self.panes.focus())
        } else {
            ZoomTarget::None
        }
    }

    /// Cmd/Ctrl + scroll wheel to zoom. This one ignores the focus and goes by
    /// what the pointer is over instead, so the surface being pointed at is the
    /// one that zooms. Skipped over the editing grid, where Ctrl+scroll already
    /// cycles layers.
    pub(super) fn handle_zoom_scroll(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.modifiers.command) {
            return;
        }
        let pane_rects: Vec<Option<egui::Rect>> = self.panes.iter().map(|p| p.view_rect).collect();
        let target = zoom_target_at(
            ctx.input(|i| i.pointer.latest_pos()),
            &pane_rects,
            self.preview_view_rect,
        );
        // Over a pixel grid Ctrl+scroll already cycles layers, so that pane's
        // own hover flag — not the focused pane's — is what suppresses zoom.
        let grid_hover = matches!(target, ZoomTarget::Editor(idx) if self
            .panes
            .get(idx)
            .and_then(|p| p.doc_idx)
            .and_then(|d| self.open_documents.get(d))
            .is_some_and(|d| d.editor_state.is_grid_hover()));
        if target == ZoomTarget::None || grid_hover {
            return;
        }
        if let Some(step) = debounced_scroll_step(ctx) {
            let delta = if step < 0 { 1 } else { -1 };
            match target {
                ZoomTarget::Editor(idx) => {
                    let level = self.panes.get(idx).map_or(1, |p| p.zoom_level);
                    self.set_pane_zoom_level(idx, zoom_step(level, delta));
                }
                ZoomTarget::Preview => {
                    let size = preview_font_step(self.preview_font_size, delta);
                    self.set_preview_font_size(size);
                }
                ZoomTarget::None => {}
            }
            ctx.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
        }
    }

    /// Cmd/Ctrl + `-` / `=` / `0` to zoom the focused surface. egui's built-in
    /// `zoom_with_keyboard` (which scales `pixels_per_point` instead) is disabled in
    /// [`UniformApp::new`] so these are the only handlers for those chords.
    pub(super) fn handle_zoom_keys(&mut self, ctx: &egui::Context) {
        let (zoom_in, zoom_out, zoom_reset) = ctx.input(|i| {
            if !i.modifiers.command {
                return (false, false, false);
            }
            (
                // `+` is Shift+`=` on most layouts; accept either, shifted or not.
                i.key_pressed(egui::Key::Equals) || i.key_pressed(egui::Key::Plus),
                i.key_pressed(egui::Key::Minus),
                i.key_pressed(egui::Key::Num0),
            )
        });
        match self.focused_zoom_target() {
            ZoomTarget::Editor(idx) => {
                let level = self.panes.get(idx).map_or(1, |p| p.zoom_level);
                if zoom_reset {
                    self.set_pane_zoom_level(idx, 1);
                } else if zoom_in {
                    self.set_pane_zoom_level(idx, zoom_step(level, 1));
                } else if zoom_out {
                    self.set_pane_zoom_level(idx, zoom_step(level, -1));
                }
            }
            ZoomTarget::Preview => {
                if zoom_reset {
                    self.set_preview_font_size(DEFAULT_PREVIEW_FONT_SIZE);
                } else if zoom_in {
                    let size = preview_font_step(self.preview_font_size, 1);
                    self.set_preview_font_size(size);
                } else if zoom_out {
                    let size = preview_font_step(self.preview_font_size, -1);
                    self.set_preview_font_size(size);
                }
            }
            ZoomTarget::None => {}
        }
    }
}

#[cfg(test)]
mod zoom_tests {
    use super::*;

    #[test]
    fn zoom_step_saturates_at_the_bounds() {
        assert_eq!(zoom_step(1, 1), 2);
        assert_eq!(zoom_step(2, -1), 1);
        // At either end the step is a no-op rather than wrapping or panicking.
        assert_eq!(zoom_step(MIN_ZOOM_LEVEL, -1), MIN_ZOOM_LEVEL);
        assert_eq!(zoom_step(MAX_ZOOM_LEVEL, 1), MAX_ZOOM_LEVEL);
    }

    #[test]
    fn preview_font_step_walks_the_16px_grid_and_clamps() {
        assert_eq!(preview_font_step(32.0, 1), 48.0);
        assert_eq!(preview_font_step(32.0, -1), 16.0);
        // The drag value admits off-grid sizes; a step snaps back onto the grid.
        assert_eq!(preview_font_step(20.0, 1), 32.0);
        assert_eq!(preview_font_step(20.0, -1), 16.0);
        assert_eq!(
            preview_font_step(MIN_PREVIEW_FONT_SIZE, -1),
            MIN_PREVIEW_FONT_SIZE
        );
        assert_eq!(
            preview_font_step(MAX_PREVIEW_FONT_SIZE, 1),
            MAX_PREVIEW_FONT_SIZE
        );
        // Below the minimum a decrement must not fall through to zero.
        assert_eq!(preview_font_step(8.0, -1), MIN_PREVIEW_FONT_SIZE);
    }

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
    }

    #[test]
    fn zoom_target_at_picks_the_hovered_surface() {
        let editor = [Some(rect(0.0, 0.0, 100.0, 100.0))];
        let preview = Some(rect(0.0, 100.0, 100.0, 150.0));
        assert_eq!(
            zoom_target_at(Some(egui::pos2(50.0, 50.0)), &editor, preview),
            ZoomTarget::Editor(0)
        );
        assert_eq!(
            zoom_target_at(Some(egui::pos2(50.0, 120.0)), &editor, preview),
            ZoomTarget::Preview
        );
        // Outside both (the sidebar, the menu bar) nothing zooms.
        assert_eq!(
            zoom_target_at(Some(egui::pos2(500.0, 50.0)), &editor, preview),
            ZoomTarget::None
        );
        assert_eq!(zoom_target_at(None, &editor, preview), ZoomTarget::None);
        // The placeholder panel shown with no open document is not an editor.
        assert_eq!(
            zoom_target_at(Some(egui::pos2(50.0, 50.0)), &[None], preview),
            ZoomTarget::None
        );
    }

    #[test]
    fn zoom_target_at_distinguishes_the_two_panes() {
        // Split panes: the wheel zooms whichever one the pointer is over,
        // and a placeholder pane beside a real one still zooms nothing.
        let panes = [
            Some(rect(0.0, 0.0, 100.0, 100.0)),
            Some(rect(104.0, 0.0, 200.0, 100.0)),
        ];
        assert_eq!(
            zoom_target_at(Some(egui::pos2(50.0, 50.0)), &panes, None),
            ZoomTarget::Editor(0)
        );
        assert_eq!(
            zoom_target_at(Some(egui::pos2(150.0, 50.0)), &panes, None),
            ZoomTarget::Editor(1)
        );
        // The divider between them belongs to neither pane.
        assert_eq!(
            zoom_target_at(Some(egui::pos2(102.0, 50.0)), &panes, None),
            ZoomTarget::None
        );
        let with_placeholder = [Some(rect(0.0, 0.0, 100.0, 100.0)), None];
        assert_eq!(
            zoom_target_at(Some(egui::pos2(150.0, 50.0)), &with_placeholder, None),
            ZoomTarget::None
        );
    }
}
