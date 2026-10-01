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

    /// The same, abortable. The editor's derived-data rebuild is cancelled to
    /// stop it *blocking the next one*, so what matters is how long a cancel
    /// takes to be noticed: the expansion, which is most of a second on a slow
    /// machine, checks between its own stages as well as here
    /// ([`crate::render::ttf_builder::expand_documents_cancellable`]).
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
        let expansion = crate::render::ttf_builder::expand_documents_cancellable(
            docs,
            &name_parts,
            &faces,
            cancel,
        )?;
        Some(Self {
            name_parts,
            faces,
            meta: crate::meta::FontMeta::for_face(docs, primary),
            expansion,
        })
    }
}

#[cfg(feature = "editor")]
impl Resolution {
    /// [`Self::compute_cancellable`] for the editor's rebuild, reusing what
    /// `memo` holds from the last one where that still applies, and leaving
    /// this one's there for the next.
    pub fn compute_reusing(
        docs: &[std::sync::Arc<Document>],
        memo: &std::sync::Mutex<NameMemo>,
        cancel: &crate::cancel::CancelToken,
    ) -> Option<Self> {
        if cancel.is_cancelled() {
            return None;
        }
        let refs: Vec<&Document> = docs.iter().map(|d| &**d).collect();
        let name_parts = crate::document::collect_name_parts(&refs);
        let faces = crate::faces::FaceSet::collect(&refs);
        let primary_id = faces.primary().id.clone();
        let primary = (!primary_id.is_empty()).then_some(primary_id.as_str());
        let reuse = crate::parallel::lock_memo(memo).reusable(docs);
        let expansion = crate::render::ttf_builder::expand_documents_reusing(
            &refs,
            &name_parts,
            &faces,
            reuse,
            cancel,
        )?;
        crate::parallel::lock_memo(memo).store(docs, expansion.name_level());
        Some(Self {
            name_parts,
            faces,
            meta: crate::meta::FontMeta::for_face(&refs, primary),
            expansion,
        })
    }
}

/// The searches and the aliases of the editor's last rebuild, with the
/// documents they were resolved from.
///
/// Between them they are a third of an expansion, and they read names alone:
/// an edit to a drawing — most of what the editor sends a rebuild for — cannot
/// change either, and one to a `ref` or an IDC line can change only the
/// aliases, the searches reading nothing of a `glyph` block but its name. The
/// documents are compared rather than fingerprinted: a document the edit did
/// not reach is the same `Arc` as last time, and the one it did is compared
/// item by item, over exactly what each stage reads
/// ([`crate::document::NameMatch`]). There is no key to go stale, and nothing
/// to collide.
#[cfg(feature = "editor")]
#[derive(Default)]
pub struct NameMemo {
    docs: Vec<std::sync::Arc<Document>>,
    names: Option<crate::render::ttf_builder::NameLevel>,
}

#[cfg(feature = "editor")]
impl NameMemo {
    /// What of the last rebuild's `docs` still answers for: the searches where
    /// every document has the names it had, and the aliases where every one
    /// has its slots as well.
    fn reusable(&self, docs: &[std::sync::Arc<Document>]) -> crate::render::ttf_builder::Reuse {
        use crate::document::NameMatch;
        let mut reuse = crate::render::ttf_builder::Reuse::default();
        let Some(names) = &self.names else {
            return reuse;
        };
        if docs.len() != self.docs.len() {
            return reuse;
        }
        let mut least = NameMatch::Slots;
        for (a, b) in docs.iter().zip(&self.docs) {
            if std::sync::Arc::ptr_eq(a, b) {
                continue;
            }
            match a.name_match(b) {
                NameMatch::Different => return reuse,
                NameMatch::Names => least = NameMatch::Names,
                NameMatch::Slots => {}
            }
        }
        reuse.searches = Some(names.searches.clone());
        if least == NameMatch::Slots {
            reuse.aliases = Some(names.aliases.clone());
        }
        reuse
    }

    fn store(
        &mut self,
        docs: &[std::sync::Arc<Document>],
        names: crate::render::ttf_builder::NameLevel,
    ) {
        self.docs = docs.to_vec();
        self.names = Some(names);
    }

