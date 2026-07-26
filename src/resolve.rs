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

use crate::document::Document;
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
