use std::collections::HashMap;

use crate::document::{
    DocLine, Document, DocumentItem, GlyphBody, GlyphRef, NamePartsMap, is_name_pattern,
};
use crate::editor::annotations::{self, InlineAnnotation};
use crate::editor::colors::Palette;
use crate::editor::doc_links;
use crate::editor::ref_composite::{self, GlyphComposite, ResolvedGlyph};
use crate::pattern::NamePattern;

use super::document_view::{
    GRID_CELL, GlyphMetrics, GridExtent, HeadingLine, INLINE_PALETTE_CELL, PREVIEW_SCALE,
    VLineKind, VisualLine, compute_grid_display_extent, glyph_metrics, heading_font_size,
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
            ref_composite::resolve_ref_name_for_view(&gref.name, named_glyphs, name_parts)
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

/// One character of the *rendered* line: either a document character (`col` is
/// its own column) or a character of an annotation (`col` is the column the
/// annotation trails, i.e. that of the document character it precedes).
struct DisplayUnit {
    ch: char,
    col: usize,
    is_annotation: bool,
}

/// The rendered line, character by character. This is `display_string()`
/// unrolled, and it is what soft wrapping breaks: an annotation is ordinary
/// text as far as the line breaker is concerned.
fn display_units(text: &str, annotations: &[InlineAnnotation]) -> Vec<DisplayUnit> {
    let mut units = Vec::new();
    let mut ai = 0usize;
    for (i, c) in text.chars().enumerate() {
        units.push(DisplayUnit {
            ch: c,
            col: i,
            is_annotation: false,
        });
        while let Some(a) = annotations.get(ai) {
            if a.col <= i + 1 {
                units.extend(a.text.chars().map(|ch| DisplayUnit {
                    ch,
                    col: a.col,
                    is_annotation: true,
                }));
                ai += 1;
            } else {
                break;
            }
        }
    }
    units
}

/// A run of display units back into one wrapped segment: `(document text,
/// document column it starts at, annotations rebased onto it)`.
///
/// A run may begin inside an annotation, which becomes an annotation at
/// relative column 0 — the tail of one that started on the previous segment.
/// A run may also hold no document character at all, when an annotation is
/// longer than the wrap width.
fn segment_from_units(units: &[DisplayUnit]) -> (String, usize, Vec<InlineAnnotation>) {
    let col_offset = units
        .iter()
        .find(|u| !u.is_annotation)
        .or_else(|| units.first())
        .map_or(0, |u| u.col);
    let mut text = String::new();
    let mut anns: Vec<InlineAnnotation> = Vec::new();
    let mut open_ann_col: Option<usize> = None;
    for u in units {
        if u.is_annotation {
            let col = u.col.saturating_sub(col_offset);
            match anns.last_mut() {
                Some(last) if open_ann_col == Some(col) => last.text.push(u.ch),
                _ => {
                    anns.push(InlineAnnotation {
                        col,
                        text: u.ch.to_string(),
                    });
                    open_ann_col = Some(col);
                }
            }
        } else {
            text.push(u.ch);
            open_ann_col = None;
        }
    }
    (text, col_offset, anns)
}

fn compute_wrap_segments(
    text: &str,
    annotations: &[InlineAnnotation],
    wrap_width: Option<f32>,
    ctx: &egui::Context,
    font_id: &egui::FontId,
) -> Vec<(String, usize, Vec<InlineAnnotation>)> {
    let all = display_units(text, annotations);
    let whole = || vec![segment_from_units(&all)];
    let max_w = match wrap_width {
        Some(w) if w > 0.0 => w,
        _ => return whole(),
    };

    // Widths are measured on the *rendered* line, annotations included, so a
    // line only wraps where it visually overflows.
    let width = |units: &[DisplayUnit]| -> f32 {
        if units.is_empty() {
            return 0.0;
        }
        let s: String = units.iter().map(|u| u.ch).collect();
        ctx.fonts(|f| {
            f.layout_no_wrap(s, font_id.clone(), egui::Color32::WHITE)
                .rect
                .width()
        })
    };

    if width(&all) <= max_w {
        return whole();
    }

    let mut result = Vec::new();
    let mut start = 0usize;
    while start < all.len() {
        if width(&all[start..]) <= max_w {
            result.push(segment_from_units(&all[start..]));
            break;
        }

        let mut lo = 1usize;
        let mut hi = all.len() - start;
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if width(&all[start..start + mid]) <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let end = start + lo.max(1);
        result.push(segment_from_units(&all[start..end]));
        start = end;
    }

    if result.is_empty() {
        return whole();
    }
    result
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
    heading: Option<HeadingLine>,
) {
    let annotations = annotations::line_annotations(text);
    // A `// …` comment is a comment wherever the line's own color comes from.
    let comment_col = crate::document_io::split_comment(text)
        .1
        .map(|c| text.chars().count() - c.chars().count());
    // A heading wraps against its own, larger font, or a long title would run
    // off the page it was measured to fit.
    let heading_font;
    let font_id = match heading {
        Some(h) if h.font_size != font_id.size => {
            heading_font = egui::FontId::new(h.font_size, font_id.family.clone());
            &heading_font
        }
        _ => font_id,
    };
    let segments = compute_wrap_segments(text, &annotations, wrap_width, ctx, font_id);
    for (seg_text, col_offset, seg_annotations) in segments {
        let seg_len = seg_text.chars().count();
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
            heading,
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
            heading: None,
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
    // The parser lets `ref` and `anchor` lines interleave, so the nth body
    // line is not necessarily the nth ref. Walk the body lines by their first
    // token (the same scan `pixel_interaction::layer_doc_line` does), pairing
    // each `ref` line with the next ref in order, until every ref has been
    // seen; interleaved `anchor` lines are pushed as plain text on the way,
    // and trailing anchors are left for the caller's catch-up loop as before.
    let mut refs_in_order = refs.iter();
    let mut next_ref = refs_in_order.next();
    while next_ref.is_some() {
        let Some(DocLine::Text(s)) = lines.get(*cur) else {
            break;
        };
        let mut error_spans = Vec::new();
        match s.split_whitespace().next() {
            Some("ref") => {
                let r = next_ref.expect("loop guard");
                next_ref = refs_in_order.next();
                // `ifexists` says the absent case is expected, so it is not
                // underlined either: the flag exists for a pattern written over
                // a range where most of the names have no target, and marking
                // every one of them would be the noise it was added to remove.
                if !r.if_exists
                    && !ref_composite::is_ref_valid(&r.name, named_glyphs, name_parts)
                    && let Some(range) = doc_links::find_name_col_range_after_prefix(s, "ref ")
                {
                    error_spans.push((range.0, range.1, format!("undefined glyph: {}", r.name)));
                }
            }
            // Passed through like an `anchor`: an IDC line stands for refs the
            // body does not list, so it pairs with none of them.
            Some("anchor") => {}
            Some(tok) if crate::compose::IdcOp::from_token(tok).is_some() => {}
            // The lines no longer match the parsed body; stop and let the
            // caller's catch-up loop display the rest.
            _ => break,
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
            None,
        );
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
    let heading_color = pal.text_heading;
    let meta_color = pal.text_meta;
    let header_color = pal.text_header;
    let ref_color = pal.text_ref;
    let directive_color = pal.text_directive;
    let directive2_color = pal.text_directive2;
    let default_color = pal.text_default;

    // A heading line's level decides both its color and its size, and is read
    // off the text rather than off the item list: the lines *between* items are
    // pushed by the catch-up loops below, which never see an item at all.
    let heading_of = |s: &str| -> Option<HeadingLine> {
        let (level, _) = crate::document_io::split_heading(s.trim())?;
        let font_size = heading_font_size(font_id.size, level);
        let font = egui::FontId::new(font_size, font_id.family.clone());
        Some(HeadingLine {
            level,
            font_size,
            row_height: ctx.fonts(|f| f.row_height(&font)),
        })
    };

    let color_for_text = |s: &str| -> egui::Color32 {
        let trimmed = s.trim_start();
        if trimmed.starts_with("//") {
            comment_color
        } else if crate::document_io::split_heading(trimmed).is_some() {
            heading_color
        } else if trimmed.starts_with("glyph ") || trimmed.starts_with("map ") {
            header_color
        } else if trimmed.starts_with("ref ")
            || trimmed.starts_with("anchor ")
            || trimmed
                .split_whitespace()
                .next()
                .and_then(crate::compose::IdcOp::from_token)
                .is_some()
        {
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
                    heading_of(s),
                );
            }
            line_idx += 1;
        }
        match item {
            DocumentItem::BlankLine
            | DocumentItem::Comment(_)
            | DocumentItem::Heading { .. }
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
            | DocumentItem::PropBlock { .. }
            | DocumentItem::PropChar { .. }
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
                        heading_of(s),
                    );
                }
                line_idx = item_start + 1;
            }
            // An alias declares no glyph, so it is one text line with no grid,
            // no refs and nothing to preview — only its target is checked.
            DocumentItem::GlyphAlias { name, target, .. } => {
                if let Some(DocLine::Text(s)) = lines.get(item_start) {
                    let mut error_spans = Vec::new();

                    let name_str = name.display();
                    if is_name_pattern(&name_str)
                        && let Err(e) = NamePattern::parse(&name_str)
                        && let Some(range) =
                            doc_links::find_name_col_range_after_prefix(s, "glyph ")
                    {
                        error_spans.push((range.0, range.1, e.to_string()));
                    }

                    if !ref_composite::is_ref_valid(target, named_glyphs, name_parts)
                        && let Some(eq_byte) = s.trim().find(" = ")
                    {
                        let trimmed = s.trim_start();
                        let leading_c = s.chars().count() - trimmed.chars().count();
                        let eq_prefix_c = trimmed[..eq_byte + 3].chars().count();
                        let target_c = trimmed[eq_byte + 3..]
                            .split_whitespace()
                            .next()
                            .map_or(0, |t| t.chars().count());
                        if target_c > 0 {
                            let cs = leading_c + eq_prefix_c;
                            error_spans.push((
                                cs,
                                cs + target_c,
                                format!("undefined glyph: {target}"),
                            ));
                        }
                    }

                    push_wrapped_text_vlines(
                        &mut vlines,
                        s,
                        item_start,
                        color_for_text(s),
                        error_spans,
                        wrap_width,
                        ctx,
                        font_id,
                        None,
                    );
                }
                line_idx = item_start + 1;
            }
            DocumentItem::Glyph { name, body } => {
                // Header line
                let header_line = item_start;

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

                    push_wrapped_text_vlines(
                        &mut vlines,
                        s,
                        header_line,
                        color_for_text(s),
                        error_spans,
                        wrap_width,
                        ctx,
                        font_id,
                        None,
                    );
                }

                let mut cur = header_line + 1;

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
                    let (own_w, own_h, mut extent) = compute_grid_display_extent(
                        Some(grid),
                        composites.get(&item_idx),
                        &body.points,
                    );
                    if let Some(s) = shadow {
                        extent.include_shadow(s);
                    }
                    let metrics = show_metrics.then(|| {
                        let m = glyph_metrics(body, composites.get(&item_idx), own_w, own_h, meta);
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
                if body.pixels.is_none() && has_composite {
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
                        let m = glyph_metrics(body, Some(comp), own_w, own_h, meta);
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
                } else {
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
                heading_of(s),
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

    /// An annotation wraps exactly as the same text written in the document
    /// would: the break may land inside it, and the character it trails is
    /// never dragged onto the next line to keep it whole.
    #[test]
    fn a_long_annotation_wraps_like_ordinary_text() {
        let ctx = test_ctx();
        let font_id = egui::FontId::monospace(16.0);
        // A long `assert shape` text spells out one `U+XXXX` per character, so
        // the annotation alone outruns any plausible wrap width.
        let text = "assert shape 안녕하세요반갑습니다 : greeting";
        let annotations = annotations::line_annotations(text);
        assert_eq!(annotations.len(), 1);
        assert!(annotations[0].text.chars().count() > 40);
        let full_display = AnnotatedText::new(text, &annotations).display_string();

        for w in (30..500).step_by(11) {
            let max_w = w as f32;
            let segments = compute_wrap_segments(text, &annotations, Some(max_w), &ctx, &font_id);

            // Segments still partition the document line in order.
            let joined: String = segments.iter().map(|(s, _, _)| s.as_str()).collect();
            assert_eq!(joined, text, "segments must reassemble the line (w={w})");
            let mut expected_offset = 0usize;
            for (seg, off, _) in &segments {
                assert_eq!(*off, expected_offset, "segment offsets must be contiguous");
                expected_offset += seg.chars().count();
            }

            // The rendered segments reassemble the rendered line: annotation
            // text is split, never dropped or duplicated.
            let joined_display: String = segments
                .iter()
                .map(|(seg, _, anns)| AnnotatedText::new(seg, anns).display_string())
                .collect();
            assert_eq!(
                joined_display, full_display,
                "rendered segments must reassemble the rendered line (w={w})"
            );

            for (seg, _, anns) in &segments {
                let display = AnnotatedText::new(seg, anns).display_string();
                assert!(
                    width(&ctx, &font_id, &display) <= max_w || display.chars().count() <= 1,
                    "rendered segment {display:?} overflows wrap width {max_w}"
                );
            }
        }
    }

    /// The parser lets `anchor` lines interleave with `ref` lines, so pairing
    /// the nth body line with the nth ref mis-attributes the undefined-ref
    /// check: the ref past the interleaved anchor lost its error span.
    #[test]
    fn interleaved_anchor_lines_do_not_derail_undefined_ref_spans() {
        let ctx = test_ctx();
        let font_id = egui::FontId::monospace(16.0);
        let lines = vec![
            DocLine::Text("ref alpha".into()),
            DocLine::Text("anchor + 0 0".into()),
            DocLine::Text("ref beta".into()),
        ];
        let gref = |name: &str| GlyphRef {
            raw_name: None,
            name: name.into(),
            offset: None,
            negated: false,
            inherit: false,
            if_exists: false,
            fill: None,
            visibility: None,
            comment: None,
        };
        let refs = vec![gref("alpha"), gref("beta")];
        // No glyphs defined at all: every ref line must carry an error span.
        let named_glyphs = HashMap::new();
        let name_parts = NamePartsMap::default();
        let mut cur = 0usize;
        let vlines = build_ref_vlines(
            &lines,
            &refs,
            &mut cur,
            &named_glyphs,
            &name_parts,
            &|_| egui::Color32::WHITE,
            None,
            &ctx,
            &font_id,
        );
        assert_eq!(cur, 3, "the scan must reach past the interleaved anchor");
        assert_eq!(vlines.len(), 3);
        assert_eq!(vlines[0].error_spans.len(), 1, "ref alpha is undefined");
        assert!(vlines[0].error_spans[0].2.contains("alpha"));
        assert!(
            vlines[1].error_spans.is_empty(),
            "the anchor line is not a ref"
        );
        assert_eq!(vlines[2].error_spans.len(), 1, "ref beta is undefined");
        assert!(vlines[2].error_spans[0].2.contains("beta"));
    }

    /// An `ifexists` ref is expected to name nothing much of the time — that is
    /// what it is for — so the editor must not underline it. The plain ref
    /// beside it still is.
    #[test]
    fn an_ifexists_ref_carries_no_undefined_span() {
        let ctx = test_ctx();
        let font_id = egui::FontId::monospace(16.0);
        let lines = vec![
            DocLine::Text("ref alpha ifexists".into()),
            DocLine::Text("ref beta".into()),
        ];
        let gref = |name: &str, if_exists: bool| GlyphRef {
            raw_name: None,
            name: name.into(),
            offset: None,
            negated: false,
            inherit: false,
            if_exists,
            fill: None,
            visibility: None,
            comment: None,
        };
        let refs = vec![gref("alpha", true), gref("beta", false)];
        let named_glyphs = HashMap::new();
        let name_parts = NamePartsMap::default();
        let mut cur = 0usize;
        let vlines = build_ref_vlines(
            &lines,
            &refs,
            &mut cur,
            &named_glyphs,
            &name_parts,
            &|_| egui::Color32::WHITE,
            None,
            &ctx,
            &font_id,
        );
        assert!(vlines[0].error_spans.is_empty());
        assert_eq!(vlines[1].error_spans.len(), 1);
    }

    /// The unannotated line wraps exactly as before the annotation feature.
    #[test]
    fn lines_without_annotations_wrap_unchanged() {
        let ctx = test_ctx();
        let font_id = egui::FontId::monospace(16.0);
        let text = "glyph hangul-syllable-ga 16 16 advance 16 left 0";
        for w in (30..500).step_by(11) {
            let segments = compute_wrap_segments(text, &[], Some(w as f32), &ctx, &font_id);
            let joined: String = segments.iter().map(|(s, _, _)| s.as_str()).collect();
            assert_eq!(joined, text);
            for (seg, _, _) in &segments {
                assert!(
                    width(&ctx, &font_id, seg) <= w as f32 || seg.chars().count() <= 1,
                    "segment {seg:?} overflows wrap width {w}"
                );
            }
        }
    }
}
