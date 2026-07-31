//! Tests for [`super`]: the face/slice graph and the rules it enforces.

use super::*;
use crate::document_io::parse_document_from_str;
use crate::issues::{Severity, collect_issues};

fn faces_of(src: &str) -> FaceSet {
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    FaceSet::collect(&[&doc])
}

fn errors(src: &str) -> Vec<String> {
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    collect_issues(&[&doc])
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.message)
        .collect()
}

/// With nothing declared there is still exactly one face, carrying the base
/// slice — which is what keeps every existing `.unf` building unchanged.
#[test]
fn an_undeclared_font_has_one_implicit_face() {
    let f = faces_of("glyph a 1 1\n@@\nmap A = a\n");
    assert_eq!(f.faces.len(), 1);
    assert!(f.faces[0].id.is_empty(), "the implicit face has no id");
    assert!(f.faces[0].includes(None), "the base slice");
}

/// The base slice is in every face and cannot be named, so a slice-qualified
/// line is always *additional* to it.
#[test]
fn the_base_slice_is_in_every_face() {
    let f = faces_of("slice wide\nface narrow\nface wide : wide\n");
    assert_eq!(f.faces.len(), 2);
    for face in &f.faces {
        assert!(face.includes(None), "{} is missing the base slice", face.id);
    }
    assert!(!f.faces[0].includes(Some("wide")));
    assert!(f.faces[1].includes(Some("wide")));
}

/// Face order is declaration order: it is user-visible, because a consumer that
/// does not choose picks the first one.
#[test]
fn faces_keep_their_declaration_order() {
    let f = faces_of("face c\nface a\nface b\n");
    let ids: Vec<&str> = f.faces.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(ids, ["c", "a", "b"]);
}

/// `slice A = B C` is shorthand for including both, transitively. It is not a
/// precedence mechanism — there is none.
#[test]
fn slice_inheritance_is_transitive() {
    let f = faces_of("slice d\nslice b = d\nslice c\nslice a = b c\nface f : a\n");
    let face = &f.faces[0];
    for s in ["a", "b", "c", "d"] {
        assert!(face.includes(Some(s)), "expected {s}");
    }
}

/// A cycle is a problem with the declarations, so it is found whether or not a
/// face reaches it — adding a `face` line should not be what surfaces one.
#[test]
fn an_inheritance_cycle_is_an_error_even_unused() {
    for src in ["slice a = b\nslice b = a\nface f : a\n", "slice a = b\nslice b = a\n"] {
        let msgs = errors(src);
        assert!(
            msgs.iter().any(|m| m.contains("cycle")),
            "expected a cycle error for {src:?}, got {msgs:?}",
        );
    }
}

/// A diamond is not a cycle: `b` and `c` both including `d` is the ordinary
/// shape of shorthand, and reporting it would make the feature unusable.
#[test]
fn a_diamond_is_not_a_cycle() {
    let msgs = errors("slice d\nslice b = d\nslice c = d\nslice a = b c\nface f : a\n");
    assert!(
        !msgs.iter().any(|m| m.contains("cycle")),
        "a diamond must not report a cycle, got {msgs:?}",
    );
}

#[test]
fn referring_to_an_undeclared_slice_is_an_error() {
    for src in [
        "face f : nope\n",
        "slice a = nope\nface f : a\n",
        "map nope : A = a\nglyph a 1 1\n@@\n",
    ] {
        let msgs = errors(src);
        assert!(
            msgs.iter().any(|m| m.contains("nope")),
            "expected an undeclared-slice error for {src:?}, got {msgs:?}",
        );
    }
}

/// Face ids become file names through `--output unison-%.ttf`, so they are
/// bounded more tightly than other names, and two ids that differ only in case
/// would collide on a case-insensitive file system.
#[test]
fn face_ids_are_bounded_and_case_insensitively_unique() {
    for bad in ["face .hidden\n", "face ..\n", "face a/b\n", "face `a b`\n", "face a%b\n"] {
        assert!(
            !errors(bad).is_empty(),
            "expected {bad:?} to be rejected",
        );
    }
    let msgs = errors("face Narrow\nface narrow\n");
    assert!(
        msgs.iter().any(|m| m.contains("more than once") || m.contains("case")),
        "expected a case-insensitive collision error, got {msgs:?}",
    );
}

