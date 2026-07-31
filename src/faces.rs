//! Faces and slices: which typefaces the source describes, and what each one
//! contains.
//!
//! # The model
//!
//! A **face** is one typeface in the output — a standalone font file, or one
//! font inside a collection. A **slice** is a named group of cmap, feature and
//! assertion data that a face may include. Slices deliberately do *not* contain
//! glyphs: every face draws from the same glyph set, and what differs between
//! two faces is which character maps to which glyph.
//!
//! An unnameable **base slice** is in every face. Everything written without a
//! qualifier belongs to it, which is why a source with no `face` line at all
//! still builds exactly as it did before faces existed.
//!
//! # Single assignment, again
//!
//! Every conflict is an error and there is no override rule — the same
//! principle [`crate::meta`] rests on. Concretely: no face may include two
//! slices that map the same character, and no slice may re-state a mapping the
//! base already has.
//!
//! That last rule has a consequence worth stating plainly, because it shapes
//! how a font is written:
//!
//! > **A character whose mapping differs between faces must not be in the base
//! > slice at all.**
//!
//! So splitting a font by, say, East Asian ambiguous width means moving those
//! characters out of the base into two slices, one per face — not adding an
//! override on top of the base. That is more work up front and much less
//! guessing later: which characters vary is visible in the source instead of
//! being the emergent result of a precedence rule.
//!
//! `slice A = B C` is shorthand for "A also includes B and C", transitively.
//! It is not a precedence mechanism either; a conflict reached through it is
//! the same error as any other.
//!
//! # Face ids are file names
//!
//! `--output unison-%.ttf` puts a face id in a path, so ids are bounded more
//! tightly than other names: [`is_valid_face_id`]. Uniqueness is checked
//! case-insensitively because the development platform's file system is, and
//! `unison-A.ttf` and `unison-a.ttf` would otherwise overwrite each other.

use std::collections::{BTreeMap, BTreeSet};

use crate::document::{Document, DocumentItem};
use crate::resolve::{Diagnostic, ItemRef};

/// One typeface, with its slice set already resolved through inheritance.
#[derive(Clone, Debug)]
pub struct Face {
    /// Empty for the implicit face of a source that declares none.
    pub id: String,
    /// Every slice this face includes, inheritance expanded. The base slice is
    /// not in here; it is implicit and [`Face::includes`] answers for it.
    pub slices: BTreeSet<String>,
    pub origin: Option<ItemRef>,
}

impl Face {
    /// Whether an item qualified with `slice` belongs to this face. `None` is
    /// the base slice, which every face includes.
    pub fn includes(&self, slice: Option<&str>) -> bool {
        match slice {
            None => true,
            Some(s) => self.slices.contains(s),
        }
    }

    /// Whether this face satisfies an `assert ... for SLICE...` constraint.
    pub fn includes_all(&self, slices: &[String]) -> bool {
        slices.iter().all(|s| self.slices.contains(s))
    }

    /// How the face is named in a diagnostic.
    pub fn label(&self) -> &str {
        if self.id.is_empty() { "<the font>" } else { &self.id }
    }
}

/// Face ids reach the file system through `--output ...-%.ttf`.
///
/// Deliberately narrower than a glyph name: no path separator, nothing that
/// makes a hidden or relative file name, and no `%` so the output pattern
/// cannot be re-triggered by a face's own id.
pub fn is_valid_face_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && !id.starts_with('.')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

/// Slice ids never become file names, so they only have to be names.
pub fn is_valid_slice_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

/// Every face the source declares, plus the slice declarations behind them.
pub struct FaceSet {
    /// In declaration order, which is the output order. A source declaring no
    /// face gets one implicit face with an empty id.
    pub faces: Vec<Face>,
    /// Declared slices and what each one inherits, before expansion.
    pub declared: BTreeMap<String, (Vec<String>, ItemRef)>,
    /// Problems found while building the graph, for [`crate::issues`] to
    /// report. Collected rather than raised so that this can run on an editor
    /// frame without deciding anything.
    pub diagnostics: Vec<Diagnostic>,
}

impl FaceSet {
    /// The face a single-face build emits: the first declared, or the implicit
    /// one. Which face is first is user-visible, so it is declaration order and
    /// not something sorted.
    pub fn primary(&self) -> &Face {
        &self.faces[0]
    }

