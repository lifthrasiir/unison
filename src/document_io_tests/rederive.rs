//! `rederive_document`: parsing again only what an edit reached has to agree
//! with parsing the whole buffer, whatever the edit made of the line.

use super::*;

/// Sources for the edits below: every `.unf` the goldens run over, and one
/// that leans on the `@` base, which is the state a reparse has to restart.
fn sources() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let mut out: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "unf"))
        .map(|p| {
            (
                p.display().to_string(),
                std::fs::read_to_string(&p).unwrap(),
            )
        })
        .collect();
    out.sort();
    out.push((
        "at-base".into(),
        "glyph base 2 2\n@@..\n..@@\n\nglyph @.alt 2 2\n@@@@\n....\n// between\n\
         glyph other\nref @.alt 0 0\nglyph @.x = base\n## section\nglyph @.y 1 1\n@@\n"
            .into(),
    ));
    out
}

/// What each line is turned into: nothing, and each kind of line that starts,
/// ends or continues an item, or moves the `@` base.
fn variants(line: &str) -> Vec<String> {
    vec![
        String::new(),
        "// c".into(),
        "## h".into(),
        "glyph @.alt 2 2".into(),
        "glyph fresh".into(),
        "ref base 0 0".into(),
        "map A = base".into(),
        "|| more".into(),
        format!("{line} x"),
        line.chars().take(line.chars().count() / 2).collect(),
    ]
}

fn assert_same(what: &str, lines: &[DocLine], old: &Document) {
    let (whole, _) = derive_document(lines, "test.unf".into()).unwrap();
    let (part, rebuild, reparse) = rederive_document(old.clone(), lines);
    assert_eq!(part.items, whole.items, "{what}: items");
    assert_eq!(
        part.item_line_starts, whole.item_line_starts,
        "{what}: item starts"
    );
    assert_eq!(part.at_bases, whole.at_bases, "{what}: @ bases");
    assert_eq!(
        part.docline_file_lines, whole.docline_file_lines,
        "{what}: file lines"
    );
    assert_eq!(part.line_fps, whole.line_fps, "{what}: fingerprints");
    assert_eq!(
        rebuild,
        crate::document::items_changed_for_rebuild(&old.items, &whole.items),
        "{what}: whether a rebuild is due"
    );
    let reparse = reparse.expect("same length, so never derived whole");
    assert_eq!(
        reparse.old.start, reparse.new.start,
        "{what}: the items before the edit are kept"
    );
    assert_eq!(
        old.items.len() - reparse.old.len(),
        whole.items.len() - reparse.new.len(),
        "{what}: the items outside are the old ones"
    );
}

#[test]
fn a_reparse_of_one_line_agrees_with_a_whole_derive() {
    for (name, src) in sources() {
        let lines = parse_doclines(&src);
        let (old, _) = derive_document(&lines, "test.unf".into()).unwrap();
        for i in 0..lines.len() {
            let original = lines[i].as_text().unwrap_or("").to_string();
            for variant in variants(&original) {
                let mut edited = lines.clone();
                edited[i] = DocLine::text(variant.as_str());
                assert_same(&format!("{name}:{i} -> {variant:?}"), &edited, &old);
            }
        }
    }
}

/// Two edits a few lines apart, as a paste or an undo makes: one reparse
/// spans both.
#[test]
fn a_reparse_of_two_lines_agrees_with_a_whole_derive() {
    for (name, src) in sources() {
        let lines = parse_doclines(&src);
        let (old, _) = derive_document(&lines, "test.unf".into()).unwrap();
        for i in 0..lines.len().saturating_sub(3) {
            let mut edited = lines.clone();
            edited[i] = DocLine::text("glyph fresh");
            edited[i + 3] = DocLine::text("");
            assert_same(&format!("{name}:{i},{}", i + 3), &edited, &old);
        }
    }
}

/// An unchanged buffer is no reparse at all, and a buffer that changed length
/// is a whole one.
#[test]
fn nothing_changed_and_a_changed_length_are_the_two_ends() {
    let lines = parse_doclines("glyph a 1 1\n@@\n// c\n");
    let (old, _) = derive_document(&lines, "test.unf".into()).unwrap();
    let (same, rebuild, reparse) = rederive_document(old.clone(), &lines);
    assert_eq!(same.items, old.items);
    assert!(!rebuild);
    assert_eq!(reparse.map(|r| (r.old, r.new)), Some((0..0, 0..0)));

    let mut longer = lines.clone();
    longer.push(DocLine::text("// more"));
    let (whole, rebuild, reparse) = rederive_document(old, &longer);
    assert_eq!(whole.items.len(), 3);
    assert!(rebuild, "one item more is a change to the item list");
    assert_eq!(reparse, None);
}
