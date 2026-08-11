//! Per-editor `egui` id namespaces.
//!
//! An editor is a widget, so several of them can be alive in one
//! `egui::Context` at once (two panes on one file, a split view, a test that
//! drives two documents in a single frame). Everything an editor parks in
//! `ctx.data()` between functions or between frames — scroll offsets, drag
//! accumulators, the caret's screen position, popup areas — therefore has to
//! be keyed by *which* editor owns it, not by a bare string that every
//! instance would collide on.
//!
//! [`EditorId`] is that key. It is allocated per [`EditorState`] and salts
//! every id the editor derives, so two editors never see each other's
//! scratch. [`Slot`] is the single inventory of what an editor stores: adding
//! a slot here is what makes it exist, and it is deliberately an enum rather
//! than a string so a typo cannot silently open a second, unread slot.
//!
//! Two things stay deliberately context-global and are *not* namespaced:
//!
//! - [`crate::editor::colors::Palette`], which is derived from the context's
//!   own theme and is identical for every editor in it.
//! - the coarse wheel debounce in [`super::document_view::debounced_scroll_step`],
//!   which describes the physical input device, not a view. Only the surface
//!   under the pointer consumes a step, so sharing it is what keeps two
//!   editors from both reacting to one wheel tick.
//!
//! [`EditorState`]: super::EditorState

use std::hash::Hash;
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes editor instances that would otherwise share `egui` ids.
///
/// Cheap (`Copy`) so it can be handed to the helpers that need to address the
/// owning editor's scratch but do not take the state itself.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EditorId(egui::Id);

/// Serial for [`EditorId::allocate`]. Editors come and go within a process
/// (documents open and close), and a reused id would inherit the previous
/// occupant's scroll offset and drag state, so ids are never recycled.
static NEXT_EDITOR_SERIAL: AtomicU64 = AtomicU64::new(0);

impl EditorId {
    /// A fresh id, distinct from every other one this process has handed out.
    pub fn allocate() -> Self {
        let serial = NEXT_EDITOR_SERIAL.fetch_add(1, Ordering::Relaxed);
        Self(egui::Id::new(("uniform_editor", serial)))
    }

    /// An id derived from a caller-chosen salt, for a host that wants its
    /// panes addressable by name rather than by allocation order.
    ///
    /// Two editors given the same salt *do* share their scratch, which is the
    /// point when the salt is a stable pane identity; pass distinct salts (or
    /// use [`EditorId::allocate`]) otherwise.
    // Part of the widget interface rather than of the current app, which
    // allocates its editors: a host that owns named panes needs this to give
    // a rebuilt pane the same namespace it had before.
    #[allow(dead_code)]
    pub fn from_salt(salt: impl Hash) -> Self {
        Self(egui::Id::new(("uniform_editor", salt)))
    }

    /// The bare `egui::Id`, for `ui.push_id` and for panels/areas that want
    /// the editor's own namespace root.
    pub fn egui_id(self) -> egui::Id {
        self.0
    }

    /// The id of one of this editor's scratch slots.
    pub(crate) fn key(self, slot: Slot) -> egui::Id {
        self.0.with(slot)
    }

    /// The id of a slot that exists once per sub-object (a grid block, a ref
    /// layer), keyed by that object's index.
    pub(crate) fn keyed(self, slot: Slot, extra: impl Hash) -> egui::Id {
        self.0.with(slot).with(extra)
    }
}

