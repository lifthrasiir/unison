//! Cross-document validation: everything that can only be judged with all
//! `.unf` files in hand.
//!
//! Missing and dangling refs, duplicate `map`s, unused glyphs, remap sanity.
//! Resolution itself emits its diagnostics directly (see [`crate::resolve`]);
//! this module is for what no single item's resolution can see. Both the build
//! and the editor print the same report, `error:`/`warning:` prefixed and
//! `file:line:` located, and a font with only warnings still builds — so the
//! report is meant to be read, not just exit-coded. [`Severity`] says how each
//! prefix is meant to be read, and which of them a build may ignore.
//!
//! A few rules are worth knowing about because they are refusals rather than
//! best-effort output:
//!
//! - a `remap` whose source and target lists are N→M or N→0 has no OpenType
//!   lookup type at all, so it is an error here instead of something the builder
//!   emits close-but-wrong;
//! - referring to a contentless glyph — one with no pixel grid and no `ref`, see
//!   [`crate::document_io`] — from a `map`, `ref` or `remap` is an error, since
//!   such a glyph never enters the resolution cache;
//! - the two anchor-exposure ambiguities in [`crate::ref_composite`] are errors,
//!   reported through an anchors-only resolution pass.

mod anchors;
mod colors;
mod directives;
mod glyph_names;
mod maps;
mod patterns;
mod remap;
mod slices;
mod unused;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::document::{Document, DocumentItem, GlyphName, SliceNameParts};
use crate::pattern::NamePartsMap;
use crate::resolve::{DocSet, Resolution};

/// How a finding is meant to be read.
///
/// The first two are about the *source*: something is wrong with what is
/// written, and a font built from it is wrong in a way nobody chose. The last
/// two are not.
///
/// [`Severity::Todo`] is work that has not been done yet. It reads exactly like
/// an error in the font — the glyph is not built and the character is not
/// mapped — but it is a normal state of the source rather than a defect in it:
/// a Han glyph whose IDC line has not picked its variants yet is on the queue,
/// not broken. So it never fails a build or a `uniform test` run, and its count
/// is expected to start in the tens of thousands and come down.
///
/// [`Severity::Note`] is the other direction: something worth saying that asks
/// for no action, so it is off by default in the editor's issue list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
    Todo,
    Note,
}

impl Severity {
    /// Every severity, worst first — the order the filter buttons are drawn in
    /// and the order [`Ord`] sorts by.
    pub const ALL: [Severity; 4] = [
        Severity::Error,
        Severity::Warning,
        Severity::Todo,
        Severity::Note,
    ];

    /// The prefix the command-line report writes.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Todo => "todo",
            Severity::Note => "note",
        }
    }

    /// The plural noun a count is counted in ("3 errors").
    #[cfg_attr(not(feature = "editor"), expect(dead_code))]
    pub fn plural(self) -> &'static str {
        match self {
            Severity::Error => "errors",
            Severity::Warning => "warnings",
            Severity::Todo => "todos",
            Severity::Note => "notes",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub severity: Severity,
    /// The one expanded glyph this is about, where the *line* is wider than the
    /// finding — see [`crate::resolve::Diagnostic::glyph`]. Only
    /// [`crate::glyph_flags`] reads it; the report itself locates by line.
    pub glyph: Option<String>,
    pub message: String,
    pub file: PathBuf,
    /// DocLine index (0-based), used for editor navigation.
    pub line: usize,
    /// 1-based file line number, used for display.
    pub file_line: usize,
}

/// An issue anchored at item `item_idx`'s defining line in `doc`.
fn issue_at(doc: &Document, item_idx: usize, severity: Severity, message: String) -> Issue {
    let (line, file_line) = doc.item_lines(item_idx);
    Issue {
        severity,
        glyph: None,
        message,
        file: doc.path.clone(),
        line,
        file_line,
    }
}

/// Everything the checks share: the documents, the resolution they were
/// expanded through, and the few tables derived from both that more than one
/// check wants.
///
/// Built once by [`collect_issues_with`] and handed to every check, so no
/// check re-expands what another already has. A check that needs something of
/// its own still computes it itself — this is for what is genuinely shared.
struct Cx<'a> {
    docs: &'a [&'a Document],
    docset: DocSet<'a>,
    resolution: &'a Resolution,
    name_parts: &'a NamePartsMap,
    /// Validation expands a slice-qualified line the way the build does: once
    /// per slice, with that slice's bindings.
    scoped_parts: SliceNameParts,
    faces: &'a crate::faces::FaceSet,
    expansion: &'a crate::render::ttf_builder::Expansion,
    aliases: &'a crate::alias::AliasMap,
    /// Every glyph the font will actually contain, including synthesized
    /// on-demand and decomposed-map glyphs. Aliases are deliberately absent:
    /// they are names, not glyphs, and the expansion has already rewritten
    /// every reference to them.
    all_glyph_names: HashSet<String>,
    /// Groups are collected up front rather than as the scan reaches them: a
    /// `feature` line may precede every rule of the group it attaches, and a
    /// declaration may follow them.
    groups: crate::document::RemapGroupOrder,
}