    /// Forget everything, for a folder that has nothing to do with the last.
    pub fn clear(&mut self) {
        *self = Self::default();
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

#[cfg(all(test, feature = "editor"))]
mod name_memo_tests {
    use super::*;
    use crate::document::DocumentItem;
    use crate::document_io::parse_document_from_str;
    use std::sync::{Arc, Mutex};

    /// Two blocks the merge folds (`a-j` into `a-g`, and `b-j` into `b-g`
    /// through its `ref`), and a lone glyph whose pixels are edited below.
    const SOURCE: &str = "\
glyph a-(g|j) 1 1
@@
glyph b-(g|j) 1 1
ref a-(g|j) 0 0
glyph c 2 1
@@..
";

    fn doc(src: &str) -> Arc<Document> {
        Arc::new(parse_document_from_str(src, "t.unf".into()).unwrap())
    }

    /// What an expansion comes to, in a form two of them compare by.
    fn outcome(r: &Resolution) -> (Vec<DocumentItem>, String) {
        (
            r.expansion.items().cloned().collect(),
            format!(
                "{:?} {:?} {:?}",
                r.expansion.diagnostics, r.expansion.aliases.diagnostics, r.expansion.exists
            ),
        )
    }

    fn fresh(docs: &[Arc<Document>]) -> Resolution {
        let refs: Vec<&Document> = docs.iter().map(|d| &**d).collect();
        Resolution::compute(&refs)
    }

    fn reusing(docs: &[Arc<Document>], memo: &Mutex<NameMemo>) -> Resolution {
        Resolution::compute_reusing(docs, memo, &crate::cancel::CancelToken::never()).unwrap()
    }

    /// Which stages `after` reuses from `before`, and that what it comes to is
    /// what resolving it afresh comes to.
    fn reused(before: &str, after: &str) -> (bool, bool) {
        let memo = Mutex::new(NameMemo::default());
        reusing(&[doc(before)], &memo);
        let after = vec![doc(after)];
        let reuse = memo.lock().unwrap().reusable(&after);
        let used = (reuse.searches.is_some(), reuse.aliases.is_some());
        assert_eq!(outcome(&reusing(&after, &memo)), outcome(&fresh(&after)));
        used
    }

    #[test]
    fn an_edit_to_a_drawing_or_an_offset_reuses_both() {
        assert_eq!(
            reused(SOURCE, &SOURCE.replace("@@..", "@@@@")),
            (true, true)
        );
        assert_eq!(
            reused(
                SOURCE,
                &SOURCE.replace("ref a-(g|j) 0 0", "ref a-(g|j) 1 0")
            ),
            (true, true)
        );
    }

    /// `b`'s second expansion now refers to `c`, so `b` no longer merges: the
    /// aliases of the last rebuild would still fold `b-j` into `b-g`.
    #[test]
    fn an_edit_to_a_slot_reuses_the_searches_alone() {
        let after = SOURCE.replace("ref a-(g|j) 0 0", "ref (a-g|c) 0 0");
        assert_eq!(reused(SOURCE, &after), (true, false));
        assert_ne!(
            outcome(&fresh(&[doc(&after)])),
            outcome(&fresh(&[doc(SOURCE)]))
        );
    }

    /// `a-(g|j)` becoming `a-(g|k)` is a change of names in a block the same
    /// length: reused, the merge would still fold an `a-j` that is gone and
    /// leave the new `a-k` a glyph of its own.
    #[test]
    fn an_edit_to_a_name_resolves_again() {
        let after = SOURCE.replacen("a-(g|j) 1 1", "a-(g|k) 1 1", 1);
        assert_eq!(reused(SOURCE, &after), (false, false));
        assert_ne!(
            outcome(&fresh(&[doc(&after)])),
            outcome(&fresh(&[doc(SOURCE)]))
        );
    }

    #[test]
    fn name_match_reads_names_then_slots() {
        use crate::document::NameMatch;
        let base = doc(SOURCE);
        let cases = [
            (SOURCE.replace("@@..", "..@@"), NameMatch::Slots),
            (
                SOURCE.replace("ref a-(g|j) 0 0", "ref a-(g|j) 1 0"),
                NameMatch::Slots,
            ),
            (
                SOURCE.replace("ref a-(g|j) 0 0", "ref (a-g|c) 0 0"),
                NameMatch::Names,
            ),
            (
                SOURCE.replace("glyph c 2 1", "glyph c 2 1 keep"),
                NameMatch::Names,
            ),
            (SOURCE.replace("glyph c", "glyph d"), NameMatch::Different),
            (format!("{SOURCE}glyph e = c\n"), NameMatch::Different),
        ];
        for (changed, expected) in cases {
            assert_eq!(base.name_match(&doc(&changed)), expected, "{changed}");
        }
        let mut elsewhere = (*doc(SOURCE)).clone();
        elsewhere.path = "u.unf".into();
        assert_eq!(base.name_match(&elsewhere), NameMatch::Different);
    }
}
