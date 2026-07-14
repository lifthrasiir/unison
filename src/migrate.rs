use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::document::*;
use crate::document_io::{self, tokenize_tokens};
use crate::pixel::*;

const FILLED_CHARS: &str = "@b9Pd(u)n";

fn is_filled_char(ch: char) -> bool {
    FILLED_CHARS.contains(ch)
}

fn safe_char(lines: &[String], row: usize, col: usize) -> Option<char> {
    lines.get(row).and_then(|line| {
        if col < line.len() {
            Some(line.as_bytes()[col] as char)
        } else {
            None
        }
    })
}

fn resolve_pixel(
    lines: &[String],
    rr: usize,
    cc: usize,
    bbox: (usize, usize, usize, usize),
) -> Result<u8> {
    let (top, left, bottom, right) = bbox;
    if rr < top || rr > bottom || cc < left || cc > right {
        bail!("pixel ({rr},{cc}) outside bbox");
    }

    let px = lines[rr].as_bytes()[cc] as char;

    let t = lines[rr - 1].as_bytes()[cc] as char;
    let b = lines[rr + 1].as_bytes()[cc] as char;
    let l = lines[rr].as_bytes()[cc - 1] as char;
    let r_ch = lines[rr].as_bytes()[cc + 1] as char;

    let v: Option<u8>;

    if px == '@' {
        v = Some(PX_ALMOSTFULL);
    } else if px == '*' {
        v = Some(PX_DOT);
    } else if px == '.' || px == '!' || px == '+' {
        v = Some(PX_EMPTY);
    } else if px == 'b' {
        let rcont = r_ch == '\\' && !(safe_char(lines, rr, cc + 2).is_some_and(is_filled_char)
                                     && is_filled_char(safe_char(lines, rr - 1, cc + 1).unwrap_or('.')));
        let tcont = t == '\\' && !(safe_char(lines, rr - 2, cc).is_some_and(is_filled_char)
                                  && is_filled_char(safe_char(lines, rr - 1, cc + 1).unwrap_or('.')));
        v = Some(if tcont {
            if rcont { bail!("ambiguous 'b' pixel at ({rr},{cc})") } else { PX_HALFSLANT1H }
        } else if rcont {
            PX_HALFSLANT1V
        } else {
            PX_HALF1
        });
    } else if px == '9' {
        let lcont = l == '\\' && !(safe_char(lines, rr, cc.wrapping_sub(2)).is_some_and(is_filled_char)
                                  && is_filled_char(safe_char(lines, rr + 1, cc - 1).unwrap_or('.')));
        let bcont = b == '\\' && !(safe_char(lines, rr + 2, cc).is_some_and(is_filled_char)
                                  && is_filled_char(safe_char(lines, rr + 1, cc - 1).unwrap_or('.')));
        v = Some(if bcont {
            if lcont { bail!("ambiguous '9' pixel at ({rr},{cc})") } else { PX_HALFSLANT2H }
        } else if lcont {
            PX_HALFSLANT2V
        } else {
            PX_HALF2
        });
    } else if px == 'P' {
        let rcont = r_ch == '/' && !(safe_char(lines, rr, cc + 2).is_some_and(is_filled_char)
                                    && is_filled_char(safe_char(lines, rr + 1, cc + 1).unwrap_or('.')));
        let bcont = b == '/' && !(safe_char(lines, rr + 2, cc).is_some_and(is_filled_char)
                                 && is_filled_char(safe_char(lines, rr + 1, cc + 1).unwrap_or('.')));
        v = Some(if bcont {
            if rcont { bail!("ambiguous 'P' pixel at ({rr},{cc})") } else { PX_HALFSLANT3H }
        } else if rcont {
            PX_HALFSLANT3V
        } else {
            PX_HALF3
        });
    } else if px == 'd' {
        let lcont = l == '/' && !(safe_char(lines, rr, cc.wrapping_sub(2)).is_some_and(is_filled_char)
                                 && is_filled_char(safe_char(lines, rr - 1, cc - 1).unwrap_or('.')));
        let tcont = t == '/' && !(safe_char(lines, rr - 2, cc).is_some_and(is_filled_char)
                                 && is_filled_char(safe_char(lines, rr - 1, cc - 1).unwrap_or('.')));
        v = Some(if tcont {
            if lcont { bail!("ambiguous 'd' pixel at ({rr},{cc})") } else { PX_HALFSLANT4H }
        } else if lcont {
            PX_HALFSLANT4V
        } else {
            PX_HALF4
        });
    } else {
        let tfull = is_filled_char(t);
        let bfull = is_filled_char(b);
        let lfull = is_filled_char(l);
        let rfull = is_filled_char(r_ch);

        v = if px == '\\' {
            if (lfull || bfull) && !(rfull && tfull) {
                Some(if b == 'b' {
                    if l == 'b' { bail!("ambiguous '\\' pixel at ({rr},{cc})") } else { PX_SLANT1H }
                } else if l == 'b' {
                    PX_SLANT1V
                } else {
                    PX_HALF1
                })
            } else if !(lfull && bfull) && (rfull || tfull) {
                Some(if t == '9' {
                    if r_ch == '9' { bail!("ambiguous '\\' pixel at ({rr},{cc})") } else { PX_SLANT2H }
                } else if r_ch == '9' {
                    PX_SLANT2V
                } else {
                    PX_HALF2
                })
            } else {
                None
            }
        } else if px == '/' {
            if (lfull || tfull) && !(rfull && bfull) {
                Some(if t == 'P' {
                    if l == 'P' { bail!("ambiguous '/' pixel at ({rr},{cc})") } else { PX_SLANT3H }
                } else if l == 'P' {
                    PX_SLANT3V
                } else {
                    PX_HALF3
                })
            } else if !(lfull && tfull) && (rfull || bfull) {
                Some(if b == 'd' {
                    if r_ch == 'd' { bail!("ambiguous '/' pixel at ({rr},{cc})") } else { PX_SLANT4H }
                } else if r_ch == 'd' {
                    PX_SLANT4V
                } else {
                    PX_HALF4
                })
            } else {
                None
            }
        } else if px == '>' || px == ')' {
            if lfull && !(rfull || tfull || bfull) {
                Some(PX_QUAD1)
            } else if !lfull && (rfull || tfull || bfull) {
                Some(PX_INVQUAD1)
            } else {
                None
            }
        } else if px == 'v' || px == 'u' {
            if tfull && !(lfull || rfull || bfull) {
                Some(PX_QUAD2)
            } else if !tfull && (lfull || rfull || bfull) {
                Some(PX_INVQUAD2)
            } else {
                None
            }
        } else if px == '<' || px == '(' {
            if rfull && !(lfull || tfull || bfull) {
                Some(PX_QUAD3)
            } else if !rfull && (lfull || tfull || bfull) {
                Some(PX_INVQUAD3)
            } else {
                None
            }
        } else if px == '^' || px == 'n' {
            if bfull && !(lfull || rfull || tfull) {
                Some(PX_QUAD4)
            } else if !bfull && (lfull || rfull || tfull) {
                Some(PX_INVQUAD4)
            } else {
                None
            }
        } else {
            bail!("unknown pixel char '{px}' at ({rr},{cc})")
        };
    }

    let v = v.ok_or_else(|| anyhow::anyhow!("ambiguous pixel '{px}' at ({rr},{cc})"))?;

    let filled = is_filled_char(px);
    Ok(v | if filled { PX_FULL } else { 0 })
}

