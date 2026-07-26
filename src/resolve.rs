//! Shared vocabulary for the resolution pipeline.
//!
//! Resolution (name-part substitution, pattern expansion, on-demand glyph
//! synthesis, `map <decomposable>` synthesis) used to be re-implemented in
//! three places — `render::ttf_builder` for the font build, `ref_composite`
//! for the editor, and `issues` for validation. The build and editor copies
//! discard the document/item an expanded name came from, which is why the
//! validation copy could not reuse them and why problems the build path
//! detects were silently dropped instead of reported.
//!
//! [`ItemRef`] restores that provenance cheaply enough to attach to every
//! expanded item, and [`Diagnostic`] is what the pipeline reports through
//! instead of `continue`-ing or `eprintln!`-ing.

use std::path::PathBuf;

use crate::document::{Document, DocumentItem};
use crate::issues::{Issue, Severity};

/// Points at one `DocumentItem` within a `&[&Document]` slice. Small enough to
/// hang off every expanded item without meaningfully growing the expansion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemRef {
    pub doc: u32,
    pub item: u32,
}

impl ItemRef {
    pub fn new(doc: usize, item: usize) -> Self {
        Self { doc: doc as u32, item: item as u32 }
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    /// `None` for findings that belong to the font as a whole rather than to
    /// any one line (e.g. a missing `font-meta`).
    pub origin: Option<ItemRef>,
    pub message: String,
}

impl Diagnostic {
    pub fn error(origin: impl Into<Option<ItemRef>>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            origin: origin.into(),
            message: message.into(),
        }
    }

    pub fn new(severity: Severity, origin: Option<ItemRef>, message: String) -> Self {
        Self { severity, origin, message }
    }
}

/// The `font-meta` values a document set declares.
///
/// Each field records whether it was actually declared, because validation has
/// to tell "not stated" apart from "stated as the default"; every other
/// consumer just wants the effective number and reads it through the
/// accessors. This used to be parsed three times — by the font build, the
/// sample renderer and the validator — with three slightly different loops.
#[derive(Clone, Copy, Debug, Default)]
pub struct FontMeta {
    pub height: Option<u16>,
    pub ascent: Option<u16>,
    pub descent: Option<u16>,
    /// The last `font-meta` line that set anything, for error reporting.
    pub origin: Option<ItemRef>,
}

impl FontMeta {
    pub const DEFAULT_HEIGHT: u16 = 16;
    pub const DEFAULT_ASCENT: u16 = 14;
    pub const DEFAULT_DESCENT: u16 = 2;

    pub fn height(&self) -> u16 {
        self.height.unwrap_or(Self::DEFAULT_HEIGHT)
    }

    pub fn ascent(&self) -> u16 {
        self.ascent.unwrap_or(Self::DEFAULT_ASCENT)
    }

    pub fn descent(&self) -> u16 {
        self.descent.unwrap_or(Self::DEFAULT_DESCENT)
    }

    pub fn collect(docs: &[&Document]) -> Self {
        let mut meta = Self::default();
        for (doc_idx, doc) in docs.iter().enumerate() {
            for (item_idx, item) in doc.items.iter().enumerate() {
                let DocumentItem::FontMeta(s) = item else {
                    continue;
                };
                meta.origin = Some(ItemRef::new(doc_idx, item_idx));
                let mut iter = s.split_whitespace();
                while let Some(key) = iter.next() {
                    let Some(val) = iter.next() else { break };
                    let Ok(v) = val.parse::<u16>() else { continue };
                    match key {
                        "height" => meta.height = Some(v),
                        "ascent" => meta.ascent = Some(v),
                        "descent" => meta.descent = Some(v),
                        _ => {}
                    }
                }
            }
        }
        meta
    }
}

/// Everything derived from a document set that more than one consumer needs.
///
/// Resolution is expensive enough (~25 ms over `font/`) that the editor used
/// to pay for it three times per edit — once for the glyph cache, once for
/// validation and once for the font build. Computing it once and handing this
/// around is the point of the type.
pub struct Resolution {
    pub name_parts: crate::document::NamePartsMap,
    pub meta: FontMeta,
    pub expansion: crate::render::ttf_builder::Expansion,
}

impl Resolution {
    pub fn compute(docs: &[&Document]) -> Self {
        let name_parts = crate::document::collect_name_parts(docs);
        let expansion = crate::render::ttf_builder::expand_documents(docs, &name_parts);
        Self { name_parts, meta: FontMeta::collect(docs), expansion }
    }
}

/// A borrowed set of documents plus the lookup that turns an [`ItemRef`] back
/// into a file position.
#[derive(Clone, Copy)]
pub struct DocSet<'a> {
    docs: &'a [&'a Document],
}

impl<'a> DocSet<'a> {
    pub fn new(docs: &'a [&'a Document]) -> Self {
        Self { docs }
    }

    pub fn get(&self, r: ItemRef) -> Option<&'a Document> {
        self.docs.get(r.doc as usize).copied()
    }

    /// `(path, docline index, 1-based file line)` — the three fields `Issue`
    /// needs. Falls back to the start of the file for a stale `ItemRef`.
    pub fn location(&self, r: ItemRef) -> (PathBuf, usize, usize) {
        let Some(doc) = self.get(r) else {
            return (PathBuf::new(), 0, 1);
        };
        let line = doc
            .item_line_starts
            .get(r.item as usize)
            .copied()
            .unwrap_or(0);
        let file_line = doc.docline_file_lines.get(line).copied().unwrap_or(line) + 1;
        (doc.path.clone(), line, file_line)
    }

    pub fn to_issue(&self, d: &Diagnostic) -> Issue {
        let (file, line, file_line) = match d.origin {
            Some(r) => self.location(r),
            None => (
                self.docs.first().map(|d| d.path.clone()).unwrap_or_default(),
                0,
                1,
            ),
        };
        Issue {
            severity: d.severity.clone(),
            message: d.message.clone(),
            file,
            line,
            file_line,
        }
    }

    pub fn to_issues(&self, diags: &[Diagnostic]) -> Vec<Issue> {
        diags.iter().map(|d| self.to_issue(d)).collect()
    }
}