impl<'a> Cx<'a> {
    /// One document's items as a check over the *source* should read them.
    ///
    /// Two substitutions, both because an `exists` line and the item it governs
    /// do not mean what they say in isolation: the directive itself drops out,
    /// and a `map` it governs is replaced by the mappings it actually produced
    /// — the codepoint on a scoped `map` is computed from the match, so there
    /// is nothing on the written line for a source-side check to read.
    ///
    /// The index each item comes back with is the **written** one, so a finding
    /// still lands on the line the author can see.
    ///
    /// A scoped `glyph` block or alias is *not* substituted: its names expand
    /// from the same header the unscoped ones do, once `$N` is bound, and
    /// binding per match is what
    /// [`crate::exists::ExistsScopes::for_each_binding`] is for. Replacing it
    /// here would hand every such check N items where the source has one.
    fn source_items(&self, doc_idx: usize) -> Vec<(usize, &'a DocumentItem)> {
        let doc = self.docs[doc_idx];
        let exists = &self.expansion.exists;
        if exists.is_empty() {
            return doc.items.iter().enumerate().collect();
        }
        let mut out = Vec::with_capacity(doc.items.len());
        for (item_idx, item) in doc.items.iter().enumerate() {
            let here = crate::resolve::ItemRef::new(doc_idx, item_idx);
            if exists.is_directive(here) {
                continue;
            }
            if exists.scope(here).is_some() && matches!(item, DocumentItem::Map { .. }) {
                out.extend(
                    self.expansion
                        .items
                        .iter()
                        .filter(|e| e.origin == Some(here))
                        .map(|e| (item_idx, &e.item)),
                );
                continue;
            }
            out.push((item_idx, item));
        }
        out
    }
}

pub fn collect_issues(docs: &[&Document]) -> Vec<Issue> {
    collect_issues_with(docs, &Resolution::compute(docs))
}

/// Validate `docs` against an already-computed [`Resolution`].
///
/// Callers that resolve for their own reasons — the editor's glyph cache, the
/// font build — should use this rather than [`collect_issues`], which resolves
/// again from scratch.
pub fn collect_issues_with(docs: &[&Document], resolution: &Resolution) -> Vec<Issue> {
    let mut issues = Vec::new();

    let expansion = &resolution.expansion;
    let cx = Cx {
        docs,
        docset: DocSet::new(docs),
        resolution,
        name_parts: &resolution.name_parts,
        scoped_parts: SliceNameParts::with_base(docs, resolution.name_parts.clone()),
        faces: &resolution.faces,
        expansion,
        aliases: &expansion.aliases,
        all_glyph_names: expansion
            .items()
            .filter_map(|item| match item {
                DocumentItem::Glyph {
                    name: GlyphName(n), ..
                } => Some(n.clone()),
                _ => None,
            })
            .collect(),
        groups: crate::document::remap_group_order(docs),
    };

    // The face/slice graph: bad ids, cycles and undeclared slices reached from
    // a `face` line are reported by `FaceSet` itself.
    issues.extend(cx.docset.to_issues(&cx.faces.diagnostics));

    // Resolution is the same expansion the font build performs, so the
    // problems it detects — unresolvable references, maps that cannot be
    // synthesized, on-demand names that resolve to nothing — are reported
    // here instead of silently skipped, and this file does not reimplement
    // any of it.
    issues.extend(cx.docset.to_issues(&expansion.diagnostics));

    // Duplicate alias declarations and alias cycles; see `crate::alias`.
    issues.extend(cx.docset.to_issues(&cx.aliases.diagnostics));

    glyph_names::check_aliases(&cx, &mut issues);
    slices::check_slice_qualifiers(&cx, &mut issues);
    slices::check_name_part_bindings(&cx, &mut issues);
    slices::check_empty_slices(&cx, &mut issues);
    glyph_names::check_glyph_charset(&cx, &mut issues);
    remap::check_glyphs_and_remaps(&cx, &mut issues);
    directives::check_audit(&cx, &mut issues);
    directives::check_meta(&cx, &mut issues);
    let mapped_glyphs = maps::check_maps(&cx, &mut issues);
    unused::check_unused_glyphs(&cx, mapped_glyphs, &mut issues);
    anchors::check_ambiguous_anchors(&cx, &mut issues);
    anchors::check_anchor_derivation(&cx, &mut issues);
    colors::check_colors(&cx, &mut issues);
    patterns::check_props(docs, &mut issues);
    maps::check_uvs_maps(&cx, &mut issues);
    patterns::check_ragged_patterns(docs, cx.name_parts, &mut issues);
    issues.extend(
        cx.docset
            .to_issues(&maps::uvs_collision_diagnostics(expansion)),
    );
    // A rule the GSUB builder would drop, reported from where the dropping is
    // decided rather than reimplemented here; see
    // `ttf_builder::shadowed_single_subst_rules`.
    issues.extend(
        cx.docset
            .to_issues(&crate::render::ttf_builder::shadowed_single_subst_rules(
                docs,
                cx.name_parts,
                cx.aliases,
            )),
    );

    issues.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    issues
}

impl PartialOrd for Severity {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Severity {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let rank = |s: &Severity| {
            Severity::ALL
                .iter()
                .position(|c| c == s)
                .unwrap_or(Severity::ALL.len())
        };
        rank(self).cmp(&rank(other))
    }
}

fn short_path(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
#[path = "issues_tests.rs"]
mod tests;
