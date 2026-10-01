//! `DocLine`s to `Document` items: `derive_document`, `rederive_document` and the
//! `derive_items` dispatch, with the item-start keyword predicates.

use crate::document::*;

use super::header::{
    parse_anchor_point, parse_compose_line, parse_glyph_flag_parts, parse_ref_line,
};
use super::tokens::{
    continuation_text, dedent_continuations, quote_token, split_comment_owned, split_heading,
    split_written_uvs_pair, tokenize_tokens,
};

/// The keywords that begin a top-level item, as [`derive_document`] dispatches
/// on them.
///
/// A line starting with one of these ends whatever block came before it; a line
/// starting with anything else — `ref`, `anchor`, an IDC operator, a pixel row —
/// belongs to the glyph block above. `assume` is the one keyword that is both:
/// in front of an IDC operator it is that line's
/// ([`IdcOp::of_line`](crate::compose::IdcOp::of_line)), so a caller holding
/// more than the first token asks [`line_starts_item`] instead. The editor's
/// text-only passes (search, navigation) need that boundary without parsing the
/// file, so it is stated here, beside the dispatch it has to agree with, rather
/// than re-listed there.
pub fn starts_item(token: &str) -> bool {
    matches!(
        token,
        "meta"
            | "audit"
            | "assume"
            | "map"
            | "glyph"
            | "name-parts"
            | "remap"
            | "feature"
            | "assert"
            | "face"
            | "slice"
            | "prop"
            | "exists"
            | "color"
            | "sample"
    )
}

/// Whether a line, given as its tokens, begins a top-level item: [`starts_item`]
/// on its first token, less an `assume` that is an IDC line's own.
pub fn line_starts_item<'a>(mut tokens: impl Iterator<Item = &'a str> + Clone) -> bool {
    let rest = tokens.clone();
    tokens.next().is_some_and(starts_item) && crate::compose::IdcOp::of_line(rest).is_none()
}

pub fn derive_document(lines: &[DocLine], path: std::path::PathBuf) -> (Document, Vec<usize>) {
    let mut doc = Document::new(path);
    let parsed = derive_items(lines, 0, None, &|_, _| false);
    doc.items = parsed.items;
    doc.item_line_starts = parsed.item_line_starts.clone();
    doc.docline_file_lines = crate::document::compute_docline_file_lines(lines);
    #[cfg(feature = "editor")]
    {
        doc.line_ids = lines.iter().map(DocLine::line_id).collect();
        doc.at_bases = parsed.at_bases.into();
        doc.line_fps = lines
            .iter()
            .map(crate::document::line_fingerprint)
            .collect();
    }
    (doc, parsed.item_line_starts)
}

/// Which items a [`rederive_document`] parsed afresh: `old` in the previous
/// document were replaced by `new` in this one. Every item outside them is the
/// previous document's, at the same index before them and shifted by the
/// difference in length after.
#[cfg(feature = "editor")]
#[derive(Clone, Debug, PartialEq)]
pub struct Reparse {
    pub old: std::ops::Range<usize>,
    pub new: std::ops::Range<usize>,
}

