use std::collections::HashMap;

use crate::document::{
    DocLine, Document, DocumentItem, GlyphBody, GlyphRef, NamePartsMap, is_name_pattern,
};
use crate::pattern::NamePattern;
use crate::editor::annotations::{self, InlineAnnotation};
use crate::editor::colors::Palette;
use crate::editor::doc_links;
use crate::editor::ref_composite::{self, GlyphComposite, ResolvedGlyph};

use super::document_view::{
    GRID_CELL, GlyphMetrics, GridExtent, INLINE_PALETTE_CELL, PREVIEW_SCALE, VLineKind, VisualLine,
    compute_grid_display_extent, glyph_metrics,
};

pub(crate) fn preview_max_height(
    body: &GlyphBody,
    composite: Option<&GlyphComposite>,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
) -> u16 {
    let mut max_h: u16 = 0;
    // The composite and the own grid are in this glyph's own `scale N`
    // subcells; the preview row is measured in logical pixels.
    let own_scale = body.scale.max(1) as u16;
    if let Some(comp) = composite {
        max_h = max_h.max(comp.height / own_scale);
    } else if let Some(grid) = &body.pixels {
        max_h = max_h.max(grid.height / own_scale);
    }
    for gref in &body.refs {
        if let Some(resolved) =
            ref_composite::resolve_ref_name_with_parts(&gref.name, named_glyphs, name_parts)
        {
            // Logical height: a `scale N` glyph's grid counts subcells, and the
            // preview row is measured in logical pixels like every other height
            // here (see the thumbnail sizing in `inline_tools.rs`).
            max_h = max_h.max(resolved.grid.height / resolved.scale.max(1) as u16);
        }
    }
    max_h
}

pub(crate) fn preview_row_height(zoom_level: u32, max_preview_h: u16) -> f32 {
    let grid_cell = GRID_CELL * zoom_level as f32;
    let zoom = zoom_level as f32;
    (max_preview_h as f32 * PREVIEW_SCALE * zoom).max(2.0 * grid_cell)
}

pub(crate) fn min_grid_rows_for_panel(zoom_level: u32, max_preview_h: u16) -> i16 {
    let grid_cell = GRID_CELL * zoom_level as f32;
    let zoom = zoom_level as f32;
    let palette_cell = INLINE_PALETTE_CELL * zoom;
    let palette_rows = crate::editor::glyph_widget::palette_rows();
    let palette_height = palette_rows as f32 * palette_cell;
    let prh = preview_row_height(zoom_level, max_preview_h);
    let panel_height = prh + 4.0 + palette_height;
    (panel_height / grid_cell).ceil() as i16
}

fn compute_wrap_segments(
    text: &str,
    annotations: &[InlineAnnotation],
    wrap_width: Option<f32>,
    ctx: &egui::Context,
    font_id: &egui::FontId,
) -> Vec<(String, usize)> {
    let max_w = match wrap_width {
        Some(w) if w > 0.0 => w,
        _ => return vec![(text.to_string(), 0)],
    };

    // Widths are measured on the *rendered* line, annotations included, so a
    // line only wraps where it visually overflows.
    let width = |from: usize, to: usize| -> f32 {
        let mut s = String::new();
        let mut ai = 0usize;
        for (i, c) in text.chars().enumerate() {
            if i >= to {
                break;
            }
            if i >= from {
                s.push(c);
            }
            while let Some(a) = annotations.get(ai) {
                if a.col <= i + 1 {
                    if i >= from {
                        s.push_str(&a.text);
                    }
                    ai += 1;
                } else {
                    break;
                }
            }
        }
        if s.is_empty() {
            return 0.0;
        }
        ctx.fonts(|f| {
            f.layout_no_wrap(s, font_id.clone(), egui::Color32::WHITE)
                .rect
                .width()
        })
    };

    let char_count = text.chars().count();
    if width(0, char_count) <= max_w {
        return vec![(text.to_string(), 0)];
    }

    let chars: Vec<char> = text.chars().collect();
    let mut result = Vec::new();
    let mut start = 0;

    while start < chars.len() {
        if width(start, chars.len()) <= max_w {
            result.push((chars[start..].iter().collect(), start));
            break;
        }

        let mut lo = 1usize;
        let mut hi = chars.len() - start;
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if width(start, start + mid) <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let end = start + lo.max(1);
        result.push((chars[start..end].iter().collect(), start));
        start = end;
    }

    if result.is_empty() {
        result.push((text.to_string(), 0));
    }
    result
}

