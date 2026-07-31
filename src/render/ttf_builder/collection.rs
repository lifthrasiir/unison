//! Writing a TrueType Collection.
//!
//! `write-fonts` cannot do this: `font_builder.rs` has a `TTCHeader` type
//! generated from the spec, but writing it is
//! `panic!("TTCHeader writing not supported (yet)")` in 0.50 and still in 0.51.
//! What it *can* do is build each face as a standalone font, which is what this
//! module takes as input — so nothing here duplicates table assembly. It only
//! re-lays those bytes out under one header.
//!
//! # What a collection actually shares
//!
//! A TTC is a file with one `TableDirectory` per face, each free to point
//! anywhere in the file. Two faces "share" a table by naming the same offset,
//! so sharing is a property of the layout, not a declaration: identical bytes
//! are stored once and pointed at twice. [`build_collection`] therefore dedups
//! by content, which needs no knowledge of what a table means.
//!
//! Content dedup only pays off because the glyph store is built once, for the
//! union of every slice, in an order no single face's cmap decided — see
//! [`super::build_faces`]. With a per-face glyph order the two `glyf` tables
//! would differ from the first differing glyph onward and nothing would be
//! shared. With it, two faces of the ambiguous-width split share everything
//! except `cmap` and `name`: measured over `font/` with a two-face overlay,
//! 1.29 MB against 2.56 MB for the same two faces as separate files.
//!
//! # checkSumAdjustment
//!
//! `head.checkSumAdjustment` is defined over "the whole font file", which has
//! no meaning for a face inside a collection: the bytes it would cover are not
//! contiguous and the file holds several fonts. It is zeroed here, which is
//! what the field being advisory allows and what keeps two faces with otherwise
//! identical `head` tables sharing one.

use std::collections::HashMap;

use write_fonts::types::Tag;

/// Offset of `checkSumAdjustment` within `head`: version (4) + fontRevision (4).
const HEAD_CHECKSUM_ADJUSTMENT_OFFSET: usize = 8;

/// Assemble standalone fonts into one collection, in the order given.
///
/// Face order is user-visible — a consumer that does not choose a face gets the
/// first — so the input order is preserved exactly.
pub(crate) fn build_collection(fonts: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if fonts.is_empty() {
        return Err("a collection needs at least one face".to_string());
    }

    // Every face's tables, sliced out of its standalone font.
    let mut per_face: Vec<Vec<(Tag, Vec<u8>)>> = Vec::new();
    for (i, bytes) in fonts.iter().enumerate() {
        let font = read_fonts::FontRef::new(bytes)
            .map_err(|e| format!("face {i} is not a readable font: {e}"))?;
        let mut tables = Vec::new();
        for rec in font.table_directory().table_records() {
            let start = rec.offset() as usize;
            let end = start + rec.length() as usize;
            let data = bytes
                .get(start..end)
                .ok_or_else(|| format!("face {i} table {} is out of bounds", rec.tag()))?;
            let mut data = data.to_vec();
            if rec.tag() == Tag::new(b"head") && data.len() >= HEAD_CHECKSUM_ADJUSTMENT_OFFSET + 4 {
                data[HEAD_CHECKSUM_ADJUSTMENT_OFFSET..HEAD_CHECKSUM_ADJUSTMENT_OFFSET + 4]
                    .copy_from_slice(&0u32.to_be_bytes());
            }
            tables.push((rec.tag(), data));
        }
        per_face.push(tables);
    }

    let num_fonts = fonts.len() as u32;
    // ttcf tag + version + numFonts + one Offset32 per face.
    let header_len = 12 + 4 * num_fonts as usize;
    // Each face's own directory: 12-byte header + 16 bytes per record.
    let directories_len: usize = per_face.iter().map(|t| 12 + 16 * t.len()).sum();

    let mut blobs: Vec<u8> = Vec::new();
    // Content -> offset, so identical tables are stored once. Keyed by the
    // bytes themselves rather than a hash: a collision here would silently
    // corrupt a face, and the tables are already in memory.
    let mut seen: HashMap<&[u8], u32> = HashMap::new();
    let blob_base = header_len + directories_len;

    // Offsets are resolved before anything is written, because a directory
    // records where its table *will* be.
    let mut placements: Vec<Vec<(Tag, u32, u32)>> = Vec::new();
    for tables in &per_face {
        let mut placed = Vec::new();
        for (tag, data) in tables {
            let offset = match seen.get(data.as_slice()) {
                Some(&off) => off,
                None => {
                    while blobs.len() % 4 != 0 {
                        blobs.push(0);
                    }
                    let off = (blob_base + blobs.len()) as u32;
                    blobs.extend_from_slice(data);
                    seen.insert(data.as_slice(), off);
                    off
                }
            };
            placed.push((*tag, offset, data.len() as u32));
        }
        // A directory's records are sorted by tag; a reader is allowed to
        // binary-search them.
        placed.sort_by_key(|(tag, _, _)| *tag);
        placements.push(placed);
    }

    let mut out: Vec<u8> = Vec::with_capacity(blob_base + blobs.len());
    out.extend_from_slice(b"ttcf");
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // version 1.0
    out.extend_from_slice(&num_fonts.to_be_bytes());
    let mut dir_offset = header_len as u32;
    for placed in &placements {
        out.extend_from_slice(&dir_offset.to_be_bytes());
        dir_offset += (12 + 16 * placed.len()) as u32;
    }

    for placed in &placements {
        let n = placed.len() as u16;
        // The binary-search assists: the largest power of two <= n, times 16.
        let entry_selector = (u16::BITS - 1 - n.max(1).leading_zeros()) as u16;
        let search_range = (1u16 << entry_selector) * 16;
        out.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // sfntVersion 1.0
        out.extend_from_slice(&n.to_be_bytes());
        out.extend_from_slice(&search_range.to_be_bytes());
        out.extend_from_slice(&entry_selector.to_be_bytes());
        out.extend_from_slice(&(n * 16 - search_range).to_be_bytes());
        for (tag, offset, length) in placed {
            out.extend_from_slice(&u32::from_be_bytes(tag.to_be_bytes()).to_be_bytes());
            out.extend_from_slice(&checksum(&blobs, *offset, *length, blob_base).to_be_bytes());
            out.extend_from_slice(&offset.to_be_bytes());
            out.extend_from_slice(&length.to_be_bytes());
        }
    }

    debug_assert_eq!(out.len(), blob_base, "directories must end where blobs begin");
    out.extend_from_slice(&blobs);
    Ok(out)
}

/// The table checksum: the sum of its big-endian `u32` words, wrapping, with
/// the last word zero-padded.
fn checksum(blobs: &[u8], offset: u32, length: u32, blob_base: usize) -> u32 {
    let start = offset as usize - blob_base;
    let end = start + length as usize;
    let mut sum: u32 = 0;
    let mut i = start;
    while i < end {
        let mut word = [0u8; 4];
        for (k, slot) in word.iter_mut().enumerate() {
            if let Some(b) = blobs.get(i + k) {
                *slot = *b;
            }
        }
        sum = sum.wrapping_add(u32::from_be_bytes(word));
        i += 4;
    }
    sum
}

#[cfg(test)]
#[path = "../ttf_tests/collection.rs"]
mod tests;
