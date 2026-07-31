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

/// One parsed `meta` line.
///
/// The variant *is* the key, so a key that parses is a key that some consumer
/// handles — adding a key without wiring it up does not compile. [`FontMeta`]
/// and [`crate::issues`] both go through [`parse_meta_entry`] rather than
/// walking the tokens themselves, so what counts as a valid line is decided in
/// exactly one place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetaEntry {
    Height(u16),
    Ascent(u16),
    Descent(u16),
}

impl MetaEntry {
    /// The key as written. Duplicate detection groups by this, so two spellings
    /// of one key would have to collapse here.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Height(_) => "height",
            Self::Ascent(_) => "ascent",
            Self::Descent(_) => "descent",
        }
    }
}

/// Every `meta` key, for error messages and completion.
pub const META_KEYS: &[&str] = &["height", "ascent", "descent"];

/// Parse the text of a `meta` item — everything after the keyword, comment
/// included, exactly as [`crate::document::DocumentItem::Meta`] stores it.
///
/// `Err` carries a message ready to be reported; the caller supplies the
/// position. A key that takes N values rejects N±1, because a `meta` value is
/// invisible in the built font: a line that is quietly half-read is a line
/// whose mistake ships.
pub fn parse_meta_entry(text: &str) -> std::result::Result<MetaEntry, String> {
    let tokens = crate::document_io::tokenize_tokens(text)
        .map_err(|e| format!("malformed `meta` line: {e}"))?;
    let Some((key, values)) = tokens.split_first() else {
        return Err("`meta` needs a key".to_string());
    };

    if values.first().is_some_and(|t| t == ":") {
        return Err(format!(
            "face-scoped `meta` is not supported yet (`meta {key} : ...`)",
        ));
    }

    // Every key so far takes one u16; the shape below is what makes adding a
    // wider key (`panose`, a flag, a string) a local change.
    let one_metric = |values: &[String]| -> std::result::Result<u16, String> {
        match values {
            [v] => v.parse::<u16>().map_err(|_| {
                format!("`meta {key}` takes a number, got `{v}`")
            }),
            _ => Err(format!(
                "`meta {key}` takes exactly 1 value, got {}",
                values.len(),
            )),
        }
    };

    match key.as_str() {
        "height" => Ok(MetaEntry::Height(one_metric(values)?)),
        "ascent" => Ok(MetaEntry::Ascent(one_metric(values)?)),
        "descent" => Ok(MetaEntry::Descent(one_metric(values)?)),
        _ => Err(format!(
            "unknown `meta` key `{key}` (known keys: {})",
            META_KEYS.join(", "),
        )),
    }
}

/// The `meta` values a document set declares.
///
/// Each field records whether it was actually declared, because validation has
/// to tell "not stated" apart from "stated as the default"; every other
/// consumer just wants the effective number and reads it through the
/// accessors. This used to be parsed three times — by the font build, the
/// sample renderer and the validator — with three slightly different loops.
///
/// Malformed and duplicate lines are *not* reported here: this runs on every
/// editor frame, while reporting belongs to [`crate::issues`]. Both sides share
/// [`parse_meta_entry`], so they cannot disagree about what a line means; a line
/// that fails to parse is simply skipped, leaving the default in place.
#[derive(Clone, Copy, Debug, Default)]
pub struct FontMeta {
    pub height: Option<u16>,
    pub ascent: Option<u16>,
    pub descent: Option<u16>,
    /// The last `meta` line that set anything, for error reporting.
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
                let DocumentItem::Meta(s) = item else {
                    continue;
                };
                let Ok(entry) = parse_meta_entry(s) else { continue };
                meta.origin = Some(ItemRef::new(doc_idx, item_idx));
                match entry {
                    MetaEntry::Height(v) => meta.height = Some(v),
                    MetaEntry::Ascent(v) => meta.ascent = Some(v),
                    MetaEntry::Descent(v) => meta.descent = Some(v),
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
        let (line, file_line) = doc.item_lines(r.item as usize);
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
