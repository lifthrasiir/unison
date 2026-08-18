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
        Self {
            doc: doc as u32,
            item: item as u32,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    /// `None` for findings that belong to the font as a whole rather than to
    /// any one line (e.g. a missing `meta`).
    pub origin: Option<ItemRef>,
    /// The one *expanded* glyph this is about, where that is narrower than
    /// `origin`. A `glyph han-($#4e00..9fff)` line is one item and eighteen
    /// thousand glyphs, and whether its substituted `ref` resolves is answered
    /// per glyph, not per line — so a finding from that path names the glyph
    /// here and [`crate::glyph_flags`] faults that one instead of the whole
    /// pattern. `None` means the finding really is about the line, which for
    /// a pattern means every glyph it stands for.
    pub glyph: Option<String>,
    pub message: String,
}

impl Diagnostic {
    pub fn error(origin: impl Into<Option<ItemRef>>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            origin: origin.into(),
            glyph: None,
            message: message.into(),
        }
    }

    /// Narrow this finding to one expanded glyph; see [`Diagnostic::glyph`].
    pub fn about(mut self, glyph: impl Into<String>) -> Self {
        self.glyph = Some(glyph.into());
        self
    }

    pub fn new(severity: Severity, origin: Option<ItemRef>, message: String) -> Self {
        Self {
            severity,
            origin,
            glyph: None,
            message,
        }
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
    pub faces: crate::faces::FaceSet,
    pub meta: crate::meta::FontMeta,
    pub expansion: crate::render::ttf_builder::Expansion,
}

impl Resolution {
    pub fn compute(docs: &[&Document]) -> Self {
        Self::compute_cancellable(docs, &crate::cancel::CancelToken::never())
            .expect("a `never` token cannot cancel")
    }

    /// The same, abortable at each stage boundary. The stages here are coarse —
    /// expansion is the expensive one and does not report progress — so this
    /// gives up between them rather than within, which is enough: the editor's
    /// derived-data rebuild is cancelled to stop it *blocking the next one*,
    /// and one stage is the granularity that costs.
    pub fn compute_cancellable(
        docs: &[&Document],
        cancel: &crate::cancel::CancelToken,
    ) -> Option<Self> {
        if cancel.is_cancelled() {
            return None;
        }
        let name_parts = crate::document::collect_name_parts(docs);
        let faces = crate::faces::FaceSet::collect(docs);
        // The face a single-face build emits; its metadata is what the tables
        // are assembled from.
        let primary_id = faces.primary().id.clone();
        let primary = if primary_id.is_empty() {
            None
        } else {
            Some(primary_id.as_str())
        };
        if cancel.is_cancelled() {
            return None;
        }
        let expansion = crate::render::ttf_builder::expand_documents_for(docs, &name_parts, &faces);
        if cancel.is_cancelled() {
            return None;
        }
        Some(Self {
            name_parts,
            faces,
            meta: crate::meta::FontMeta::for_face(docs, primary),
            expansion,
        })
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
        let (line, file_line) = doc.item_lines(r.item as usize);
        (doc.path.clone(), line, file_line)
    }

    pub fn to_issue(self, d: &Diagnostic) -> Issue {
        let (file, line, file_line) = match d.origin {
            Some(r) => self.location(r),
            None => (
                self.docs
                    .first()
                    .map(|d| d.path.clone())
                    .unwrap_or_default(),
                0,
                1,
            ),
        };
        Issue {
            severity: d.severity,
            glyph: d.glyph.clone(),
            message: d.message.clone(),
            file,
            line,
            file_line,
        }
    }

    pub fn to_issues(self, diags: &[Diagnostic]) -> Vec<Issue> {
        diags.iter().map(|d| self.to_issue(d)).collect()
    }
}