/// The annotations falling in the wrapped segment `col_offset..col_offset +
/// seg_len`, rebased onto it. An annotation trails the character at `col - 1`,
/// so it stays on the segment holding that character and never straddles a
/// soft wrap.
fn segment_annotations(
    annotations: &[InlineAnnotation],
    col_offset: usize,
    seg_len: usize,
) -> Vec<InlineAnnotation> {
    annotations
        .iter()
        .filter(|a| a.col > col_offset && a.col <= col_offset + seg_len)
        .map(|a| InlineAnnotation {
            col: a.col - col_offset,
            text: a.text.clone(),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn push_wrapped_text_vlines(
    vlines: &mut Vec<VisualLine>,
    text: &str,
    doc_line: usize,
    color: egui::Color32,
    error_spans: Vec<(usize, usize, String)>,
    wrap_width: Option<f32>,
    ctx: &egui::Context,
    font_id: &egui::FontId,
) {
    let annotations = annotations::line_annotations(text);
    // A `// …` comment is a comment wherever the line's own color comes from.
    let comment_col = crate::document_io::split_comment(text)
        .1
        .map(|c| text.chars().count() - c.chars().count());
    let segments = compute_wrap_segments(text, &annotations, wrap_width, ctx, font_id);
    for (seg_text, col_offset) in segments {
        let seg_len = seg_text.chars().count();
        let seg_annotations = segment_annotations(&annotations, col_offset, seg_len);
        let seg_errors: Vec<(usize, usize, String)> = error_spans
            .iter()
            .filter_map(|(s, e, msg)| {
                let adj_s = (*s).max(col_offset).saturating_sub(col_offset);
                let adj_e = (*e).min(col_offset + seg_len).saturating_sub(col_offset);
                if adj_s < adj_e {
                    Some((adj_s, adj_e, msg.clone()))
                } else {
                    None
                }
            })
            .collect();
        let seg_comment_col = comment_col
            .filter(|c| *c < col_offset + seg_len)
            .map(|c| c.saturating_sub(col_offset));
        vlines.push(VisualLine {
            doc_line,
            kind: VLineKind::Text(seg_text),
            color,
            error_spans: seg_errors,
            col_offset,
            annotations: seg_annotations,
            comment_col: seg_comment_col,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn push_grid_vlines(
    vlines: &mut Vec<VisualLine>,
    item_idx: usize,
    grid_doc_line: usize,
    own_w: u16,
    own_h: u16,
    mut extent: GridExtent,
    metrics: Option<GlyphMetrics>,
    is_editing: bool,
    default_color: egui::Color32,
    zoom_level: u32,
    max_preview_h: u16,
) {
    if is_editing {
        let min_rows = min_grid_rows_for_panel(zoom_level, max_preview_h);
        let rows = extent.bottom - extent.top;
        if rows < min_rows {
            extent.bottom = extent.top + min_rows;
        }
    }
    for row in extent.top..extent.bottom {
        vlines.push(VisualLine {
            doc_line: grid_doc_line,
            kind: VLineKind::GridRow {
                item_idx,
                row,
                own_width: own_w,
                own_height: own_h,
                grid_doc_line,
                extent,
                metrics,
            },
            color: default_color,
            error_spans: Vec::new(),
            col_offset: 0,
            annotations: Vec::new(),
            comment_col: None,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn build_ref_vlines(
    lines: &[DocLine],
    refs: &[GlyphRef],
    cur: &mut usize,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    color_for_text: &dyn Fn(&str) -> egui::Color32,
    wrap_width: Option<f32>,
    ctx: &egui::Context,
    font_id: &egui::FontId,
) -> Vec<VisualLine> {
    let mut ref_vlines = Vec::new();
    for r in refs {
        if let Some(DocLine::Text(s)) = lines.get(*cur) {
            let mut error_spans = Vec::new();
            if !ref_composite::is_ref_valid(&r.name, named_glyphs, name_parts)
                && let Some(range) = doc_links::find_name_col_range_after_prefix(s, "ref ")
            {
                error_spans.push((range.0, range.1, format!("undefined glyph: {}", r.name)));
            }
            push_wrapped_text_vlines(
                &mut ref_vlines,
                s,
                *cur,
                color_for_text(s),
                error_spans,
                wrap_width,
                ctx,
                font_id,
            );
        }
        *cur += 1;
    }
    ref_vlines
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_visual_lines(
    lines: &[DocLine],
    doc: &Document,
    item_line_starts: &[usize],
    composites: &HashMap<usize, GlyphComposite>,
    named_glyphs: &HashMap<String, ResolvedGlyph>,
    name_parts: &NamePartsMap,
    editing_item_idx: Option<usize>,
    zoom_level: u32,
    pal: &Palette,
    wrap_width: Option<f32>,
    ctx: &egui::Context,
    font_id: &egui::FontId,
    meta: crate::meta::FontMetrics,
    show_metrics: bool,
    shadow: Option<&(usize, crate::editor::anchor_shadow::AnchorShadow)>,
) -> Vec<VisualLine> {
    let comment_color = pal.text_comment;
    let meta_color = pal.text_meta;
    let header_color = pal.text_header;
    let ref_color = pal.text_ref;
    let directive_color = pal.text_directive;
    let directive2_color = pal.text_directive2;
    let default_color = pal.text_default;

    let color_for_text = |s: &str| -> egui::Color32 {
        let trimmed = s.trim_start();
        if trimmed.starts_with("//") {
            comment_color
        } else if trimmed.starts_with("glyph ") || trimmed.starts_with("map ") {
            header_color
        } else if trimmed.starts_with("ref ") || trimmed.starts_with("anchor ") {
            ref_color
        } else if trimmed.starts_with("meta ")
            || trimmed.starts_with("face ")
            || trimmed.starts_with("slice ")
        {
            meta_color
        } else if trimmed.starts_with("exclude-from-sample ")
            || trimmed.starts_with("assume unused ")
            || trimmed.starts_with("assert ")
        {
            directive_color
        } else if trimmed.starts_with("name-parts ")
            || trimmed.starts_with("remap ")
            || trimmed.starts_with("feature ")
        {
            directive2_color
        } else {
            default_color
        }
    };

    let mut vlines: Vec<VisualLine> = Vec::new();
    let mut line_idx = 0usize;

    for (item_idx, item) in doc.items.iter().enumerate() {
        let item_start = if item_idx < item_line_starts.len() {
            item_line_starts[item_idx]
        } else {
            line_idx
        };
        while line_idx < item_start {
            if let Some(DocLine::Text(s)) = lines.get(line_idx) {
                push_wrapped_text_vlines(
                    &mut vlines,
                    s,
                    line_idx,
                    color_for_text(s),
                    Vec::new(),
                    wrap_width,
                    ctx,
                    font_id,
                );
            }
            line_idx += 1;
        }
        match item {
            DocumentItem::BlankLine
            | DocumentItem::Comment(_)
            | DocumentItem::Directive(_)
            | DocumentItem::Face { .. }
            | DocumentItem::Slice { .. }
            | DocumentItem::Meta(_)
            | DocumentItem::Map { .. }
            | DocumentItem::NameParts { .. }
            | DocumentItem::Remap { .. }
            | DocumentItem::RemapGroup { .. }
            | DocumentItem::Feature { .. }
            | DocumentItem::FeatureAnchor { .. }
            | DocumentItem::MapDecomposed { .. }
            | DocumentItem::Color { .. }
            | DocumentItem::AssertShape { .. }
            | DocumentItem::AssertSame { .. }
            | DocumentItem::AssertDistinct { .. } => {
                if let Some(DocLine::Text(s)) = lines.get(item_start) {
                    push_wrapped_text_vlines(
                        &mut vlines,
                        s,
                        item_start,
                        color_for_text(s),
                        Vec::new(),
                        wrap_width,
                        ctx,
                        font_id,
                    );
                }
                line_idx = item_start + 1;
            }
            DocumentItem::Glyph { name, body } => {
                // Header line
                let header_line = item_start;
                let is_alias = body.is_simple_alias()
                    && matches!(
                        lines.get(header_line),
                        Some(DocLine::Text(s)) if {
                            let tokens = crate::document_io::tokenize_tokens(s.trim())
                                .unwrap_or_default();
                            tokens.len() >= 4
                                && tokens[0] == "glyph"
                                && tokens[2..].iter().any(|t| t == "=")
                        }
                    );

                if let Some(DocLine::Text(s)) = lines.get(header_line) {
                    let mut error_spans = Vec::new();

                    let name_str = name.display();
                    if is_name_pattern(&name_str)
                        && let Err(e) = NamePattern::parse(&name_str)
                        && let Some(range) =
                            doc_links::find_name_col_range_after_prefix(s, "glyph ")
                    {
                        error_spans.push((range.0, range.1, e.to_string()));
                    }

                    if is_alias
                        && !ref_composite::is_ref_valid(
                            &body.refs[0].name,
                            named_glyphs,
                            name_parts,
                        )
                        && let Some(eq_byte) = s.trim().find(" = ")
                    {
                        let trimmed = s.trim_start();
                        let leading_c = s.chars().count() - trimmed.chars().count();
                        let eq_prefix_c = trimmed[..eq_byte + 3].chars().count();
                        let alias_c = trimmed[eq_byte + 3..]
                            .split_whitespace()
                            .next()
                            .map_or(0, |t| t.chars().count());
                        if alias_c > 0 {
                            let cs = leading_c + eq_prefix_c;
                            error_spans.push((
                                cs,
                                cs + alias_c,
                                format!("undefined glyph: {}", body.refs[0].name),
                            ));
                        }
                    }

                    push_wrapped_text_vlines(
                        &mut vlines,
                        s,
                        header_line,
                        color_for_text(s),
                        error_spans,
                        wrap_width,
                        ctx,
                        font_id,
                    );
                }

                let mut cur = header_line + 1;
                let skip_grid = is_alias;

                let is_editing = editing_item_idx == Some(item_idx);
                // Only the glyph the anchor belongs to makes room for it.
                let shadow = shadow.filter(|(idx, _)| *idx == item_idx).map(|(_, s)| s);
                let max_ph = if is_editing {
                    preview_max_height(body, composites.get(&item_idx), named_glyphs, name_parts)
                } else {
                    0
                };

                // Pixel rows
                if let Some(grid) = &body.pixels {
                    let grid_doc_line = cur;
                    let (own_w, own_h, mut extent) =
                        compute_grid_display_extent(Some(grid), composites.get(&item_idx), &body.points);
                    if let Some(s) = shadow {
                        extent.include_shadow(s);
                    }
                    let metrics = show_metrics.then(|| {
                        let m = glyph_metrics(
                            body,
                            composites.get(&item_idx),
                            own_w,
                            own_h,
                            meta,
                        );
                        extent.include_metrics(&m);
                        m
                    });
                    push_grid_vlines(
                        &mut vlines,
                        item_idx,
                        grid_doc_line,
                        own_w,
                        own_h,
                        extent,
                        metrics,
                        is_editing,
                        default_color,
                        zoom_level,
                        max_ph,
                    );
                    cur += 1; // Grid is one DocLine
                }

                let has_composite = composites.contains_key(&item_idx);

                // Ref-only glyph with composite
                if !skip_grid && body.pixels.is_none() && has_composite {
                    let comp = &composites[&item_idx];

                    let first_ref_line = cur;
                    let ref_vlines = build_ref_vlines(
                        lines,
                        &body.refs,
                        &mut cur,
                        named_glyphs,
                        name_parts,
                        &color_for_text,
                        wrap_width,
                        ctx,
                        font_id,
                    );

                    let (own_w, own_h, mut extent) =
                        compute_grid_display_extent(None, Some(comp), &body.points);
                    if let Some(s) = shadow {
                        extent.include_shadow(s);
                    }
                    let metrics = show_metrics.then(|| {
                        let m = glyph_metrics(
                            body,
                            Some(comp),
                            own_w,
                            own_h,
                            meta,
                        );
                        extent.include_metrics(&m);
                        m
                    });
                    push_grid_vlines(
                        &mut vlines,
                        item_idx,
                        first_ref_line,
                        own_w,
                        own_h,
                        extent,
                        metrics,
                        is_editing,
                        default_color,
                        zoom_level,
                        max_ph,
                    );
                    vlines.extend(ref_vlines);
                } else if !is_alias {
                    let mut ref_vlines = build_ref_vlines(
                        lines,
                        &body.refs,
                        &mut cur,
                        named_glyphs,
                        name_parts,
                        &color_for_text,
                        wrap_width,
                        ctx,
                        font_id,
                    );
                    vlines.append(&mut ref_vlines);
                }

                line_idx = cur;
            }
        }
    }

    // Trailing lines not covered by items
    while line_idx < lines.len() {
        if let Some(DocLine::Text(s)) = lines.get(line_idx) {
            push_wrapped_text_vlines(
                &mut vlines,
                s,
                line_idx,
                color_for_text(s),
                Vec::new(),
                wrap_width,
                ctx,
                font_id,
            );
        }
        line_idx += 1;
    }

    vlines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::annotations::AnnotatedText;

    fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        // Run a frame so the font atlas exists before we measure with it.
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        ctx
    }

    fn width(ctx: &egui::Context, font_id: &egui::FontId, s: &str) -> f32 {
        if s.is_empty() {
            return 0.0;
        }
        ctx.fonts(|f| {
            f.layout_no_wrap(s.to_string(), font_id.clone(), egui::Color32::WHITE)
                .rect
                .width()
        })
    }

    /// Soft wrapping measures the *rendered* line, so an annotation never
    /// splits across a wrap and never pushes a segment past the wrap width:
    /// the line breaks before the annotated character instead.
    #[test]
    fn annotations_wrap_atomically_with_their_character() {
        let ctx = test_ctx();
        let font_id = egui::FontId::monospace(16.0);
        let text = "map 가 = hangul-syllable-ga-with-a-long-name";
        let annotations = annotations::line_annotations(text);
        assert_eq!(annotations.len(), 1);

        for w in (30..500).step_by(11) {
            let max_w = w as f32;
            let segments = compute_wrap_segments(text, &annotations, Some(max_w), &ctx, &font_id);

            // Segments still partition the document line in order.
            let joined: String = segments.iter().map(|(s, _)| s.as_str()).collect();
            assert_eq!(joined, text, "segments must reassemble the line (w={w})");
            let mut expected_offset = 0usize;
            for (seg, off) in &segments {
                assert_eq!(*off, expected_offset, "segment offsets must be contiguous");
                expected_offset += seg.chars().count();
            }

            // No segment overflows, and the annotation is never cut in half.
            let mut seen_annotation = 0usize;
            for (seg, off) in &segments {
                let seg_len = seg.chars().count();
                let seg_annotations = segment_annotations(&annotations, *off, seg_len);
                seen_annotation += seg_annotations.len();
                for a in &seg_annotations {
                    assert!(a.col >= 1 && a.col <= seg_len);
                }
                let display = AnnotatedText::new(seg, &seg_annotations).display_string();
                assert!(
                    width(&ctx, &font_id, &display) <= max_w || seg_len <= 1,
                    "rendered segment {display:?} overflows wrap width {max_w}"
                );
            }
            assert_eq!(
                seen_annotation, 1,
                "the annotation must land on exactly one segment (w={w})"
            );
        }
    }

    /// The unannotated line wraps exactly as before the annotation feature.
    #[test]
    fn lines_without_annotations_wrap_unchanged() {
        let ctx = test_ctx();
        let font_id = egui::FontId::monospace(16.0);
        let text = "glyph hangul-syllable-ga 16 16 advance 16 left 0";
        for w in (30..500).step_by(11) {
            let segments = compute_wrap_segments(text, &[], Some(w as f32), &ctx, &font_id);
            let joined: String = segments.iter().map(|(s, _)| s.as_str()).collect();
            assert_eq!(joined, text);
            for (seg, _) in &segments {
                assert!(
                    width(&ctx, &font_id, seg) <= w as f32 || seg.chars().count() <= 1,
                    "segment {seg:?} overflows wrap width {w}"
                );
            }
        }
    }
}
