//! Color swatches behind color tokens, and the display-color helpers they share.

use super::super::*;

pub(super) fn resolve_color_for_display(
    token: &str,
    aliases: &ColorAliasMap,
) -> Option<egui::Color32> {
    if token == "fg" {
        return None;
    }
    if token.starts_with('#') {
        let rgba = crate::render::ttf_builder::parse_hex_color(token)?;
        return Some(egui::Color32::from_rgba_unmultiplied(
            rgba.r, rgba.g, rgba.b, rgba.a,
        ));
    }
    let (rgba, _) = aliases.get(token)?;
    Some(egui::Color32::from_rgba_unmultiplied(
        rgba.r, rgba.g, rgba.b, rgba.a,
    ))
}

pub(super) fn contrast_text_color(bg: egui::Color32) -> egui::Color32 {
    let [r, g, b, _] = bg.to_array();
    let luma = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    if luma > 128.0 {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    }
}

/// Paints the color swatches `line` calls for onto the segment `atext` draws,
/// and returns the spans it painted in absolute document columns. A swatch
/// whose token the wrap put on another segment belongs to that segment.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_color_backgrounds(
    painter: &egui::Painter,
    ui: &egui::Ui,
    font_id: &egui::FontId,
    atext: &AnnotatedText<'_>,
    line: &str,
    col_offset: usize,
    base_x: f32,
    base_y: f32,
    row_h: f32,
    aliases: &ColorAliasMap,
) -> Vec<(usize, usize)> {
    let text = atext.text();
    let trimmed = line.trim_start();
    let leading = line.chars().count() - trimmed.chars().count();
    let spans = match tokenize_with_spans(trimmed) {
        Ok(s) if !s.is_empty() => s,
        _ => return Vec::new(),
    };
    let keyword = spans[0].value.as_str();
    let rest = &spans[1..];

    let mut color_spans: Vec<(usize, usize, egui::Color32)> = Vec::new();

    match keyword {
        "color" => {
            if rest.len() >= 3 && rest[1].value == "=" {
                let val_span = &rest[2];
                if let Some(color) = resolve_color_for_display(&val_span.value, aliases) {
                    color_spans.push((
                        leading + val_span.raw_start,
                        leading + val_span.raw_end,
                        color,
                    ));
                }
            }
        }
        "ref" => {
            if let Some(fill_pos) = rest.iter().position(|s| s.value == "fill")
                && let Some(color_span) = rest.get(fill_pos + 1)
                && let Some(color) = resolve_color_for_display(&color_span.value, aliases)
            {
                color_spans.push((
                    leading + color_span.raw_start,
                    leading + color_span.raw_end,
                    color,
                ));
            }
        }
        _ => {}
    }

    let seg_len = text.chars().count();
    let mut painted = Vec::new();
    for (col_start, col_end, bg_color) in &color_spans {
        // Clipped to this segment: a token the wrap put wholly on another one
        // has nothing to draw here.
        let adj_start = (*col_start).clamp(col_offset, col_offset + seg_len) - col_offset;
        let adj_end = (*col_end).clamp(col_offset, col_offset + seg_len) - col_offset;
        if adj_start >= adj_end {
            continue;
        }
        painted.push((col_offset + adj_start, col_offset + adj_end));
        let x0 = base_x + atext.x_pos(ui, font_id, adj_start);
        let x1 = base_x + atext.x_pos(ui, font_id, adj_end);
        let rect = egui::Rect::from_min_size(egui::pos2(x0, base_y), egui::vec2(x1 - x0, row_h));
        painter.rect_filled(rect, 0.0, *bg_color);
        let token_text: String = text
            .chars()
            .skip(adj_start)
            .take(adj_end - adj_start)
            .collect();
        let fg = contrast_text_color(*bg_color);
        painter.text(
            egui::pos2(x0, base_y),
            egui::Align2::LEFT_TOP,
            &token_text,
            font_id.clone(),
            fg,
        );
    }
    painted
}