/// Every `egui`-side slot an editor instance owns.
///
/// Each variant is one addressable piece of per-editor state: either transient
/// data parked in `ctx.data()` or the id of a widget/area/panel the editor
/// creates. The comments say who writes and who reads, because most of these
/// exist purely to carry a value from one stage of the frame to another.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Slot {
    // -- vertical scrolling ------------------------------------------------
    /// Last frame's scroll offset; read at the top of the next frame, before
    /// the scroll area itself exists.
    ScrollY,
    /// Last frame's viewport height, read alongside `ScrollY`.
    ViewportH,
    /// A one-shot absolute scroll offset for this frame (goto, page scroll,
    /// caret-into-view), consumed by `resolve_scroll_target`.
    ScrollTarget,
    /// Whether the in-flight wheel gesture started over a scroll interceptor
    /// (a pixel grid, the subglyph preview, the shape palette).
    ScrollOnInterceptor,
    /// `(time, on_interceptor)` of the current gesture, so it keeps its
    /// starting zone while content moves under the pointer.
    ScrollGestureZone,
    /// `(last_tick_time, consecutive_ticks)` for wheel acceleration.
    ScrollAccel,
    /// Whether the wheel currently belongs to the Alt+wheel number gesture,
    /// so its still-draining delta must not reach the scroll area.
    ScrollSwallow,
    /// A pending PageUp/PageDown as `(direction, shift_held)`: written by key
    /// handling, consumed by `handle_page_scroll`.
    PageScrollRequest,
    /// Visual-line index the caret sticks to across repeated page scrolls.
    PageStickyVline,
    /// Caret position as of the last page scroll, to detect that the caret
    /// has since moved by other means and drop the sticky index.
    PageLastCursor,

    // -- glyph grid horizontal scrolling -----------------------------------
    /// Whether a grid scrollbar thumb is being dragged, which suppresses the
    /// grid's own pointer handling.
    GridHscrollDrag,
    /// Interaction id of one grid's horizontal scrollbar, keyed by item index.
    GridHscrollBar,

    // -- caret-anchored popups ---------------------------------------------
    /// Screen position of the caret, published by the paint pass for the
    /// popups that anchor to it.
    CursorScreenPos,
    /// Row height at the caret, published with `CursorScreenPos`.
    CursorRowHeight,
    /// `Option<(pos, message)>` for the error tooltip, published by the paint
    /// pass and consumed after the scroll area closes.
    ErrorTooltipData,
    /// Area id of the rename popup.
    RenamePopup,
    /// Area id of the autocomplete popup.
    AutocompletePopup,
    /// Area id of the Ctrl+K code point popup.
    CodepointPopup,
    /// Area id of the error tooltip.
    ErrorTooltip,

    // -- inline tool panel and pixel editing -------------------------------
    /// Whether the pointer is over the subglyph preview row.
    SubglyphPreviewHover,
    /// Whether the pointer is over the shape palette.
    ShapePaletteHover,
    /// Whether the subglyph context menu was opened from the grid itself
    /// rather than from a ref thumbnail.
    GridSubglyphCtxOnGrid,
    /// Last cell a slant shape was painted into, so dragging along a run of
    /// cells does not re-toggle the slant direction within one cell.
    SlantToggleLastCell,
    /// Latched at press time: did the in-flight pointer gesture start on the
    /// glyph grid itself? Painting follows only such a gesture.
    GridPaintGesture,
    /// Sub-cell remainder of an in-progress layer-move drag.
    LayerDragAccum,
    /// The in-progress pixel-selection drag (new selection, or moving one).
    PixelSelectDrag,
    /// Interaction id of one ref-layer thumbnail, keyed by
    /// `(edit_idx, ref_idx)`.
    RefLayerCtx,
    /// The edge a glyph-resize drag grabbed, latched at press time: the
    /// pointer leaves the edge as soon as the boundary follows it.
    ResizeDragSide,
    /// Sub-cell remainder of an in-progress glyph-resize drag.
    ResizeDragAccum,

    // -- sub-panels --------------------------------------------------------
    /// Id of the minimap side panel.
    MinimapPanel,

    // -- test instrumentation ----------------------------------------------
    /// `ViewSnapshot` of the frame's layout, published for `EditorHarness`.
    #[cfg(test)]
    TestViewSnapshot,
    /// Map of ref-thumbnail rects, published for `EditorHarness`.
    #[cfg(test)]
    TestRefRects,
    /// Map of shape-palette cell rects, published for `EditorHarness`.
    #[cfg(test)]
    TestPaletteRects,
    /// Color-token backgrounds painted this frame, published for
    /// `EditorHarness`.
    #[cfg(test)]
    TestColorSpans,
    /// The edit-mode border rect painted this frame, published for
    /// `EditorHarness`.
    #[cfg(test)]
    TestEditBorder,
    /// Rects of the resize mode's Apply/Cancel buttons, published for
    /// `EditorHarness`.
    #[cfg(test)]
    TestResizeButtons,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocated_ids_are_distinct_and_slots_do_not_alias() {
        let a = EditorId::allocate();
        let b = EditorId::allocate();
        assert_ne!(a, b);
        assert_ne!(a.key(Slot::ScrollY), b.key(Slot::ScrollY));
        assert_ne!(a.key(Slot::ScrollY), a.key(Slot::ViewportH));
        assert_ne!(
            a.keyed(Slot::GridHscrollBar, 0),
            a.keyed(Slot::GridHscrollBar, 1)
        );
    }

    #[test]
    fn equal_salts_share_a_namespace() {
        assert_eq!(EditorId::from_salt("left"), EditorId::from_salt("left"));
        assert_ne!(EditorId::from_salt("left"), EditorId::from_salt("right"));
    }
}