/// [`derive_document`] of `lines`, reusing what `old` — derived from the same
/// buffer before an edit — still says about the lines the edit did not touch.
///
/// The changed lines are found by [`Document::line_fps`]. The parse restarts
/// one item before the first of them (a line that stops starting an item is
/// absorbed by the item above it) under the `@` base recorded there, and stops
/// at the first item boundary past the last of them that `old` also had under
/// the same base: from there on the parse would repeat `old`'s exactly (see
/// [`derive_items`]). A buffer whose length changed is derived whole — an item
/// boundary is a line index, and every one after the edit moved.
///
/// Returns the document, whether anything a rebuild reads changed (see
/// [`crate::document::items_changed_for_rebuild`]), and which items were
/// parsed afresh, when the parse was not whole.
#[cfg(feature = "editor")]
pub fn rederive_document(
    mut old: Document,
    lines: &[DocLine],
) -> (Document, bool, Option<Reparse>) {
    let fps: Vec<u64> = lines
        .iter()
        .map(crate::document::line_fingerprint)
        .collect();
    let usable = old.line_fps.len() == lines.len()
        && old.at_bases.len() == old.items.len()
        && old.item_line_starts.len() == old.items.len();
    let changed = usable.then(|| {
        let mut diff = old
            .line_fps
            .iter()
            .zip(&fps)
            .enumerate()
            .filter(|(_, (a, b))| a != b);
        let first = diff.next().map(|(i, _)| i)?;
        let last = diff.next_back().map_or(first, |(i, _)| i);
        Some((first, last))
    });
    let Some(changed) = changed else {
        let (doc, _) = derive_document(lines, old.path.clone());
        let rebuild = crate::document::items_changed_for_rebuild(&old.items, &doc.items);
        return (doc, rebuild, None);
    };
    let (ks, j, parsed) = match changed {
        // The same text line for line: nothing to parse.
        None => (0, 0, None),
        Some((first, last)) => {
            let starts = &old.item_line_starts;
            let holding = starts.partition_point(|&s| s <= first);
            let ks = holding.saturating_sub(2);
            let (start, base) = if holding == 0 {
                (0, None)
            } else {
                (starts[ks], old.at_bases[ks].as_deref().map(str::to_string))
            };
            let at_bases = &old.at_bases;
            let stop = |i: usize, base: Option<&str>| {
                i > last
                    && starts
                        .binary_search(&i)
                        .is_ok_and(|j| at_bases[j].as_deref() == base)
            };
            let parsed = derive_items(lines, start, base, &stop);
            let j = if parsed.end >= lines.len() {
                old.items.len()
            } else {
                starts
                    .binary_search(&parsed.end)
                    .expect("stopped at an old boundary")
            };
            (ks, j, Some(parsed))
        }
    };
    let mut rebuild = false;
    let reparse = match parsed {
        None => Reparse {
            old: 0..0,
            new: 0..0,
        },
        Some(parsed) => {
            let n = parsed.items.len();
            rebuild = crate::document::items_changed_for_rebuild(&old.items[ks..j], &parsed.items);
            let removed: Vec<DocumentItem> = old.items.splice(ks..j, parsed.items).collect();
            drop(removed);
            old.item_line_starts.splice(ks..j, parsed.item_line_starts);
            let mut at_bases = old.at_bases.to_vec();
            at_bases.splice(ks..j, parsed.at_bases);
            old.at_bases = at_bases.into();
            Reparse {
                old: ks..j,
                new: ks..ks + n,
            }
        }
    };
    old.docline_file_lines = crate::document::compute_docline_file_lines(lines);
    old.line_ids = lines.iter().map(DocLine::line_id).collect();
    old.line_fps = fps.into();
    (old, rebuild, Some(reparse))
}

/// What one run of [`derive_items`] parsed.
struct DerivedItems {
    items: Vec<DocumentItem>,
    item_line_starts: Vec<usize>,
    /// The `@` base each item was parsed under; see [`Document::at_bases`].
    #[cfg_attr(not(feature = "editor"), expect(dead_code))]
    at_bases: Vec<Option<std::sync::Arc<str>>>,
    /// The line the run stopped at: an item boundary, or the end.
    #[cfg_attr(not(feature = "editor"), expect(dead_code))]
    end: usize,
}

