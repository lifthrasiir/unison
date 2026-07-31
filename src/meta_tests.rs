//! Tests for [`super`]: `meta` parsing and the name records it derives.
//!
//! Declared as a child module through `#[path]`, so it reaches private items
//! while keeping the source at a readable size.

use super::*;
use crate::document_io::parse_document_from_str;

fn meta_of(src: &str) -> FontMeta {
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    FontMeta::collect(&[&doc])
}

#[test]
fn metrics_parse_as_before() {
    let m = meta_of("meta height 20\nmeta ascent 16\nmeta descent 4\n");
    assert_eq!((m.height(), m.ascent(), m.descent()), (20, 16, 4));
}

#[test]
fn defaults_apply_when_nothing_is_declared() {
    let m = meta_of("");
    assert_eq!((m.height(), m.ascent(), m.descent()), (16, 14, 2));
    assert_eq!(m.family(), DEFAULT_FAMILY);
    assert_eq!(m.subfamily(), DEFAULT_SUBFAMILY);
}

/// A backtick-quoted value is one token, which is the only way a name with
/// spaces can be written.
#[test]
fn a_quoted_name_is_one_value() {
    let m = meta_of("meta family `Unison Mono`\n");
    assert_eq!(m.family(), "Unison Mono");
    assert_eq!(
        parse_meta_entry("family Unison Mono").unwrap_err(),
        "`meta family` takes exactly 1 value, got 2 — quote a value \
         containing spaces with backticks",
    );
}

/// `family` and `name 1` are two spellings of one slot, so they must produce
/// the same slot key — that is what makes declaring both a conflict.
#[test]
fn a_named_key_and_its_name_id_share_a_slot() {
    let a = parse_meta_entry("family `Unison`").unwrap();
    let b = parse_meta_entry("name 1 `Unison`").unwrap();
    assert_eq!(a.slot(), b.slot());
    assert_eq!(a.slot(), "name 1 @en-US");
}

#[test]
fn a_language_tag_files_the_record_under_its_windows_id() {
    let m = meta_of("meta family `Unison`\nmeta family @ko-KR `유니슨`\n");
    assert_eq!(m.names.get(&(1, LANG_EN_US)).map(String::as_str), Some("Unison"));
    assert_eq!(m.names.get(&(1, 0x0412)).map(String::as_str), Some("유니슨"));
    // Two languages are two slots, so this is not a conflict.
    assert_ne!(
        parse_meta_entry("family `a`").unwrap().slot(),
        parse_meta_entry("family @ko-KR `a`").unwrap().slot(),
    );
}

#[test]
fn an_unmapped_language_tag_is_rejected() {
    let err = parse_meta_entry("family @xx-YY `a`").unwrap_err();
    assert!(err.contains("unmapped language tag"), "got {err}");
}

/// A localized name falls back to en-US, so everything derived from the family
/// has one definition even in a font with several localized families.
#[test]
fn a_missing_localization_falls_back_to_en_us() {
    let m = meta_of("meta family `Unison`\n");
    assert_eq!(m.name(1, 0x0412), Some("Unison"));
}

#[test]
fn name_ids_3_4_5_6_are_derived_when_absent() {
    let m = meta_of("meta family `Unison`\nmeta revision 1.25\n");
    assert_eq!(m.version_text(), "Version 1.250");
    assert_eq!(m.full_name(), "Unison", "Regular is dropped from the full name");
    assert_eq!(m.postscript_name(), "Unison");
    assert_eq!(m.unique_id(), "Version 1.250;NONE;Unison");

    let m = meta_of("meta family `Unison`\nmeta subfamily `Bold`\nmeta vendor-id UNSN\n");
    assert_eq!(m.full_name(), "Unison Bold");
    assert_eq!(m.postscript_name(), "Unison-Bold");
    assert_eq!(m.unique_id(), "Version 1.000;UNSN;Unison-Bold");
}

#[test]
fn an_explicit_name_id_wins_over_the_derived_one() {
    let m = meta_of("meta family `Unison`\nmeta name 6 `Unison-Custom`\n");
    assert_eq!(m.postscript_name(), "Unison-Custom");
    let recs = m.name_records();
    assert_eq!(
        recs.iter().filter(|(id, _, _)| *id == 6).count(),
        1,
        "the derived record must not be emitted alongside the declared one",
    );
}

/// The PostScript charset is restricted; the derived name only has to be
/// valid, so the filter drops what it cannot carry rather than failing.
#[test]
fn a_derived_postscript_name_is_filtered_to_its_charset() {
    let m = meta_of("meta family `Uni (son)/[x] 유니`\n");
    let ps = m.postscript_name();
    assert!(
        ps.chars().all(|c| c.is_ascii_graphic()
            && !matches!(c, '[' | ']' | '(' | ')' | '{' | '}' | '<' | '>' | '/' | '%')),
        "got {ps}",
    );
    // Spaces are not legal in a PostScript name and the subfamily is the
    // default, so there is no separator to keep either.
    assert_eq!(ps, "Unisonx");
}

/// The format requires name records in platform, encoding, language, name ID
/// order; platform and encoding are constant here, so the key is
/// `(language, name ID)`. `write-fonts` refuses to build an unsorted table.
#[test]
fn name_records_are_sorted_by_language_then_id() {
    let m = meta_of("meta family @ko-KR `유니슨`\nmeta family `Unison`\nmeta copyright `c`\n");
    let recs = m.name_records();
    let keys: Vec<(u16, u16)> = recs.iter().map(|&(id, lang, _)| (lang, id)).collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted);
}

#[test]
fn vendor_id_is_bounded_to_a_four_byte_tag() {
    assert!(parse_meta_entry("vendor-id UNSN").is_ok());
    assert!(parse_meta_entry("vendor-id TOOLONG").is_err());
    assert!(parse_meta_entry("vendor-id `유니`").is_err());
}

#[test]
fn revision_must_be_a_positive_number() {
    assert_eq!(parse_meta_entry("revision 1.5"), Ok(MetaEntry::Revision(1.5)));
    assert!(parse_meta_entry("revision 0").is_err());
    assert!(parse_meta_entry("revision -1").is_err());
    assert!(parse_meta_entry("revision x").is_err());
}
