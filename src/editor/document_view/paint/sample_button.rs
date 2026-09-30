//! The *Use* button trailing a `sample` header: where it goes, what it hands over, how it is drawn.

use super::super::layout::VisualLine;
use super::super::*;
use super::colors::contrast_text_color;
use super::doc_line_text;

/// The gap between the end of a `sample` header and its *Use* button, and the
/// padding inside the button, both in the text font's own units so that the
/// button follows the zoom the rest of the line does.
const SAMPLE_USE_GAP: f32 = 1.0;
const SAMPLE_USE_PAD: f32 = 0.4;
const SAMPLE_USE_LABEL: &str = "Use";

/// The text of the [`sample`](crate::samples) whose header is `doc_line`, if
/// that is what the line is.
///
/// A sample's text is the item's, not the buffer's: the `||` lines have already
/// been dedented, and joining them here is what the preview is handed — read
/// through the line's [mode](crate::samples::SampleMode), so *Use* hands over
/// what the sample stands for and not the axes a `matrix` writes it as.
pub(super) fn sample_text_at(doc: &Document, doc_line: usize) -> Option<String> {
    let (mode, text) = sample_at(doc, doc_line)?;
    Some(
        crate::samples::SampleText {
            raw: text.join("\n"),
            mode: crate::samples::SampleMode::from_tokens(mode),
        }
        .expanded(),
    )
}

/// The `sample` item whose header is `doc_line`, if it carries a text: the
/// test the *Use* button is drawn by on every frame, which must not pay for
/// expanding a `matrix` it will not use.
fn sample_at(doc: &Document, doc_line: usize) -> Option<(&[String], &[String])> {
    let idx = line_to_item_idx(&doc.item_line_starts, doc_line)?;
    if doc.item_line_starts.get(idx) != Some(&doc_line) {
        return None;
    }
    match doc.items.get(idx) {
        Some(DocumentItem::Sample { mode, text, .. }) if !text.is_empty() => Some((mode, text)),
        _ => None,
    }
}

/// Where the *Use* button of a `sample` header goes on this visual line, or
/// `None` if the line is not one, carries no text, or is not the segment the
/// header *ends* on — a wrapped header puts the button after its last piece,
/// which is where the line ends on screen.
pub(super) fn sample_use_rect(
    doc: &Document,
    lines: &[DocLine],
    ui: &egui::Ui,
    font_id: &egui::FontId,
    atext: &AnnotatedText,
    vl: &VisualLine,
    line_rect: egui::Rect,
) -> Option<egui::Rect> {
    let segment = atext.text();
    let seg_len = segment.chars().count();
    if vl.col_offset + seg_len != doc_line_text(lines, vl, segment).chars().count() {
        return None;
    }
    sample_at(doc, vl.doc_line)?;
    let end = atext.x_pos(ui, font_id, seg_len);
    let label_w = ui.fonts(|f| {
        f.layout_no_wrap(
            SAMPLE_USE_LABEL.to_string(),
            font_id.clone(),
            egui::Color32::WHITE,
        )
        .rect
        .width()
    });
    let space = font_id.size * SAMPLE_USE_GAP;
    let pad = font_id.size * SAMPLE_USE_PAD;
    Some(egui::Rect::from_min_size(
        egui::pos2(line_rect.min.x + end + space, line_rect.min.y + 1.0),
        egui::vec2(label_w + pad * 2.0, (line_rect.height() - 2.0).max(1.0)),
    ))
}

/// The button itself: an outline that fills in under the pointer, so that it
/// reads as a control rather than as more of the line it trails.
pub(super) fn paint_sample_use_button(
    painter: &egui::Painter,
    ui: &egui::Ui,
    font_id: &egui::FontId,
    rect: egui::Rect,
    pal: &Palette,
    hovered: bool,
) {
    let accent = pal.link;
    let radius = rect.height() * 0.3;
    if hovered {
        painter.rect_filled(rect, radius, accent);
    }
    painter.rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0, accent),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        SAMPLE_USE_LABEL,
        font_id.clone(),
        if hovered {
            contrast_text_color(accent)
        } else {
            accent
        },
    );
    let _ = ui;
}