/// The items of `lines` from line `start` on, with `at_base` the `@` base in
/// force there, until `stop` says so at an item boundary.
///
/// That is the whole of the parser's state between two items — the line it is
/// at and the base — so a run started at an item boundary with the base
/// recorded for it parses exactly what a run from the top would, and a run
/// that reaches a boundary a previous derive also had, under the same base,
/// would parse the rest exactly as that derive did.
fn derive_items(
    lines: &[DocLine],
    start: usize,
    at_base: Option<String>,
    stop: &dyn Fn(usize, Option<&str>) -> bool,
) -> DerivedItems {
    let mut items: Vec<DocumentItem> = Vec::new();
    let mut item_line_starts: Vec<usize> = Vec::new();
    let mut at_bases: Vec<Option<std::sync::Arc<str>>> = Vec::new();
    let mut i = start;
    // The `@` base: the last glyph name declared without one. Scoped to the
    // file, and carried across the lines between two glyph blocks, so a helper
    // glyph keeps expanding against its base however far below it is written.
    let mut at_base = at_base;
    let mut base_here: Option<std::sync::Arc<str>> = at_base.as_deref().map(std::sync::Arc::from);

    while i < lines.len() {
        // What the items the last pass pushed were parsed under — a pass
        // pushes at most one, and does it before `at_base` can move below.
        at_bases.resize(items.len(), base_here.clone());
        if stop(i, at_base.as_deref()) {
            break;
        }
        if base_here.as_deref() != at_base.as_deref() {
            base_here = at_base.as_deref().map(std::sync::Arc::from);
        }
        match &lines[i] {
            DocLine::Grid(_) => {
                // Orphan grid — skip (reconciliation should prevent this)
                i += 1;
            }
            DocLine::Text(s) => {
                let trimmed = s.trim();

                if trimmed.is_empty() {
                    item_line_starts.push(i);
                    items.push(DocumentItem::BlankLine);
                    i += 1;
                    continue;
                }

                if let Some(comment) = trimmed.strip_prefix("//") {
                    item_line_starts.push(i);
                    items.push(DocumentItem::Comment(comment.to_string()));
                    i += 1;
                    continue;
                }

                // A continuation reaching here is one nothing claimed: every
                // command that takes them swallows its own below. Kept as an
                // opaque directive so the line survives a round trip and
                // `issues` can name it — see `Directive::OrphanContinuation`.
                if continuation_text(trimmed).is_some() {
                    item_line_starts.push(i);
                    items.push(DocumentItem::Directive(trimmed.to_string()));
                    i += 1;
                    continue;
                }

                // A heading is prose after its `#` run, so it is taken whole
                // like a comment rather than tokenized: a backtick in a section
                // title is a backtick, not an unterminated quote.
                if let Some((level, text)) = split_heading(trimmed) {
                    item_line_starts.push(i);
                    items.push(DocumentItem::Heading {
                        level,
                        text: text.to_string(),
                    });
                    i += 1;
                    continue;
                }

                // Every directive line may end in a `// …` comment; it is one
                // token, it never reaches the grammar below, and it is kept on
                // the item so serializing the document does not drop it.
                let (body_text, comment) = split_comment_owned(trimmed);
                let comment_raw = comment
                    .as_deref()
                    .map(|c| format!(" // {c}"))
                    .unwrap_or_default();
                // A line the tokenizer cannot read at all — in practice an
                // unterminated `` ` `` quote, usually one the author is halfway
                // through typing — is kept as one opaque text item rather than
                // failing the whole derive. The editor builds its visual lines
                // from this item list against the same buffer, so a derive that
                // bails leaves the *previous* structure in place and every grid
                // after the bad line is drawn on the wrong line. `issues.rs`
                // reports it as an unrecognized directive; the strict CLI path
                // (`tokenize_strict`) still rejects the file outright.
                let Ok(tokens) = tokenize_tokens(body_text) else {
                    item_line_starts.push(i);
                    items.push(DocumentItem::Directive(trimmed.to_string()));
                    i += 1;
                    continue;
                };
                if tokens.is_empty() {
                    item_line_starts.push(i);
                    // A comment-only line never reaches here: it was taken by
                    // the `//` branch above.
                    items.push(DocumentItem::BlankLine);
                    i += 1;
                    continue;
                }

                match tokens[0].as_str() {
                    "meta" | "audit" => {
                        item_line_starts.push(i);
                        let rest: Vec<String> =
                            tokens[1..].iter().map(|t| quote_token(t)).collect();
                        let text = rest.join(" ");
                        let text = format!("{}{comment_raw}", text.trim_end());
                        items.push(if tokens[0] == "meta" {
                            DocumentItem::Meta(text)
                        } else {
                            DocumentItem::Audit(text)
                        });
                        i += 1;
                    }
                    "assume" => {
                        item_line_starts.push(i);
                        let rest: Vec<String> =
                            tokens[1..].iter().map(|t| quote_token(t)).collect();
                        let text = format!("{} {}", tokens[0], rest.join(" "));
                        items.push(DocumentItem::Directive(format!(
                            "{}{comment_raw}",
                            text.trim_end()
                        )));
                        i += 1;
                    }
                    "map" => {
                        // An optional `SLICE :` qualifier comes off first, so
                        // the arities below are the same ones the unqualified
                        // form has always had. See
                        // `DocumentItem::split_slice_qualifier` for why the
                        // qualifier cannot be confused with `map : = colon`.
                        let (slices, tokens) = DocumentItem::split_slice_qualifier(&tokens[1..]);
                        // `map generate CHAR [= GLYPH]` is checked first, but only
                        // in the arities the plain form cannot take: `map generate
                        // = g` stays an ordinary (if nonsensical) `map`.
                        let generate = tokens.len() >= 2 && tokens[0] == "generate";
                        // Everything past the `=` is a target, and there may be
                        // several: `map A = first second` tries `first` and
                        // falls back to `second`. See `DocumentItem::Map`.
                        if tokens.len() >= 3 && tokens[1] == "=" {
                            let (char_repr, selector) = split_written_uvs_pair(&tokens[0]);
                            item_line_starts.push(i);
                            items.push(DocumentItem::Map {
                                slices,
                                char_repr,
                                selector,
                                glyphs: tokens[2..].to_vec(),
                                comment,
                            });
                            i += 1;
                        } else if generate
                            && (tokens.len() == 2 || (tokens.len() == 4 && tokens[2] == "="))
                        {
                            let (char_repr, selector) = split_written_uvs_pair(&tokens[1]);
                            item_line_starts.push(i);
                            items.push(DocumentItem::MapDecomposed {
                                slices,
                                char_repr,
                                selector,
                                glyph: tokens.get(3).cloned(),
                                comment,
                            });
                            i += 1;
                        } else if generate
                            && (tokens.len() == 3 || (tokens.len() == 5 && tokens[3] == "="))
                        {
                            // `map generate BASE SELECTOR [= GLYPH]` parses so
                            // that it can be *rejected* by name. `generate`
                            // wins this arity over the plain pair form below,
                            // which is what keeps `map generate Á = a-acute`
                            // decomposed rather than read as a sequence.
                            item_line_starts.push(i);
                            items.push(DocumentItem::MapDecomposed {
                                slices,
                                char_repr: tokens[1].clone(),
                                selector: Some(tokens[2].clone()),
                                glyph: tokens.get(4).cloned(),
                                comment,
                            });
                            i += 1;
                        } else if tokens.len() >= 4 && tokens[2] == "=" && !generate {
                            // `!generate`, so the one arity the two forms share
                            // (`map generate B = g`) stays decomposed above
                            // rather than becoming a variation sequence here.
                            item_line_starts.push(i);
                            items.push(DocumentItem::Map {
                                slices,
                                char_repr: tokens[0].clone(),
                                selector: Some(tokens[1].clone()),
                                glyphs: tokens[3..].to_vec(),
                                comment,
                            });
                            i += 1;
                        } else {
                            item_line_starts.push(i);
                            items.push(DocumentItem::Directive(trimmed.to_string()));
                            i += 1;
                        }
                    }
                    "glyph" => {
                        let header_idx = i;
                        i += 1;

                        let parts = &tokens[1..];
                        // A bare `glyph`, which is what a header being typed
                        // from scratch looks like for a keystroke or two. Same
                        // reasoning as the unreadable line above: one opaque
                        // item, not a failed derive.
                        if parts.is_empty() {
                            item_line_starts.push(header_idx);
                            items.push(DocumentItem::Directive(trimmed.to_string()));
                            continue;
                        }

                        // The header's own `@` expands against the base that
                        // was already in force, and only a header written
                        // *without* one becomes the next base — which is what
                        // makes `glyph @-bar` / `ref @-baz` name `foo-baz`
                        // rather than `foo-bar-baz`.
                        let rest_parts = &parts[1..];
                        let alias_target = rest_parts
                            .iter()
                            .position(|p| p == "=")
                            .and_then(|eq_pos| rest_parts.get(eq_pos + 1));
                        // `glyph NAME* = PREFIX*` is read as the alias its
                        // `exists` form scopes, `glyph NAME($1) = ($0)`, and
                        // every name-reading step below sees that. One the
                        // strict parse rejects stays an ordinary alias here,
                        // whose `*` names no glyph.
                        let multi = crate::document::header_multi_alias(parts);
                        let written = crate::document::header_written_name(parts).into_owned();
                        let expanded =
                            crate::document::expand_at_name(&written, at_base.as_deref());
                        let raw_name = match multi {
                            Some(_) => Some(parts[0].clone()),
                            None => crate::document::written_form(&written, &expanded),
                        };
                        if let Some(base) = crate::document::at_base_from_glyph_name(&written) {
                            at_base = Some(base);
                        }
                        let name = parse_glyph_name(&expanded);

                        // `glyph NAME = TARGET` is an alias: a name for a
                        // glyph, with no body of its own. Flags before the `=`
                        // are rejected by `validate_glyph_header`; the lenient
                        // `DocLine` path drops them the same way.
                        if let Some(target) = alias_target {
                            item_line_starts.push(header_idx);
                            let (expanded_target, raw_target, search_prefix) = match multi {
                                Some((_, prefix)) => (
                                    "($0)".to_string(),
                                    Some(target.clone()),
                                    Some(prefix.to_string()),
                                ),
                                None => {
                                    let expanded_target =
                                        crate::document::expand_at_name(target, at_base.as_deref());
                                    let raw_target =
                                        crate::document::written_form(target, &expanded_target);
                                    (expanded_target, raw_target, None)
                                }
                            };
                            items.push(DocumentItem::GlyphAlias {
                                name,
                                target: expanded_target,
                                raw_name,
                                raw_target,
                                search_prefix,
                                comment,
                            });
                            continue;
                        }

                        let mut body = GlyphBody::new();
                        body.comment = comment;
                        body.raw_name = raw_name;
                        let flags = parse_glyph_flag_parts(rest_parts);
                        body.keep = flags.keep;
                        body.inline = flags.inline;
                        body.mark = flags.mark;
                        body.desync = flags.desync;
                        body.vectoronly = flags.vectoronly;
                        body.advance = flags.advance;
                        body.origin = flags.origin;
                        body.extent = flags.extent;
                        body.scale = flags.scale.unwrap_or(1);
                        body.margin = flags.margin;
                        let scale = body.scale as u16;
                        let (width, height) = (
                            flags.width.and_then(|w| w.checked_mul(scale)),
                            flags.height.and_then(|h| h.checked_mul(scale)),
                        );

                        if let (Some(w), Some(h)) = (width, height) {
                            if let Some(DocLine::Grid(g)) = lines.get(i)
                                && g.width == w
                                && g.height == h
                            {
                                body.pixels = Some(PixelGrid::clone(g));
                                i += 1;
                            } else {
                                body.pixels = Some(PixelGrid::new(w, h));
                            }
                        }

                        // Collect ref and anchor lines
                        while let Some(DocLine::Text(t)) = lines.get(i) {
                            let (sub_text, sub_comment) = split_comment_owned(t.trim());
                            let sub_tokens = match tokenize_tokens(sub_text) {
                                Ok(t) => t,
                                Err(_) => break,
                            };
                            if sub_tokens.first().is_some_and(|t| t == "ref") {
                                let parsed_ref = parse_ref_line(
                                    &sub_tokens[1..],
                                    sub_comment,
                                    at_base.as_deref(),
                                );
                                let Some(parsed_ref) = parsed_ref else {
                                    break;
                                };
                                body.refs.push(parsed_ref);
                                i += 1;
                                continue;
                            } else if let Some((op, assumed)) = crate::compose::IdcOp::of_line(
                                sub_tokens.iter().map(String::as_str),
                            ) {
                                let Some(line) = parse_compose_line(
                                    op,
                                    assumed,
                                    &sub_tokens[1 + usize::from(assumed)..],
                                    sub_comment,
                                    at_base.as_deref(),
                                ) else {
                                    break;
                                };
                                body.compose.push(line);
                                i += 1;
                                continue;
                            } else if sub_tokens.first().is_some_and(|t| t == "anchor") {
                                let point_parts = &sub_tokens[1..];
                                if point_parts.len() == 3
                                    && let Some(pt) = parse_anchor_point(
                                        &point_parts[0],
                                        &point_parts[1],
                                        &point_parts[2],
                                        body.scale,
                                        sub_comment,
                                    )
                                {
                                    body.points.push(pt);
                                    i += 1;
                                    continue;
                                }
                                break;
                            } else {
                                break;
                            }
                        }

                        item_line_starts.push(header_idx);
                        items.push(DocumentItem::Glyph { name, body });
                    }
                    "name-parts" | "remap" | "feature" | "assert" | "face" | "slice" | "prop" => {
                        item_line_starts.push(i);
                        items.push(DocumentItem::parse_directive(&tokens, comment));
                        i += 1;
                    }
                    "exists" => {
                        item_line_starts.push(i);
                        // Exactly one token: the pattern is a regex, and a
                        // second token would either be a second pattern (there
                        // is no conjunction) or a flag (there are none). Both
                        // are better said by `issues` than guessed at here.
                        if tokens.len() == 2 {
                            items.push(DocumentItem::Exists {
                                pattern: tokens[1].clone(),
                                comment,
                            });
                        } else {
                            items.push(DocumentItem::Directive(trimmed.to_string()));
                        }
                        i += 1;
                    }
                    "sample" => {
                        item_line_starts.push(i);
                        // A header the grammar cannot read keeps its own line
                        // as an opaque directive and claims nothing: its
                        // continuations fall through to the orphan branch on
                        // the next pass, so both halves of the mistake are
                        // reported instead of one being folded into the other.
                        let Some((label, sublabel, mode)) = parse_sample_header(&tokens[1..])
                        else {
                            items.push(DocumentItem::Directive(trimmed.to_string()));
                            i += 1;
                            continue;
                        };
                        i += 1;
                        // The continuations are the command's, not the
                        // document's: they are read here so that no later pass
                        // has to know a `sample` spans more than one line.
                        let mut raw: Vec<String> = Vec::new();
                        while let Some(DocLine::Text(t)) = lines.get(i) {
                            let Some(rest) = continuation_text(t) else {
                                break;
                            };
                            raw.push(rest.to_string());
                            i += 1;
                        }
                        items.push(DocumentItem::Sample {
                            label,
                            sublabel,
                            mode,
                            text: dedent_continuations(&raw),
                            comment,
                        });
                    }
                    "color" => {
                        item_line_starts.push(i);
                        if tokens.len() >= 4 && tokens[2] == "=" {
                            let visibility = match tokens.get(4).map(|s| s.as_str()) {
                                Some("coloronly") => Some(LayerVisibility::ColorOnly),
                                Some("monoonly") => Some(LayerVisibility::MonoOnly),
                                _ => None,
                            };
                            items.push(DocumentItem::Color {
                                name: tokens[1].clone(),
                                value: tokens[3].clone(),
                                visibility,
                                comment,
                            });
                        } else {
                            items.push(DocumentItem::Directive(trimmed.to_string()));
                        }
                        i += 1;
                    }
                    _ => {
                        item_line_starts.push(i);
                        items.push(DocumentItem::Directive(trimmed.to_string()));
                        i += 1;
                    }
                }
            }
        }
    }

    at_bases.resize(items.len(), base_here);
    DerivedItems {
        items,
        item_line_starts,
        at_bases,
        end: i.min(lines.len()),
    }
}

/// `LABEL [SUBLABEL] [: MODE...]` — the tokens of a `sample` line after the
/// keyword, or `None` if they do not read as one.
///
/// The mode tail is told from the labels by a bare `:` token, the same rule a
/// slice qualifier is told by; a label that is literally `:` is written
/// `` `:` `` like any other token holding punctuation. Nothing validates `MODE`
/// here — the words are kept as written, [`crate::samples::SampleMode`] reads
/// them and [`crate::issues`] is where one that names nothing is reported.
fn parse_sample_header<S: AsRef<str>>(
    parts: &[S],
) -> Option<(String, Option<String>, Vec<String>)> {
    let colon = parts.iter().position(|p| p.as_ref() == ":");
    let (labels, mode) = match colon {
        Some(at) => (
            &parts[..at],
            parts[at + 1..]
                .iter()
                .map(|p| p.as_ref().to_string())
                .collect(),
        ),
        None => (parts, Vec::new()),
    };
    match labels {
        [label] => Some((label.as_ref().to_string(), None, mode)),
        [label, sublabel] => Some((
            label.as_ref().to_string(),
            Some(sublabel.as_ref().to_string()),
            mode,
        )),
        _ => None,
    }
}