/// The whole point of the split: two slices may map the same character, as long
/// as no single face includes both. When one does, it is a conflict — and there
/// is no override rule to fall back on, by design.
#[test]
fn the_same_character_in_two_slices_of_one_face_is_a_conflict() {
    let ok = "\
slice narrow
slice wide
glyph a 1 1
@@
map narrow : ° = a
map wide : ° = a
face narrow : narrow
face wide : wide
";
    // Two faces cannot be *built* yet, which is its own error; what matters
    // here is that the shared mapping is not one of the complaints.
    assert!(
        !errors(ok).iter().any(|m| m.contains("U+00B0")),
        "disjoint faces are fine, got {:?}",
        errors(ok),
    );

    let bad = ok.replace("face wide : wide\n", "face both : narrow wide\n");
    let msgs = errors(&bad);
    assert!(
        msgs.iter().any(|m| m.contains("both")),
        "expected a per-face conflict naming the face, got {msgs:?}",
    );
}

/// A character in the base slice cannot be re-stated in a slice, because the
/// base is in every face. This is the invariant that forces characters whose
/// mapping varies per face out of the base entirely.
#[test]
fn a_slice_cannot_restate_a_base_mapping() {
    let msgs = errors(
        "slice wide\nglyph a 1 1\n@@\nmap ° = a\nmap wide : ° = a\nface f : wide\n",
    );
    assert!(!msgs.is_empty(), "expected a conflict, got none");
}

/// An assertion whose slice combination no face satisfies would never run.
/// Silently skipping it is the worst failure mode a test suite can have.
#[test]
fn an_assertion_no_face_can_satisfy_is_an_error() {
    let msgs = errors(
        "slice a\nslice b\nface f : a\nglyph x 1 1\n@@\nmap A = x\n\
         assert shape A for a b : x\n",
    );
    assert!(
        msgs.iter().any(|m| m.contains("no face")),
        "expected a no-matching-face error, got {msgs:?}",
    );
}

/// `meta` obeys the same single-assignment rule slices do: a bare key reaches
/// every face, so stating it bare *and* for a face gives that face two values
/// with no rule to choose between them.
#[test]
fn a_bare_and_a_face_scoped_meta_key_conflict() {
    let msgs = errors("face wide\nmeta family `Unison`\nmeta wide : family `Unison Wide`\n");
    assert!(
        msgs.iter().any(|m| m.contains("wide")),
        "expected a per-face meta conflict, got {msgs:?}",
    );

    // Stating it once per face is the way to write it, and is not a conflict.
    let msgs = errors(
        "face narrow\nface wide\n\
         meta narrow : family `Unison`\nmeta wide : family `Unison Wide`\n",
    );
    assert!(
        !msgs.iter().any(|m| m.contains("name ID")),
        "two faces stating their own family is fine, got {msgs:?}",
    );
}

#[test]
fn meta_for_an_undeclared_face_is_an_error() {
    let msgs = errors("face wide\nmeta nosuch : family `x`\n");
    assert!(
        msgs.iter().any(|m| m.contains("nosuch")),
        "expected an unknown-face error, got {msgs:?}",
    );
}

/// Two faces the OS cannot tell apart are two faces the user cannot pick
/// between; a duplicate PostScript name also breaks PDF embedding.
#[test]
fn two_faces_may_not_share_a_name() {
    let msgs = errors(
        "face a\nface b\nmeta a : family `Unison`\nmeta b : family `Unison`\n",
    );
    assert!(
        msgs.iter().any(|m| m.contains("same") || m.contains("both")),
        "expected a duplicate-name error, got {msgs:?}",
    );
}