fn parse_pixels(
    lines: &[String],
    bbox: (usize, usize, usize, usize),
) -> Result<PixelGrid> {
    let line_len = lines.first().map_or(1, |l| l.len()) + 1;
    let sentinel_row = ".".repeat(line_len);

    let mut padded: Vec<String> = Vec::with_capacity(lines.len() + 1);
    padded.push(sentinel_row);
    for line in lines {
        padded.push(format!(".{line}"));
    }

    let adjusted_bbox = (bbox.0 + 1, bbox.1 + 1, bbox.2 + 1, bbox.3 + 1);

    let (top, left, bottom, right) = adjusted_bbox;
    let width = (right - left + 1) as u16;
    let height = (bottom - top + 1) as u16;
    let mut grid = PixelGrid::new(width, height);

    for rr in top..=bottom {
        for cc in left..=right {
            let px_val = resolve_pixel(&padded, rr, cc, adjusted_bbox)?;
            let shape = PixelShape(px_val);
            grid.set((rr - top) as u16, (cc - left) as u16, shape);
        }
    }

    Ok(grid)
}

// ---------------------------------------------------------------------------
// PIXELNAMES — explicit pixel spec mapping from process.py's `pixel` command
// ---------------------------------------------------------------------------

fn pixelname_to_value(spec: &str) -> Option<u8> {
    match spec {
        "." => Some(PX_EMPTY),
        "," => Some(PX_FULL | PX_EMPTY),
        "O" => Some(PX_ALMOSTFULL),
        "@" => Some(PX_FULL | PX_ALMOSTFULL),
        "|\\" => Some(PX_HALF1),
        "|b" => Some(PX_FULL | PX_HALF1),
        "\\|" => Some(PX_HALF2),
        "9|" => Some(PX_FULL | PX_HALF2),
        "|/" => Some(PX_HALF3),
        "|P" => Some(PX_FULL | PX_HALF3),
        "/|" => Some(PX_HALF4),
        "d|" => Some(PX_FULL | PX_HALF4),
        "|>" => Some(PX_QUAD1),
        "|)" => Some(PX_FULL | PX_QUAD1),
        "'v'" => Some(PX_QUAD2),
        "'u'" => Some(PX_FULL | PX_QUAD2),
        "<|" => Some(PX_QUAD3),
        "(|" => Some(PX_FULL | PX_QUAD3),
        ".^." => Some(PX_QUAD4),
        ".n." => Some(PX_FULL | PX_QUAD4),
        "+" => Some(PX_DOT),
        "*" => Some(PX_FULL | PX_DOT),
        ">|" => Some(PX_INVQUAD1),
        ")|" => Some(PX_FULL | PX_INVQUAD1),
        "|v|" => Some(PX_INVQUAD2),
        "|u|" => Some(PX_FULL | PX_INVQUAD2),
        "|<" => Some(PX_INVQUAD3),
        "|(" => Some(PX_FULL | PX_INVQUAD3),
        "|^|" => Some(PX_INVQUAD4),
        "|n|" => Some(PX_FULL | PX_INVQUAD4),
        "|\\."|"i\\" => Some(PX_SLANT1H),
        "|b." => Some(PX_FULL | PX_SLANT1H),
        "'\\|" => Some(PX_SLANT2H),
        "'9|" => Some(PX_FULL | PX_SLANT2H),
        "|/'" => Some(PX_SLANT3H),
        "|P'" => Some(PX_FULL | PX_SLANT3H),
        "./|" => Some(PX_SLANT4H),
        ".d|" => Some(PX_FULL | PX_SLANT4H),
        "i\\." => Some(PX_SLANT1V),
        "ib." => Some(PX_FULL | PX_SLANT1V),
        "'\\!" => Some(PX_SLANT2V),
        "'9!" => Some(PX_FULL | PX_SLANT2V),
        "!/'" => Some(PX_SLANT3V),
        "!P'" => Some(PX_FULL | PX_SLANT3V),
        "./i" | "/i" => Some(PX_SLANT4V),
        ".di" => Some(PX_FULL | PX_SLANT4V),
        "|_\\" => Some(PX_HALFSLANT1H),
        "|_b" => Some(PX_FULL | PX_HALFSLANT1H),
        "\\_|" => Some(PX_HALFSLANT2H),
        "9_|" => Some(PX_FULL | PX_HALFSLANT2H),
        "|_/" => Some(PX_HALFSLANT3H),
        "|_P" => Some(PX_FULL | PX_HALFSLANT3H),
        "/_|" => Some(PX_HALFSLANT4H),
        "d_|" => Some(PX_FULL | PX_HALFSLANT4H),
        "|\\i" => Some(PX_HALFSLANT1V),
        "|bi" => Some(PX_FULL | PX_HALFSLANT1V),
        "!\\|" => Some(PX_HALFSLANT2V),
        "!9|" => Some(PX_FULL | PX_HALFSLANT2V),
        "|/!" => Some(PX_HALFSLANT3V),
        "|P!" => Some(PX_FULL | PX_HALFSLANT3V),
        "i/|" => Some(PX_HALFSLANT4V),
        "id|" => Some(PX_FULL | PX_HALFSLANT4V),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// name-parts expansion
// ---------------------------------------------------------------------------

fn collect_name_parts(content: &str) -> Result<HashMap<String, Vec<String>>> {
    let mut parts_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut pending_name: Option<String> = None;
    let mut pending_tokens: Vec<String> = Vec::new();

    for line in content.lines() {
        let comment_stripped = if let Some(idx) = line.find("//") {
            &line[..idx]
        } else {
            line
        };
        let trimmed = comment_stripped.trim();

        if trimmed.starts_with("name-parts ") || trimmed.starts_with("name-parts\t") {
            // Flush previous
            if let Some(name) = pending_name.take() {
                let resolved = resolve_parts_tokens(&pending_tokens, &parts_map)?;
                parts_map.insert(name, resolved);
                pending_tokens.clear();
            }

            let args = tokenize_tokens(trimmed).unwrap_or_default();
            if args.len() < 4 || args[2] != "=" {
                bail!("invalid name-parts: {trimmed}");
            }
            pending_name = Some(args[1].to_string());
            let rest_tokens = &args[3..];
            if rest_tokens.last().is_some_and(|s| s == "..") {
                pending_tokens.extend_from_slice(&rest_tokens[..rest_tokens.len() - 1]);
            } else {
                pending_tokens.extend_from_slice(rest_tokens);
                let name = pending_name.take().unwrap();
                let resolved = resolve_parts_tokens(&pending_tokens, &parts_map)?;
                parts_map.insert(name, resolved);
                pending_tokens.clear();
            }
        } else if pending_name.is_some() && trimmed.starts_with('\t') || (pending_name.is_some() && line.starts_with('\t')) {
            // Continuation line for name-parts (tab-indented)
            let cont = line.trim();
            let cont = if let Some(idx) = cont.find("//") {
                cont[..idx].trim()
            } else {
                cont
            };
            let tokens = tokenize_tokens(cont).unwrap_or_default();
            if tokens.last().is_some_and(|s| s == "..") {
                pending_tokens.extend_from_slice(&tokens[..tokens.len() - 1]);
            } else {
                pending_tokens.extend_from_slice(&tokens);
                let name = pending_name.take().unwrap();
                let resolved = resolve_parts_tokens(&pending_tokens, &parts_map)?;
                parts_map.insert(name, resolved);
                pending_tokens.clear();
            }
        } else if pending_name.is_some() && !trimmed.is_empty() {
            // Non-continuation, non-empty line ends the pending name-parts
            let name = pending_name.take().unwrap();
            let resolved = resolve_parts_tokens(&pending_tokens, &parts_map)?;
            parts_map.insert(name, resolved);
            pending_tokens.clear();
        }
    }
    if let Some(name) = pending_name.take() {
        let resolved = resolve_parts_tokens(&pending_tokens, &parts_map)?;
        parts_map.insert(name, resolved);
    }
    Ok(parts_map)
}

fn resolve_parts_tokens(
    tokens: &[String],
    existing: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>> {
    let mut result = Vec::new();
    for token in tokens {
        if token.starts_with('$') {
            if let Some(referenced) = existing.get(token.as_str()) {
                result.extend(referenced.iter().cloned());
            } else {
                bail!("unknown name-parts reference: {token}");
            }
        } else {
            // Split by | and handle *N repeats
            for part in token.split('|') {
                if let Some((name, rep_str)) = part.rsplit_once('*') {
                    let rep: usize = rep_str
                        .parse()
                        .map_err(|_| anyhow::anyhow!("invalid repeat count: {rep_str}"))?;
                    for _ in 0..rep {
                        result.push(name.to_string());
                    }
                } else {
                    result.push(part.to_string());
                }
            }
        }
    }
    Ok(result)
}


// ---------------------------------------------------------------------------
// Map char representation helpers
// ---------------------------------------------------------------------------

fn char_to_codepoint(s: &str) -> Option<u32> {
    if let Some(hex) = s.strip_prefix("U+").or_else(|| s.strip_prefix("u+")) {
        u32::from_str_radix(hex, 16).ok()
    } else {
        let mut chars = s.chars();
        let c = chars.next()?;
        if chars.next().is_none() {
            Some(c as u32)
        } else {
            None
        }
    }
}

fn parse_map_char_to_repr(s: &str) -> String {
    // If it's a single Unicode character, keep it as-is
    // If it's U+XXXX, keep as-is
    s.to_string()
}

/// Parse a subglyph spec token (e.g. "a-upper", "dia-upper@vv", "sm-a@X")
/// Returns (name, row_offset, col_offset, placeholder, negated, explicitly positioned).
fn parse_subglyph_offset(s: &str) -> (String, i16, i16, Option<char>, bool, bool) {
    let (s, explicit) = if let Some(stripped) = s.strip_prefix('-') {
        (stripped, true)
    } else {
        (s, false)
    };
    let (s, negated) = if let Some(stripped) = s.strip_suffix("!negate") {
        (stripped, true)
    } else {
        (s, false)
    };
    if let Some((name, at_part)) = s.rsplit_once('@') {
        if !name.is_empty() && !at_part.is_empty() && at_part.chars().all(|c| "^v<>".contains(c)) {
            let roff = at_part.matches('v').count() as i16
                - at_part.matches('^').count() as i16;
            let coff = at_part.matches('>').count() as i16
                - at_part.matches('<').count() as i16;
            return (name.to_string(), roff, coff, None, negated, true);
        }
        if !name.is_empty() && at_part.len() == 1 {
            let ch = at_part.chars().next().unwrap();
            if ch.is_ascii_uppercase() {
                return (name.to_string(), 0, 0, Some(ch), negated, true);
            }
        }
    }
    (s.to_string(), 0, 0, None, negated, explicit)
}

fn glyph_ref_offset(row: i16, col: i16, explicit: bool) -> Option<(i16, i16)> {
    if explicit || row != 0 || col != 0 {
        Some((col, row))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Main file conversion
// ---------------------------------------------------------------------------

#[derive(Default)]
struct DefaultState {
    sticky: bool,
    advance: Option<u16>,
    left: Option<i16>,
}


struct GlyphAccum {
    name: String,
    pixel_lines: Vec<String>,
    row_marks: HashMap<char, usize>,
    col_marks: HashMap<char, usize>,
    points: Vec<(String, String, usize)>, // (position, mark_str, row_in_grid)
    question_marks: Vec<(usize, usize)>,  // (row, col) of ? in pixel grid
    pixel_specs: Vec<u8>,                 // explicit pixel values for ?
    specs: Vec<String>,                   // subglyph specs for = syntax
    sticky: bool,
    advance: Option<u16>,
    left: Option<i16>,
    is_inline: bool,
}

pub fn migrate_file(
    content: &str,
    _name_parts: &HashMap<String, Vec<String>>,
    default_offsets: &HashMap<String, (i16, i16)>,
) -> Result<Vec<DocumentItem>> {
    let mut items: Vec<DocumentItem> = Vec::new();
    let mut defaults = DefaultState::default();
    let mut current_glyph: Option<GlyphAccum> = None;
    let mut prev_continuation: Vec<String> = Vec::new();
    let mut in_continuation = false;
    let mut pending_directive: Option<Vec<String>> = None;
    let mut pending_map_chars: Option<String> = None;

    let flush_glyph = |accum: GlyphAccum, items: &mut Vec<DocumentItem>| -> Result<()> {
        if accum.is_inline && accum.specs.is_empty() && accum.pixel_lines.is_empty() {
            return Ok(());
        }

        let glyph_name = GlyphName(accum.name.clone());

        // Case 1: composite glyph (has specs from `= spec1 spec2`)
        if !accum.specs.is_empty() && accum.pixel_lines.is_empty() {
            let refs: Vec<GlyphRef> = accum
                .specs
                .iter()
                .map(|spec| {
                    let (name, roff, coff, _, negated, explicit) =
                        parse_subglyph_offset(spec);
                    let def_off = default_offsets.get(&name).copied().unwrap_or((0, 0));
                    let row = roff + def_off.0;
                    let col = coff + def_off.1;
                    let offset = glyph_ref_offset(row, col, explicit);
                    GlyphRef { name, offset, negated, fill: None }
                })
                .collect();

            items.push(DocumentItem::Glyph {
                name: glyph_name,
                body: GlyphBody {
                    refs,
                    sticky: accum.sticky,
                    inline: accum.is_inline,
                    advance: accum.advance,
                    left: accum.left,
                    ..GlyphBody::new()
                },
            });
            return Ok(());
        }

        // Case 2: pixel glyph
        if accum.pixel_lines.is_empty() {
            // Ref-only with no specs and no pixels — skip
            return Ok(());
        }

        // Identify placeholder characters from specs (e.g. spec "foo@X" → placeholder 'X')
        let placeholders: std::collections::HashSet<char> = accum
            .specs
            .iter()
            .filter_map(|spec| {
                let (_, _, _, ph, _, _) = parse_subglyph_offset(spec);
                ph
            })
            .collect();

        let mut placeholder_bboxes: HashMap<char, (usize, usize, usize, usize)> = HashMap::new();

        let mut question_idx = 0usize;
        let mut processed_lines: Vec<String> = Vec::new();
        for (r, line) in accum.pixel_lines.iter().enumerate() {
            let mut new_line = String::with_capacity(line.len());
            for (c, ch) in line.chars().enumerate() {
                if ch == '?' {
                    // Use a filled substitute if the explicit pixel value is filled
                    let is_filled = accum.pixel_specs.get(question_idx)
                        .is_some_and(|v| v & PX_FULL != 0);
                    new_line.push(if is_filled { '@' } else { '!' });
                    question_idx += 1;
                } else if placeholders.contains(&ch) {
                    let e = placeholder_bboxes.entry(ch).or_insert((r, c, r, c));
                    e.0 = e.0.min(r);
                    e.1 = e.1.min(c);
                    e.2 = e.2.max(r);
                    e.3 = e.3.max(c);
                    new_line.push('!');
                } else {
                    new_line.push(ch);
                }
            }
            new_line.push('.');
            processed_lines.push(new_line);
        }
        if let Some(first) = processed_lines.first() {
            processed_lines.push(".".repeat(first.len()));
        }

        // Compute bbox (all content, not just filled)
        let mut bbox: Option<(usize, usize, usize, usize)> = None;
        for (r, line) in accum.pixel_lines.iter().enumerate() {
            for (c, px) in line.chars().enumerate() {
                if px != '+' {
                    bbox = Some(match bbox {
                        Some((bt, bl, bb, br)) => (bt.min(r), bl.min(c), bb.max(r), br.max(c)),
                        None => (r, c, r, c),
                    });
                }
            }
        }

        let Some(bbox) = bbox else {
            // Empty glyph — still emit with empty grid
            let width = accum.pixel_lines.first().map_or(0, |l| l.len()) as u16;
            let height = accum.pixel_lines.len() as u16;
            let grid = PixelGrid::new(width, height);
            items.push(DocumentItem::Glyph {
                name: glyph_name,
                body: GlyphBody {
                    pixels: Some(grid),
                    sticky: accum.sticky,
                    advance: accum.advance,
                    left: accum.left,
                    ..GlyphBody::new()
                },
            });
            return Ok(());
        };

        let mut grid = parse_pixels(&processed_lines, bbox)?;

        // Overlay explicit pixel values for ? positions
        let roff = bbox.0;
        let coff = bbox.1;
        for (i, &(qr, qc)) in accum.question_marks.iter().enumerate() {
            if i < accum.pixel_specs.len() {
                let gr = qr as i16 - roff as i16;
                let gc = qc as i16 - coff as i16;
                if gr >= 0 && gc >= 0 {
                    grid.set(gr as u16, gc as u16, PixelShape(accum.pixel_specs[i]));
                }
            }
        }

        // Build points
        let mut points = Vec::new();
        for (position, mark_str, row_idx) in &accum.points {
            let (col, row) = resolve_point_marks(
                mark_str,
                *row_idx,
                &accum.col_marks,
                &accum.row_marks,
            );
            points.push(GlyphPoint {
                position: position.clone(),
                col: col - coff as i16,
                row: row - roff as i16,
                col_end: col - coff as i16,
                row_end: row - roff as i16,
            });
        }

        // Build refs from specs if any
        let roff = bbox.0 as i16;
        let coff_base = bbox.1 as i16;
        let make_ref =
            |row: i16, col: i16, name: String, negated: bool, explicit: bool| -> GlyphRef {
            let offset = glyph_ref_offset(row, col, explicit);
            GlyphRef { name, offset, negated, fill: None }
        };
        let refs: Vec<GlyphRef> = accum
            .specs
            .iter()
            .map(|spec| {
                let (name, spec_roff, spec_coff, ph, negated, explicit) =
                    parse_subglyph_offset(spec);
                if let Some(ph) = ph
                    && let Some(&(pt, pl, _, _)) = placeholder_bboxes.get(&ph) {
                        let row = pt as i16 - roff + spec_roff;
                        let col = pl as i16 - coff_base + spec_coff;
                        return make_ref(row, col, name, negated, explicit);
                    }
                let def_off = default_offsets.get(&name).copied().unwrap_or((0, 0));
                let row = spec_roff + def_off.0;
                let col = spec_coff + def_off.1;
                make_ref(row, col, name, negated, explicit)
            })
            .collect();

        items.push(DocumentItem::Glyph {
            name: glyph_name,
            body: GlyphBody {
                pixels: Some(grid),
                refs,
                points,
                sticky: accum.sticky,
                inline: accum.is_inline,
                advance: accum.advance,
                left: accum.left,
            },
        });
        Ok(())
    };

    for line in content.lines() {
        // Strip comments from code portion
        let comment_start = line.find("//");
        let code_part = if let Some(idx) = comment_start {
            &line[..idx]
        } else {
            line
        };
        let comment_part = comment_start.map(|idx| &line[idx + 2..]);

        // Handle tab-indented continuation lines
        if line.starts_with('\t') && (in_continuation || pending_directive.is_some()) {
            let trimmed = code_part.trim();
            let tokens: Vec<String> = tokenize_tokens(trimmed).unwrap_or_default();
            let target = if let Some(ref mut pd) = pending_directive {
                pd
            } else {
                &mut prev_continuation
            };
            if tokens.last().is_some_and(|s| s == "..") {
                target.extend_from_slice(&tokens[..tokens.len() - 1]);
            } else {
                target.extend_from_slice(&tokens);
                in_continuation = false;
                // Directive continuation is complete
                if let Some(tokens) = pending_directive.take() {
                    items.push(DocumentItem::parse_directive(&tokens));
                }
            }
            continue;
        }

        // If we had continuation lines and now hit a non-continuation,
        // the previous glyph's specs should already be set
        if in_continuation || !prev_continuation.is_empty() {
            in_continuation = false;
            if let Some(char_str) = pending_map_chars.take() {
                let specs = std::mem::take(&mut prev_continuation);
                expand_map_items(&char_str, &specs, &mut items, default_offsets)?;
            } else if let Some(ref mut accum) = current_glyph {
                accum.specs = std::mem::take(&mut prev_continuation);
            } else {
                prev_continuation.clear();
            }
        }
        if let Some(tokens) = pending_directive.take() {
            items.push(DocumentItem::parse_directive(&tokens));
        }

        let code_trimmed = code_part.trim();

        // name-parts continuation lines (tab-indented, outside a glyph)
        if line.starts_with('\t') && current_glyph.is_none() {
            continue;
        }

        // Empty line
        if code_trimmed.is_empty() {
            if let Some(accum) = current_glyph.take() {
                flush_glyph(accum, &mut items)?;
            }
            if let Some(comment) = comment_part {
                if comment.is_empty() && line.starts_with("//") {
                    items.push(DocumentItem::Comment(String::new()));
                } else {
                    items.push(DocumentItem::Comment(comment.to_string()));
                }
            } else {
                items.push(DocumentItem::BlankLine);
            }
            continue;
        }

        // Full-line comment
        if line.trim().starts_with("//") {
            if let Some(accum) = current_glyph.take() {
                flush_glyph(accum, &mut items)?;
            }
            let comment_text = line.trim().strip_prefix("//").unwrap_or("");
            items.push(DocumentItem::Comment(comment_text.to_string()));
            continue;
        }

        let args: Vec<String> = tokenize_tokens(code_trimmed).unwrap_or_default();
        if args.is_empty() {
            continue;
        }

        match args[0].as_str() {
            "glyph" => {
                if let Some(accum) = current_glyph.take() {
                    flush_glyph(accum, &mut items)?;
                }

                if args.len() < 2 {
                    bail!("empty glyph name");
                }

                let raw_name = &args[1];

                // Check for `= ...` syntax
                let eq_pos = args.iter().position(|a| a == "=");
                let (glyph_flags, specs) = if let Some(ep) = eq_pos {
                    // Flags between name and =
                    let flag_tokens = &args[2..ep];
                    let spec_tokens = &args[ep + 1..];
                    (flag_tokens, spec_tokens)
                } else {
                    (&args[2..], &[][..])
                };

                let mut sticky = defaults.sticky;
                let advance = defaults.advance;
                let left = defaults.left;
                let mut is_inline = false;

                for flag in glyph_flags {
                    match flag.as_str() {
                        "sticky" => sticky = true,
                        "inline" => is_inline = true,
                        _ => {}
                    }
                }

                let has_continuation = specs.last().is_some_and(|s| s == "..");
                let specs_vec: Vec<String> = if has_continuation {
                    prev_continuation = specs[..specs.len() - 1].to_vec();
                    in_continuation = true;
                    Vec::new()
                } else {
                    specs.to_vec()
                };

                current_glyph = Some(GlyphAccum {
                    name: raw_name.to_lowercase(),
                    pixel_lines: Vec::new(),
                    row_marks: HashMap::new(),
                    col_marks: HashMap::new(),
                    points: Vec::new(),
                    question_marks: Vec::new(),
                    pixel_specs: Vec::new(),
                    specs: specs_vec,
                    sticky,
                    advance,
                    left,
                    is_inline,
                });
            }

            "map" => {
                if let Some(accum) = current_glyph.take() {
                    flush_glyph(accum, &mut items)?;
                }

                if args.len() < 4 || args[2] != "=" {
                    bail!("invalid map syntax: {code_trimmed}");
                }

                let char_str = &args[1];

                let rhs = &args[3..];
                let has_continuation = rhs.last().is_some_and(|s| s == "..");
                if has_continuation {
                    prev_continuation = rhs[..rhs.len() - 1].to_vec();
                    in_continuation = true;
                    pending_map_chars = Some(char_str.to_string());
                } else if rhs.len() == 1 && !rhs[0].contains('@') {
                    // Simple map
                    items.push(DocumentItem::Map {
                        char_repr: parse_map_char_to_repr(char_str),
                        glyph: rhs[0].to_lowercase(),
                    });
                } else {
                    // Composite map — decompose
                    emit_map_items(char_str, rhs, &mut items, default_offsets)?;
                }
            }

            "default" => {
                if let Some(accum) = current_glyph.take() {
                    flush_glyph(accum, &mut items)?;
                }

                if args.len() >= 2 {
                    match args[1].as_str() {
                        "+sticky" => defaults.sticky = true,
                        "-sticky" => defaults.sticky = false,
                        "+advance" => {
                            if args.len() >= 3 {
                                defaults.advance = args[2].parse().ok();
                            }
                        }
                        "-advance" => defaults.advance = None,
                        "+left" => {
                            if args.len() >= 3 {
                                defaults.left = args[2].parse().ok();
                            }
                        }
                        "-left" => defaults.left = None,
                        _ => {}
                    }
                }
            }

            "name-parts" | "exclude-from-sample" | "remap" | "feature" => {
                if let Some(accum) = current_glyph.take() {
                    flush_glyph(accum, &mut items)?;
                }
                if args.last().is_some_and(|s| s == "..") {
                    pending_directive = Some(args[..args.len() - 1].to_vec());
                } else if args[0] == "feature" {
                    // Old format: feature NAME : REMAP_GROUP
                    // Convert to: feature NAME for hang : REMAP_GROUP
                    if args.len() >= 4 && args[2] == ":" {
                        items.push(DocumentItem::Feature {
                            name: args[1].to_string(),
                            scripts: vec!["hang".to_string()],
                            remap_group: args[3].to_string(),
                        });
                    } else {
                        items.push(DocumentItem::parse_directive(&args));
                    }
                } else {
                    items.push(DocumentItem::parse_directive(&args));
                }
            }

            "font-meta" => {
                if let Some(accum) = current_glyph.take() {
                    flush_glyph(accum, &mut items)?;
                }
                let rest = code_trimmed.strip_prefix("font-meta ").unwrap_or("");
                items.push(DocumentItem::FontMeta(rest.to_string()));
            }

            // Standalone `sticky` or `inline` command inside a glyph
            "sticky" if current_glyph.is_some() => {
                current_glyph.as_mut().unwrap().sticky = true;
            }

            "inline" if current_glyph.is_some() => {
                current_glyph.as_mut().unwrap().is_inline = true;
            }

            _ => {
                // Should be a pixel line for the current glyph
                if let Some(ref mut accum) = current_glyph {
                    // Parse pixel row: first token is pixel data, then optional
                    // row mark (single char), then optional commands (point, pixel, etc.)
                    //
                    // However, pixel rows in the new format can contain tab-separated
                    // annotations. Use tab as the primary separator.
                    // Strip comments first, then split by tabs.
                    let line_no_comment = if let Some(idx) = line.find(" //") {
                        &line[..idx]
                    } else {
                        line
                    };
                    let tab_parts: Vec<&str> = line_no_comment.split('\t').collect();
                    let pixel_part_raw = tab_parts[0].trim();

                    // Parse the pixel data line. Format can be:
                    //   PIXELDATA
                    //   PIXELDATA ROWMARK
                    //   PIXELDATA ROWMARK pixel SPEC1 SPEC2
                    //   PIXELDATA pixel SPEC1 SPEC2
                    //   PIXELDATA ROWMARK point DIRECTION@MARKS
                    // Where PIXELDATA is always the first token and ROWMARK is a single char.
                    let pixel_words: Vec<&str> = pixel_part_raw.split_whitespace().collect();
                    
                    let mut row_mark = None;
                    let mut extra_cmd_start = 1; // index into pixel_words where commands begin

                    if pixel_words.is_empty() {
                        continue;
                    }
                    let pixel_data = pixel_words[0].to_string();

                    // Check if second token is a row mark (single char) or a command
                    if pixel_words.len() >= 2 {
                        let second = pixel_words[1];
                        if second.chars().count() == 1 && second != "pixel" && second != "point" && second != "inline" && second != "sticky" {
                            row_mark = Some(second.chars().next().unwrap());
                            extra_cmd_start = 2;
                        } else {
                            extra_cmd_start = 1;
                        }
                    }

                    let mut point_cmds: Vec<(&str, &str)> = Vec::new();
                    let mut pixel_cmd_specs: Vec<&str> = Vec::new();

                    // Parse commands from pixel_words after pixel_data and optional mark
                    {
                        let mut wi = extra_cmd_start;
                        while wi < pixel_words.len() {
                            match pixel_words[wi] {
                                "point" => {
                                    if wi + 1 < pixel_words.len() {
                                        point_cmds.push(("point", pixel_words[wi + 1]));
                                        wi += 2;
                                    } else {
                                        wi += 1;
                                    }
                                }
                                "pixel" => {
                                    wi += 1;
                                    while wi < pixel_words.len() && pixel_words[wi] != "point" {
                                        pixel_cmd_specs.push(pixel_words[wi]);
                                        wi += 1;
                                    }
                                }
                                "inline" => {
                                    accum.is_inline = true;
                                    wi += 1;
                                }
                                "sticky" => {
                                    accum.sticky = true;
                                    wi += 1;
                                }
                                _ => {
                                    wi += 1;
                                }
                            }
                        }
                    }

                    // Collect point/pixel commands from tab-separated parts

                    for &tab_part in &tab_parts[1..] {
                        let tp = tab_part.trim();
                        // Could start with a row mark char then tab-separated commands
                        let words: Vec<&str> = tp.split_whitespace().collect();
                        let mut wi = 0;
                        // Check if first word is a single char (row mark on the
                        // tab-separated portion)
                        if !words.is_empty()
                            && words[0].chars().count() == 1
                            && words[0].chars().next().is_some_and(|c| c.is_ascii_uppercase())
                        {
                            // This is a row mark
                            let _mark_char = words[0].chars().next().unwrap();
                            if row_mark.is_none() {
                                // Use this as the row mark (will set below)
                            }
                            wi = 1;
                        }
                        while wi < words.len() {
                            match words[wi] {
                                "point" => {
                                    if wi + 1 < words.len() {
                                        point_cmds.push(("point", words[wi + 1]));
                                        wi += 2;
                                    } else {
                                        wi += 1;
                                    }
                                }
                                "pixel" => {
                                    wi += 1;
                                    while wi < words.len() && words[wi] != "point" {
                                        pixel_cmd_specs.push(words[wi]);
                                        wi += 1;
                                    }
                                }
                                "inline" => {
                                    accum.is_inline = true;
                                    wi += 1;
                                }
                                "sticky" => {
                                    accum.sticky = true;
                                    wi += 1;
                                }
                                _ => {
                                    wi += 1;
                                }
                            }
                        }
                    }

                    // Handle mark = '=' row (column marks definition)
                    if row_mark == Some('=') {
                        for (c, ch) in pixel_data.chars().enumerate() {
                            if ch != '=' {
                                accum.col_marks.insert(ch, c);
                            }
                        }
                        // Don't add to pixel lines
                        continue;
                    }

                    let grid_row = accum.pixel_lines.len();

                    // Record row mark
                    if let Some(mark) = row_mark {
                        accum.row_marks.insert(mark, grid_row);
                    }

                    // Also check tab_parts for marks: "........ A\t..."
                    // where A is not the row_mark from pixel_words
                    // This is already handled above.

                    // Record ? positions
                    for (c, ch) in pixel_data.chars().enumerate() {
                        if ch == '?' {
                            accum.question_marks.push((grid_row, c));
                        }
                    }

                    // Record pixel specs
                    for spec in &pixel_cmd_specs {
                        if let Some(val) = pixelname_to_value(spec) {
                            accum.pixel_specs.push(val);
                        }
                    }

                    // Record point annotations
                    for &(_, pointspec) in &point_cmds {
                        if let Some((posname, mark_str)) = pointspec.split_once('@') {
                            accum.points.push((
                                posname.to_string(),
                                mark_str.to_string(),
                                grid_row,
                            ));
                        }
                    }

                    accum.pixel_lines.push(pixel_data);
                } else {
                    // Not in a glyph — could be a single-char glyph name
                    // from the old format (a bare character line). In the
                    // new format this shouldn't happen, but be safe.
                    if code_trimmed.chars().count() == 1 {
                        // Single character on a line — probably a glyph definition
                        // in the new format this shouldn't happen, skip
                    }
                    // Otherwise it's an unrecognized line
                }
            }
        }
    }

    if !prev_continuation.is_empty() || in_continuation {
        if let Some(char_str) = pending_map_chars.take() {
            expand_map_items(&char_str, &prev_continuation, &mut items, default_offsets)?;
            prev_continuation.clear();
        } else if let Some(ref mut accum) = current_glyph {
            accum.specs = std::mem::take(&mut prev_continuation);
        }
    }
    if let Some(accum) = current_glyph.take() {
        flush_glyph(accum, &mut items)?;
    }
    if let Some(tokens) = pending_directive.take() {
        items.push(DocumentItem::parse_directive(&tokens));
    }

    Ok(items)
}

fn resolve_point_marks(
    mark_str: &str,
    annotation_row: usize,
    col_marks: &HashMap<char, usize>,
    row_marks: &HashMap<char, usize>,
) -> (i16, i16) {
    let chars: Vec<char> = mark_str.chars().collect();
    match chars.len() {
        1 => {
            let col = col_marks.get(&chars[0]).copied().unwrap_or(0);
            (col as i16, annotation_row as i16)
        }
        2 => {
            let mut col = 0usize;
            let mut row = annotation_row;

            for &ch in &chars {
                if let Some(&r) = row_marks.get(&ch) {
                    row = r;
                } else if let Some(&c) = col_marks.get(&ch) {
                    col = c;
                }
            }
            (col as i16, row as i16)
        }
        _ => (0, annotation_row as i16),
    }
}

fn make_glyph_name(char_str: &str) -> String {
    if let Some(hex_rest) = char_str.strip_prefix("U+").or_else(|| char_str.strip_prefix("u+")) {
        if hex_rest.contains("..") {
            return char_str.to_uppercase();
        }
        if let Ok(cp) = u32::from_str_radix(hex_rest, 16) {
            return format!("uni{cp:04X}");
        }
    }
    let cp = char_to_codepoint(char_str);
    if let Some(cp) = cp {
        format!("uni{cp:04X}")
    } else {
        format!("map-{}", char_str.to_lowercase())
    }
}

fn emit_map_glyph(
    char_str: &str,
    rhs: &[String],
    items: &mut Vec<DocumentItem>,
    default_offsets: &HashMap<String, (i16, i16)>,
) -> Result<String> {
    let glyph_name = make_glyph_name(char_str);

    let refs: Vec<GlyphRef> = rhs
        .iter()
        .map(|spec| {
            let (name, roff, coff, _, negated, explicit) = parse_subglyph_offset(spec);
            let lname = name.to_lowercase();
            let def_off = default_offsets.get(&lname).copied().unwrap_or((0, 0));
            let row = roff + def_off.0;
            let col = coff + def_off.1;
            let offset = glyph_ref_offset(row, col, explicit);
            GlyphRef { name: lname, offset, negated, fill: None }
        })
        .collect();

    items.push(DocumentItem::Glyph {
        name: GlyphName(glyph_name.clone()),
        body: GlyphBody {
            refs,
            ..GlyphBody::new()
        },
    });

    Ok(glyph_name)
}

fn emit_map_items(
    char_str: &str,
    rhs: &[String],
    items: &mut Vec<DocumentItem>,
    default_offsets: &HashMap<String, (i16, i16)>,
) -> Result<()> {
    let glyph_name = emit_map_glyph(char_str, rhs, items, default_offsets)?;
    items.push(DocumentItem::Map {
        char_repr: parse_map_char_to_repr(char_str),
        glyph: glyph_name,
    });
    Ok(())
}

fn expand_parenthesized(spec: &str, index: usize) -> String {
    let Some(open) = spec.find('(') else {
        return spec.to_string();
    };
    let Some(close) = spec[open..].find(')') else {
        return spec.to_string();
    };
    let close = open + close;
    let prefix = &spec[..open];
    let suffix = &spec[close + 1..];
    let mut alternatives: Vec<String> = Vec::new();
    for part in spec[open + 1..close].split('|') {
        if let Some((name, count_str)) = part.rsplit_once('*')
            && let Ok(n) = count_str.parse::<usize>() {
                for _ in 0..n {
                    alternatives.push(name.to_string());
                }
                continue;
            }
        alternatives.push(part.to_string());
    }
    let alt = alternatives.get(index).unwrap_or(alternatives.last().unwrap());
    format!("{prefix}{alt}{suffix}")
}

fn expand_map_items(
    char_str: &str,
    specs: &[String],
    items: &mut Vec<DocumentItem>,
    default_offsets: &HashMap<String, (i16, i16)>,
) -> Result<()> {
    let chars: Vec<&str> = char_str.split('|').collect();
    if chars.len() <= 1 {
        return emit_map_items(char_str, specs, items, default_offsets);
    }

    let glyph_names: Vec<String> = chars.iter().map(|ch| make_glyph_name(ch)).collect();
    let combined_name = glyph_names.join("|");

    let refs: Vec<GlyphRef> = specs
        .iter()
        .map(|spec| {
            let expanded_first = expand_parenthesized(spec, 0);
            let (name_first, roff, coff, _, negated, explicit) =
                parse_subglyph_offset(&expanded_first);
            let lname_first = name_first.to_lowercase();
            let def_off = default_offsets.get(&lname_first).copied().unwrap_or((0, 0));

            let (unexpanded_name, _, _, _, _, _) = parse_subglyph_offset(spec);
            let row = roff + def_off.0;
            let col = coff + def_off.1;
            let offset = glyph_ref_offset(row, col, explicit);
            GlyphRef { name: unexpanded_name.to_lowercase(), offset, negated, fill: None }
        })
        .collect();

    items.push(DocumentItem::Glyph {
        name: GlyphName(combined_name.clone()),
        body: GlyphBody {
            refs,
            ..GlyphBody::new()
        },
    });

    items.push(DocumentItem::Map {
        char_repr: char_str.to_string(),
        glyph: combined_name,
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Default offsets — first non-'+' pixel position per glyph
// ---------------------------------------------------------------------------

fn compute_default_offset(lines: &[String]) -> Option<(i16, i16)> {
    let mut min_r = usize::MAX;
    let mut min_c = usize::MAX;
    for (r, line) in lines.iter().enumerate() {
        for (c, px) in line.chars().enumerate() {
            if px != '+' {
                min_r = min_r.min(r);
                min_c = min_c.min(c);
            }
        }
    }
    if min_r == usize::MAX {
        None
    } else {
        Some((min_r as i16, min_c as i16))
    }
}

fn collect_default_offsets_new(
    content: &str,
    _name_parts: &HashMap<String, Vec<String>>,
) -> Result<HashMap<String, (i16, i16)>> {
    let mut offsets = HashMap::new();
    let mut current_name: Option<String> = None;
    let mut pixel_lines: Vec<String> = Vec::new();

    let flush = |offsets: &mut HashMap<String, (i16, i16)>,
                 name: &str,
                 lines: &[String]| {
        if !lines.is_empty()
            && let Some(off) = compute_default_offset(lines) {
                offsets.insert(name.to_string(), off);
            }
    };

    for line in content.lines() {
        let comment_stripped = if let Some(idx) = line.find("//") {
            &line[..idx]
        } else {
            line
        };
        let trimmed = comment_stripped.trim();

        if trimmed.is_empty() {
            if let Some(name) = current_name.take() {
                flush(&mut offsets, &name, &pixel_lines);
                pixel_lines.clear();
            }
            continue;
        }

        if trimmed.starts_with("name-parts ") || trimmed.starts_with("map ")
            || trimmed.starts_with("default ") || trimmed.starts_with("exclude-from-sample ")
            || trimmed.starts_with("font-meta ") || trimmed.starts_with("remap ")
            || trimmed.starts_with("feature ")
        {
            if let Some(name) = current_name.take() {
                flush(&mut offsets, &name, &pixel_lines);
                pixel_lines.clear();
            }
            continue;
        }

        let args = tokenize_tokens(trimmed).unwrap_or_default();
        if args.is_empty() {
            continue;
        }

        if args[0] == "glyph" && args.len() >= 2 {
            if let Some(name) = current_name.take() {
                flush(&mut offsets, &name, &pixel_lines);
                pixel_lines.clear();
            }
            let raw_name = &args[1];
            if !raw_name.contains("($") {
                current_name = Some(raw_name.to_lowercase());
            }
        } else if current_name.is_some() {
            // Skip column mark rows (row mark is '=')
            if args.len() >= 2 && args[1] == "=" {
                continue;
            }
            let pixel_data = &args[0];
            if pixel_lines.is_empty() || pixel_lines[0].len() == pixel_data.len() {
                pixel_lines.push(pixel_data.to_string());
            }
        }
    }
    if let Some(name) = current_name.take() {
        flush(&mut offsets, &name, &pixel_lines);
    }

    Ok(offsets)
}

// ---------------------------------------------------------------------------
// Directory-level migration
// ---------------------------------------------------------------------------

pub fn migrate_directory(input_dir: &Path, output_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;

    let mut entries: Vec<_> = std::fs::read_dir(input_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let p = e.path();
            p.extension().is_some_and(|ext| ext == "txt")
                && p.file_name().is_none_or(|n| n != "LICENSE.txt")
        })
        .collect();
    entries.sort_by_key(|e| e.path());

    // First pass: collect all name-parts and default offsets across all files
    let mut all_name_parts: HashMap<String, Vec<String>> = HashMap::new();
    let mut all_contents: Vec<(std::path::PathBuf, String)> = Vec::new();
    for entry in &entries {
        let content = std::fs::read_to_string(entry.path())
            .with_context(|| format!("reading {}", entry.path().display()))?;
        match collect_name_parts(&content) {
            Ok(parts) => all_name_parts.extend(parts),
            Err(e) => eprintln!(
                "  WARNING scanning name-parts in {}: {e:#}",
                entry.path().display()
            ),
        }
        all_contents.push((entry.path(), content));
    }

    let mut global_offsets: HashMap<String, (i16, i16)> = HashMap::new();
    for (path, content) in &all_contents {
        match collect_default_offsets_new(content, &all_name_parts) {
            Ok(offsets) => global_offsets.extend(offsets),
            Err(e) => eprintln!(
                "  WARNING scanning offsets in {}: {e:#}",
                path.display()
            ),
        }
    }

    let mut total_glyphs = 0usize;
    let mut total_maps = 0usize;

    for (path, content) in &all_contents {
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let out_path = output_dir.join(format!("{stem}.unf"));

        eprintln!("Migrating {}...", path.display());

        match migrate_file(content, &all_name_parts, &global_offsets) {
            Ok(items) => {
                let glyph_count = items
                    .iter()
                    .filter(|i| matches!(i, DocumentItem::Glyph { .. }))
                    .count();
                let map_count = items
                    .iter()
                    .filter(|i| matches!(i, DocumentItem::Map { .. }))
                    .count();
                total_glyphs += glyph_count;
                total_maps += map_count;

                let doc = Document {
                    items,
                    item_line_starts: Vec::new(),
                    docline_file_lines: Vec::new(),
                    path: out_path.clone(),
                    dirty: false,
                    edit_gen: 0,
                };
                let mut buf = Vec::new();
                document_io::serialize_document(&doc, &mut buf)?;
                crate::document_io::write_and_sync(&out_path, &buf)?;

                eprintln!(
                    "  -> {} ({} glyphs, {} maps)",
                    out_path.display(),
                    glyph_count,
                    map_count
                );
            }
            Err(e) => {
                eprintln!("  ERROR: {e:#}");
            }
        }
    }

    eprintln!(
        "Done. Total: {} glyphs, {} maps across {} files.",
        total_glyphs,
        total_maps,
        entries.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated_ref(content: &str) -> GlyphRef {
        let items = migrate_file(content, &HashMap::new(), &HashMap::new()).unwrap();
        items
            .into_iter()
            .find_map(|item| match item {
                DocumentItem::Glyph { body, .. } => body.refs.into_iter().next(),
                _ => None,
            })
            .expect("migrated glyph ref")
    }

    #[test]
    fn explicit_zero_subglyph_offsets_are_preserved() {
        assert_eq!(
            migrated_ref("glyph child = -base\n").offset,
            Some((0, 0)),
        );
        assert_eq!(
            migrated_ref("glyph child = base@X\nX\n").offset,
            Some((0, 0)),
        );
        assert_eq!(migrated_ref("glyph child = base\n").offset, None);
    }
}