    pub fn collect(docs: &[&Document]) -> Self {
        let mut declared: BTreeMap<String, (Vec<String>, ItemRef)> = BTreeMap::new();
        let mut face_decls: Vec<(String, Vec<String>, ItemRef)> = Vec::new();
        let mut diagnostics = Vec::new();

        for (doc_idx, doc) in docs.iter().enumerate() {
            for (item_idx, item) in doc.items.iter().enumerate() {
                let here = ItemRef::new(doc_idx, item_idx);
                match item {
                    DocumentItem::Slice { id, inherits, .. } => {
                        if !is_valid_slice_id(id) {
                            diagnostics.push(Diagnostic::error(
                                here,
                                format!(
                                    "slice id `{id}` may only contain letters, digits, `-`, `.` and `_`",
                                ),
                            ));
                            continue;
                        }
                        if declared.contains_key(id) {
                            diagnostics.push(Diagnostic::error(
                                here,
                                format!("slice `{id}` is declared more than once"),
                            ));
                            continue;
                        }
                        declared.insert(id.clone(), (inherits.clone(), here));
                    }
                    DocumentItem::Face { id, slices, .. } => {
                        if !is_valid_face_id(id) {
                            diagnostics.push(Diagnostic::error(
                                here,
                                format!(
                                    "face id `{id}` may only contain letters, digits, `-`, `.` \
                                     and `_`, and may not start with `.` — it becomes a file name",
                                ),
                            ));
                            continue;
                        }
                        if let Some((prev, _, _)) =
                            face_decls.iter().find(|(f, _, _)| f.eq_ignore_ascii_case(id))
                        {
                            diagnostics.push(Diagnostic::error(
                                here,
                                format!(
                                    "face `{id}` is declared more than once (`{prev}` differs only \
                                     in case, and face ids become file names)",
                                ),
                            ));
                            continue;
                        }
                        face_decls.push((id.clone(), slices.clone(), here));
                    }
                    _ => {}
                }
            }
        }

        // Cycles are a problem with the declarations, not with any face that
        // happens to reach them, so they are found over every declared slice.
        // Otherwise adding a `face` line would be what makes an existing cycle
        // appear, which is a confusing place to learn about it.
        //
        // Undeclared references are *not* reported here: `issues` already
        // reports those against the line that names them, which is a better
        // position than the face that transitively reached one.
        for (name, (_, origin)) in &declared {
            let mut resolved = BTreeSet::new();
            let mut visiting: Vec<String> = Vec::new();
            let mut cycle_only = Vec::new();
            expand_slice(&declared, name, &mut resolved, &mut visiting, *origin, &mut cycle_only);
            diagnostics.extend(cycle_only.into_iter().filter(|d| d.message.contains("cycle")));
        }

        // Expand inheritance per face. Done here rather than once per slice so
        // that a cycle is reported against the face that reaches it, which is
        // where the fix has to happen.
        let mut faces: Vec<Face> = Vec::new();
        for (id, roots, origin) in &face_decls {
            let mut resolved = BTreeSet::new();
            let mut visiting: Vec<String> = Vec::new();
            for root in roots {
                expand_slice(
                    &declared,
                    root,
                    &mut resolved,
                    &mut visiting,
                    *origin,
                    &mut diagnostics,
                );
            }
            faces.push(Face { id: id.clone(), slices: resolved, origin: Some(*origin) });
        }

        if faces.is_empty() {
            // No `face` line: one face carrying the base slice, which is what
            // every `.unf` written before faces existed describes.
            faces.push(Face { id: String::new(), slices: BTreeSet::new(), origin: None });
        }

        Self { faces, declared, diagnostics }
    }
}

/// Depth-first expansion with cycle detection.
///
/// A slice is inserted into `resolved` only after its parents are, so a name
/// still on `visiting` when it comes up again is a back edge — a real cycle,
/// not the diamond that `slice a = b c` with `b = d` and `c = d` produces.
///
/// A cycle is well-defined as a set union, so this could quietly succeed; it is
/// an error because two slices that include each other are the same slice, and
/// nobody writes that on purpose.
fn expand_slice(
    declared: &BTreeMap<String, (Vec<String>, ItemRef)>,
    name: &str,
    resolved: &mut BTreeSet<String>,
    visiting: &mut Vec<String>,
    origin: ItemRef,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if resolved.contains(name) {
        return;
    }
    if let Some(pos) = visiting.iter().position(|v| v == name) {
        let mut loop_names: Vec<&str> = visiting[pos..].iter().map(String::as_str).collect();
        loop_names.push(name);
        diagnostics.push(Diagnostic::error(
            origin,
            format!("slice inheritance cycle: `{}`", loop_names.join("` -> `")),
        ));
        return;
    }
    let Some((inherits, _)) = declared.get(name) else {
        // Reported here rather than at the referring line when it is a `face`
        // that reached it; `issues` catches the direct references.
        diagnostics.push(Diagnostic::error(
            origin,
            format!("undeclared slice `{name}`"),
        ));
        return;
    };
    visiting.push(name.to_string());
    for parent in inherits {
        expand_slice(declared, parent, resolved, visiting, origin, diagnostics);
    }
    visiting.pop();
    resolved.insert(name.to_string());
}

#[cfg(test)]
#[path = "faces_tests.rs"]
mod tests;
