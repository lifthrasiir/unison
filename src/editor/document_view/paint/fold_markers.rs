//! Fold markers: painting them and capturing the click that toggles one.

use super::super::*;

/// Draws the fold marker of every group with a row on screen, and resolves a
/// click on one.
///
/// A marker is an inverted plaque: a bar in the line-number colour, as tall as
/// what the group currently shows — one row while it is shut, unless its
/// header wraps — with a triangle cut out of its top in the page colour,
/// pointing down while the group is open and right while it is shut. The whole
/// bar is the target, not the triangle: hovering shades it and a click
/// anywhere on it toggles.
///
/// Returns `click_pos` with a position that landed on a marker removed, so the
/// caller's line hit tests never see it.
#[expect(clippy::too_many_arguments)]
pub(super) fn paint_fold_markers(
    ui: &egui::Ui,
    painter: &egui::Painter,
    lines: &mut Vec<DocLine>,
    state: &mut EditorState,
    markers: &[layout::FoldMarker],
    pal: &Palette,
    gutter: GutterLayout,
    gutter_x: f32,
    origin: egui::Pos2,
    click_pos: Option<egui::Pos2>,
    hover_pos: Option<egui::Pos2>,
    clicked: bool,
    needs_rederive: &mut bool,
) -> Option<egui::Pos2> {
    if gutter.marker_area() <= 0.0 {
        return click_pos;
    }
    let clip = ui.clip_rect();
    // A bar is as tall as its whole group, so an open one routinely runs on
    // past both edges of the viewport — where it is painted clipped away and
    // where a click, which comes through the widget's response, never reaches
    // it. The hover comes straight off the pointer instead, so it is the one
    // that has to be clipped by hand.
    let hover_pos = hover_pos.filter(|p| clip.contains(*p));
    let dark_mode = ui.visuals().dark_mode;
    let page = ui.visuals().panel_fill;
    let mut toggle: Option<usize> = None;
    let mut consumed = false;
    #[cfg(test)]
    let mut captured: Vec<crate::editor::harness::FoldMarkerRect> = Vec::new();
    #[cfg(test)]
    let mut captured_hover: Option<usize> = None;

    for marker in markers {
        let Some(cell) = gutter.marker_rect(
            gutter_x,
            marker.depth,
            origin.y + marker.y0,
            origin.y + marker.y1,
        ) else {
            continue;
        };
        if cell.max.y < clip.min.y || cell.min.y > clip.max.y || cell.height() <= 0.0 {
            continue;
        }

        let hovered = hover_pos.is_some_and(|p| cell.contains(p));
        #[cfg(test)]
        if hovered {
            captured_hover = Some(marker.group.header);
        }
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let fill = if hovered {
            shade_marker(pal.line_num, dark_mode)
        } else {
            pal.line_num
        };
        painter.rect_filled(cell, cell.width() * 0.25, fill);

        // The triangle sits in the square at the top of the bar, so a group
        // that spans a hundred rows and one that spans two look the same where
        // the eye goes.
        let w = cell.width();
        let half = w * 0.28;
        let cx = cell.center().x;
        let cy = cell.min.y + (w * 0.5).min(cell.height() * 0.5);
        let points = if marker.collapsed {
            vec![
                egui::pos2(cx - half * 0.8, cy - half),
                egui::pos2(cx - half * 0.8, cy + half),
                egui::pos2(cx + half * 0.8, cy),
            ]
        } else {
            vec![
                egui::pos2(cx - half, cy - half * 0.8),
                egui::pos2(cx + half, cy - half * 0.8),
                egui::pos2(cx, cy + half * 0.8),
            ]
        };
        painter.add(egui::Shape::convex_polygon(
            points,
            page,
            egui::Stroke::NONE,
        ));

        #[cfg(test)]
        captured.push((marker.group.header, cell, marker.collapsed));

        // Only a *click* is the bar's: a drag that merely passes over the
        // gutter belongs to the text selection it started in.
        if clicked && click_pos.is_some_and(|p| cell.contains(p)) {
            consumed = true;
            toggle = Some(marker.group.header);
        }
    }

    #[cfg(test)]
    crate::editor::harness::capture_fold_markers(ui.ctx(), state.id(), &captured);
    #[cfg(test)]
    crate::editor::harness::capture_fold_marker_hover(ui.ctx(), state.id(), captured_hover);

    if let Some(header) = toggle {
        *needs_rederive |= crate::editor::folding::toggle_at(lines, state, header);
    }
    if consumed { None } else { click_pos }
}

/// The hovered shade of a marker: brighter on a dark page, darker on a light
/// one, so the change reads the same either way.
fn shade_marker(c: egui::Color32, dark_mode: bool) -> egui::Color32 {
    let f = if dark_mode { 1.45 } else { 0.7 };
    let ch = |v: u8| (v as f32 * f).clamp(0.0, 255.0) as u8;
    egui::Color32::from_rgb(ch(c.r()), ch(c.g()), ch(c.b()))
}
