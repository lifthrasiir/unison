//! Following a Ctrl/Cmd+click on a glyph reference written as a **pattern**.
//!
//! Most of `font/` refers to glyphs by pattern rather than by name — a `ref
//! han-xxxx-($-1):15x16` under a `glyph han-xxxx-(g|h|t|j|k|p|v):15x16` header
//! names seven glyphs at once — so "go to definition" on one of those tokens
//! used to have nothing to go to and fell back to listing the name's
//! appearances. That fallback is still what a reference with no target does;
//! what this module adds is the step *before* it, which is what makes the click
//! a jump again in the case that matters.
//!
//! The step is: expand the token to the names it denotes, find where each of
//! them is declared, and group the expansions by the answer. Whether the reader
//! is offered a choice at all is then a property of the *font*, not of the
//! token: seven names all declared by one `glyph han-xxxx-($han-regions)` block
//! are one group and the click simply jumps, while the same seven split over
//! two blocks are two groups and the reader picks. That is the whole point of
//! grouping rather than listing the expansions — a pattern over a region list
//! would otherwise put seven identical rows in front of a reader who has one
//! place to go.
//!
//! Where a name is declared is decided by exactly the test the jump itself uses
//! ([`crate::editor::doc_links::find_link_target_in_doc`], through
//! [`pattern_denotes`]), so a row the popup offers is a row
//! [`super::UniformApp::goto_glyph`] can carry out. Names nothing declares are
//! dropped rather than shown: there is nothing to go to for them, and a click
//! whose *every* expansion is like that has no target at all and falls back to
//! the search, as before.
//!
//! Like the search, the walk is memory-only — open buffers as they stand, the
//! rest from the directory snapshot — because it runs on a click and the font
//! directory is routinely a network volume. See [`super::search`].

use std::path::{Path, PathBuf};

use crate::document::{DocLine, NamePartsMap};
use crate::document_io::{SourceLine, tokenize_tokens};
use crate::editor::doc_links::pattern_denotes;

use super::search::SearchText;

/// How many names a pattern may denote before the click gives up on resolving
/// it one by one and falls back to the search.
///
/// A reference names a handful of glyphs — a region list, a set of variants —
/// and a token that names hundreds is not something a reader is picking a
/// jump out of. The bound is what keeps a mistyped `($#4e00..9fff)` from
/// turning one click into a scan of the whole font per name.
const MAX_EXPANSIONS: usize = 256;

/// One place a pattern's expansions lead, and how many of them lead there.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct GotoGroup {
    /// The first expansion that landed here, and the name a jump to this row
    /// is carried out with.
    pub name: String,
    /// How many *further* names of the pattern land in the same place, so a
    /// group of three reads `[+2]`.
    pub extra: usize,
    /// The declaring file and its 1-based line, for the row to display.
    pub path: PathBuf,
    pub file_line: usize,
}

/// What a Ctrl/Cmd+click on a pattern reference resolves to.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum PatternLink {
    /// The token could not be expanded (it still names something no line binds),
    /// or nothing it denotes is declared anywhere. Either way there is no jump
    /// to make and the click lists the token's appearances instead — which is
    /// what it did before any of this existed.
    Nowhere,
    /// Every expansion is declared in the same place: no choice to offer.
    One(String),
    /// The expansions split, in the order they first appear in the pattern.
    Many(Vec<GotoGroup>),
}

/// The names a written glyph reference denotes, or `None` where the token is
/// not something this can resolve.
///
/// `captures` are the groups in force on the line — a `$-N` on a `ref` names a
/// group of the block header above it, which the line does not say (see
/// [`block_captures_at_line`]). A token still holding a `$` after substitution
/// names something nothing here binds — an `exists` capture (`($1)`), or a
/// slice-qualified `name-parts` variable, neither of which is expanded without
/// the resolve — so it is left to the fallback rather than guessed at.
pub(super) fn expand_link_token(
    token: &str,
    name_parts: &NamePartsMap,
    captures: &[Vec<String>],
) -> Option<Vec<String>> {
    let substituted =
        crate::pattern::substitute_name_parts_and_captures(token, name_parts, captures);
    if substituted.contains('$') {
        return None;
    }
    // The operand grammar, not the block-header one: a top-level `a|b` in a
    // reference is one group and not two names. `pattern_denotes` reads the
    // same token the same way — see its note on the two grammars.
    let names = crate::pattern::NamePattern::parse_element(&substituted)
        .ok()?
        .into_vec();
    (!names.is_empty() && names.len() <= MAX_EXPANSIONS).then_some(names)
}

/// The capture groups in force at `line`: the ones the enclosing `glyph` block
/// header wrote, unless the line writes a pattern of its own.
///
/// The carry is the one [`super::search::LineCarry`] keeps while it walks a
/// file, for the same reason: the groups a `$-N` names are written on another
/// line, so a line cannot answer for itself.
pub(super) fn block_captures_at_line(
    lines: &[DocLine],
    line: usize,
    name_parts: &NamePartsMap,
) -> Vec<Vec<String>> {
    let mut at_base: Option<String> = None;
    let mut captures: Vec<Vec<String>> = Vec::new();
    for (i, doc_line) in lines.iter().enumerate() {
        let Some(text) = doc_line.as_text() else {
            continue;
        };
        if i == line {
            // A `glyph A = B` or a `map C = G` writes its pattern and names it
            // again on the same line, so it binds the groups itself; anything
            // else is under the block header's.
            return super::search::line_captures(text, at_base.as_deref(), name_parts)
                .unwrap_or(captures);
        }
        // After the line above is done with it, exactly as the parser reads a
        // header's own `@` against the base that was already in force.
        super::search::advance_at_base(&mut at_base, text);
        super::search::advance_block_captures(&mut captures, text, at_base.as_deref(), name_parts);
    }
    captures
}

/// How one line is read: the `name-parts` bindings in force over its names and
/// the capture groups it binds, which depend on those bindings in turn.
///
/// There is one reading per slice of a `SLICE :` qualifier, because the line is
/// stated once per slice — see [`readings_at_line`].
pub(super) struct Reading<'a> {
    pub parts: &'a NamePartsMap,
    pub captures: Vec<Vec<String>>,
}

/// How the token written on `line` is to be read, once per slice the line is
/// stated for.
///
/// A `map wide|narrow : ⁂ = triple-star($-half)` writes its target with the
/// parts those slices bind, and a scoped part is in no unqualified map — so
/// without this the token still carries a `$`, expands to nothing, and the
/// click falls back to the search. That was the one glyph-name position a
/// Ctrl/Cmd+click could not follow; every other one is unqualified and comes
/// back with the single unqualified reading.
pub(super) fn readings_at_line<'a>(
    lines: &[DocLine],
    line: usize,
    scoped: &'a crate::document::SliceNameParts,
) -> Vec<Reading<'a>> {
    let text = lines.get(line).and_then(DocLine::as_text).unwrap_or("");
    scoped
        .for_each_slice(&crate::editor::line_fields::qualifier_slices(text))
        .into_iter()
        .map(|parts| Reading {
            parts,
            // Per reading, not once: the groups a `map` binds are read off its
            // own character spec, which a slice-scoped part can be written in.
            captures: block_captures_at_line(lines, line, parts),
        })
        .collect()
}

/// Where each of `names` is declared, in the same order, `None` for a name
/// nothing declares.
///
/// The scan is deliberately the one `find_link_target_in_doc` makes — first
/// `glyph` line of the first file whose token denotes the name, with no `@`
/// expansion — because a row this produces has to be a jump that path can then
/// carry out. Every name is looked for in one pass over the files, and a name
/// already found is skipped, so a pattern over a region list costs one walk and
/// not one per region.
fn locate_declarations(
    files: &[(PathBuf, SearchText<'_>)],
    names: &[String],
    name_parts: &NamePartsMap,
) -> Vec<Option<(PathBuf, usize)>> {
    let mut found: Vec<Option<(PathBuf, usize)>> = vec![None; names.len()];
    for (path, text) in files {
        if found.iter().all(Option::is_some) {
            break;
        }
        scan_file(path, text, names, name_parts, &mut found);
    }
    found
}

fn scan_file(
    path: &Path,
    text: &SearchText<'_>,
    names: &[String],
    name_parts: &NamePartsMap,
    found: &mut [Option<(PathBuf, usize)>],
) {
    let mut exists = crate::exists::Carry::default();
    let mut visit = |file_line: usize, line: &str| {
        exists.enter(line);
        let trimmed = line.trim();
        // Only a `glyph` line declares anything, and this runs over every line
        // of every file; the cheap prefix test is what keeps the tokenizer off
        // the rest of them.
        if !trimmed.starts_with("glyph") {
            return;
        }
        let Ok(tokens) = tokenize_tokens(trimmed) else {
            return;
        };
        if tokens.first().is_none_or(|t| t != "glyph") {
            return;
        }
        let Some(written) = tokens.get(1) else { return };
        for (i, name) in names.iter().enumerate() {
            if found[i].is_some() {
                continue;
            }
            if written == name
                || pattern_denotes(written, true, name, name_parts, exists.pattern(), &[])
            {
                found[i] = Some((path.to_path_buf(), file_line));
            }
        }
    };
    match text {
        SearchText::Buffer(lines, doc) => {
            for (i, line) in lines.iter().enumerate() {
                if let Some(text) = line.as_text() {
                    visit(doc.docline_file_line(i), text);
                }
            }
        }
        SearchText::Source(content) => {
            crate::document_io::walk_source_lines(content, |file_line, unit| {
                if let SourceLine::Text(text) = unit {
                    visit(file_line, text);
                }
            });
        }
    }
}

/// Resolves one clicked pattern reference against the files in memory.
///
/// Kept free of the application so it can be tested over plain sources; the
/// method that feeds it the open buffers is [`super::UniformApp::follow_nav_request`]'s.
pub(super) fn resolve(
    files: &[(PathBuf, SearchText<'_>)],
    token: &str,
    readings: &[Reading<'_>],
) -> PatternLink {
    // One expansion per reading, in the order the slices are written, with a
    // name a later slice repeats dropped: the reader picks a place to go, and
    // two slices that spell one name are one place.
    let mut names: Vec<String> = Vec::new();
    for reading in readings {
        let Some(expansion) = expand_link_token(token, reading.parts, &reading.captures) else {
            continue;
        };
        for name in expansion {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    if names.is_empty() {
        return PatternLink::Nowhere;
    }
    // A `glyph` line is never slice-qualified, so any reading's parts locate a
    // declaration equally well.
    let name_parts = readings[0].parts;
    let mut groups: Vec<GotoGroup> = Vec::new();
    for (name, at) in names
        .iter()
        .zip(locate_declarations(files, &names, name_parts))
    {
        // A name nothing declares is no target; a click whose every expansion
        // is like that ends as `Nowhere`, which is the search fallback.
        let Some((path, file_line)) = at else {
            continue;
        };
        match groups
            .iter_mut()
            .find(|g| g.file_line == file_line && g.path == path)
        {
            Some(group) => group.extra += 1,
            None => groups.push(GotoGroup {
                name: name.clone(),
                extra: 0,
                path,
                file_line,
            }),
        }
    }
    match groups.len() {
        0 => PatternLink::Nowhere,
        1 => PatternLink::One(groups.remove(0).name),
        _ => PatternLink::Many(groups),
    }
}

impl super::UniformApp {
    /// Where a Ctrl/Cmd+click on the pattern `token`, written on `line` of
    /// document `from_doc`, leads.
    ///
    /// Open documents are read as they stand, unsaved edits included; the rest
    /// come from the directory snapshot, never from disk. That is the same rule
    /// [`super::search`] follows and for the same reason — this runs on a click.
    pub(super) fn resolve_pattern_link(
        &self,
        from_doc: usize,
        token: &str,
        line: usize,
    ) -> PatternLink {
        let Some(doc) = self.open_documents.get(from_doc) else {
            return PatternLink::Nowhere;
        };
        // The line's own slices decide what its `$`-names mean; see
        // [`readings_at_line`].
        let readings = readings_at_line(&doc.lines, line, &self.scoped_name_parts);
        let paths: Vec<PathBuf> = self
            .collect_all_docs()
            .iter()
            .map(|doc| doc.path.clone())
            .collect();
        let files: Vec<(PathBuf, SearchText<'_>)> = paths
            .into_iter()
            .filter_map(|path| {
                let text = match self.open_documents.iter().find(|d| d.document.path == path) {
                    Some(doc) => SearchText::Buffer(&doc.lines, &doc.document),
                    None => SearchText::Source(&self.font_sources.get(&path)?.text),
                };
                Some((path, text))
            })
            .collect();
        resolve(&files, token, &readings)
    }

    /// Puts the choice in front of the reader, in the editor the click came
    /// from. The jump itself happens on whichever later frame a row is picked.
    pub(super) fn open_goto_choice(
        &mut self,
        from_doc: usize,
        groups: Vec<GotoGroup>,
        from: crate::editor::caret::Caret,
        from_offset: f32,
    ) {
        use crate::editor::goto_popup::{GotoChoice, GotoChoicePopup};

        let Some(doc) = self.open_documents.get_mut(from_doc) else {
            return;
        };
        let choices = groups
            .into_iter()
            .map(|group| GotoChoice {
                name: group.name,
                extra: group.extra,
                location: format!(
                    "{}:{}",
                    group
                        .path
                        .file_name()
                        .unwrap_or(group.path.as_os_str())
                        .to_string_lossy(),
                    group.file_line
                ),
            })
            .collect();
        let anchor = doc.editor_state.goto_anchor;
        doc.editor_state.goto_choice =
            Some(GotoChoicePopup::new(choices, anchor, from, from_offset));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(pairs: &[(&str, &[&str])]) -> NamePartsMap {
        pairs
            .iter()
            .map(|(name, values)| {
                (
                    (*name).to_string(),
                    values.iter().map(|v| (*v).to_string()).collect(),
                )
            })
            .collect()
    }

    fn resolve_in(
        sources: &[(&str, &str)],
        token: &str,
        captures: &[Vec<String>],
        name_parts: &NamePartsMap,
    ) -> PatternLink {
        resolve_readings(
            sources,
            token,
            &[Reading {
                parts: name_parts,
                captures: captures.to_vec(),
            }],
        )
    }

    fn resolve_readings(
        sources: &[(&str, &str)],
        token: &str,
        readings: &[Reading<'_>],
    ) -> PatternLink {
        let files: Vec<(PathBuf, SearchText<'_>)> = sources
            .iter()
            .map(|(path, text)| (PathBuf::from(path), SearchText::Source(text)))
            .collect();
        resolve(&files, token, readings)
    }

    fn parse(source: &str) -> crate::document::Document {
        crate::document_io::parse_document_from_str(source, PathBuf::from("a.unf")).unwrap()
    }

    fn caps(groups: &[&[&str]]) -> Vec<Vec<String>> {
        groups
            .iter()
            .map(|g| g.iter().map(|v| (*v).to_string()).collect())
            .collect()
    }

    /// The case the feature exists for: seven names, one block, no choice to
    /// make. The click is a jump and the reader never sees a popup.
    #[test]
    fn expansions_that_share_a_declaration_are_one_jump() {
        let source = "glyph han-xxxx-($han-regions):15x16 15 16\n";
        let name_parts = parts(&[("$han-regions", &["g", "h", "t", "j", "k", "p", "v"])]);
        let link = resolve_in(
            &[("han-0001.unf", source)],
            "han-xxxx-($-1):15x16",
            &caps(&[&["g", "h", "t", "j", "k", "p", "v"]]),
            &name_parts,
        );
        assert_eq!(link, PatternLink::One("han-xxxx-g:15x16".to_string()));
    }

    /// The same reference against a font that declares the regions in two
    /// blocks: two groups, in the order the pattern writes them, each naming
    /// its first expansion and counting the rest.
    #[test]
    fn expansions_declared_apart_become_one_group_each() {
        let source = "\
glyph han-xxxx-(g|h|t):15x16 15 16
glyph han-xxxx-(j|k|p|v):15x16 15 16
";
        let link = resolve_in(
            &[("han-0001.unf", source)],
            "han-xxxx-($-1):15x16",
            &caps(&[&["g", "h", "t", "j", "k", "p", "v"]]),
            &NamePartsMap::default(),
        );
        let PatternLink::Many(groups) = link else {
            panic!("two blocks declare these, so there is a choice: {link:?}");
        };
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "han-xxxx-g:15x16");
        assert_eq!(groups[0].extra, 2);
        assert_eq!(groups[0].file_line, 1);
        assert_eq!(groups[1].name, "han-xxxx-j:15x16");
        assert_eq!(groups[1].extra, 3);
        assert_eq!(groups[1].file_line, 2);
    }

    /// A group is a *place*, not a block: two files, and the rows say which.
    #[test]
    fn groups_are_told_apart_by_file_as_well_as_by_line() {
        let link = resolve_in(
            &[
                ("a.unf", "glyph foo-a 8 16\n"),
                ("b.unf", "glyph foo-b 8 16\n"),
            ],
            "foo-(a|b)",
            &[],
            &NamePartsMap::default(),
        );
        let PatternLink::Many(groups) = link else {
            panic!("declared in two files: {link:?}");
        };
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].path, PathBuf::from("a.unf"));
        assert_eq!(groups[1].path, PathBuf::from("b.unf"));
    }

    /// Names nothing declares are not rows: there is nowhere for them to go,
    /// and the group that *is* declared is still a jump without a popup.
    #[test]
    fn an_undeclared_expansion_is_not_offered() {
        let link = resolve_in(
            &[("a.unf", "glyph foo-a 8 16\n")],
            "foo-(a|b)",
            &[],
            &NamePartsMap::default(),
        );
        assert_eq!(link, PatternLink::One("foo-a".to_string()));
    }

    /// With none of them declared there is no target at all, which is where
    /// the search fallback takes over — as it did before any of this existed.
    #[test]
    fn a_pattern_declared_nowhere_has_no_target() {
        let link = resolve_in(
            &[("a.unf", "glyph bar 8 16\n")],
            "foo-(a|b)",
            &[],
            &NamePartsMap::default(),
        );
        assert_eq!(link, PatternLink::Nowhere);
    }

    /// A token still naming something nothing here binds — an `exists`
    /// capture — is left to the fallback rather than guessed at.
    #[test]
    fn a_token_that_does_not_expand_has_no_target() {
        assert_eq!(
            expand_link_token("($0)", &NamePartsMap::default(), &[]),
            None
        );
        assert_eq!(
            expand_link_token("han-($1)", &NamePartsMap::default(), &[]),
            None
        );
    }

    /// A pattern over a whole block is not a choice anyone picks out of a
    /// popup, so it is left to the fallback rather than scanned name by name.
    #[test]
    fn a_pattern_too_wide_to_choose_from_is_left_alone() {
        let name_parts = NamePartsMap::default();
        assert!(expand_link_token("han-($#4e00..4eff)", &name_parts, &[]).is_some());
        assert_eq!(
            expand_link_token("han-($#4e00..9fff)", &name_parts, &[]),
            None
        );
    }

    /// The shape `font/` really writes: the sizes a block declares come from
    /// the `exists` above it, so the scan has to carry that directive along or
    /// most of the han glyphs are declared nowhere.
    #[test]
    fn a_declaration_scoped_by_an_exists_is_found() {
        let source = "\
glyph han-5b50:9x16 9 16
exists han-5b50:([0-9]+x[0-9]+(?:-[a-z])?)
glyph han-5b50-($han-regions):($1) = ($0)
";
        let name_parts = parts(&[("$han-regions", &["g", "h", "t", "j", "k", "p", "v"])]);
        let link = resolve_in(
            &[("han-0038.unf", source)],
            "han-5b50-($-1):9x16",
            &caps(&[&["g", "h", "t", "j", "k", "p", "v"]]),
            &name_parts,
        );
        assert_eq!(link, PatternLink::One("han-5b50-g:9x16".to_string()));
    }

    /// A `map`'s target is written with the `name-parts` its own `SLICE :`
    /// qualifier binds, and a scoped part is in no unqualified map — so
    /// without the line's slices the token keeps its `$`, expands to nothing
    /// and the click falls back to the search. Every other glyph-name position
    /// is unqualified and resolves either way; this is the one that did not.
    #[test]
    fn a_slice_qualified_map_target_is_read_with_its_slices_parts() {
        let src = "\
name-parts wide : $-half = ``
name-parts narrow : $-half = -half
glyph triple-star 8 16
glyph triple-star-half 8 16
map wide|narrow : ⁂ = triple-star($-half)
";
        let docs = [parse(src)];
        let refs: Vec<&crate::document::Document> = docs.iter().collect();
        let scoped = crate::document::SliceNameParts::with_base(
            &refs,
            crate::document::collect_name_parts(&refs),
        );
        let lines: Vec<DocLine> = src.lines().map(|s| DocLine::Text(s.to_string())).collect();
        let map_line = 4;
        let readings = readings_at_line(&lines, map_line, &scoped);
        assert_eq!(readings.len(), 2, "one reading per slice of the qualifier");
        let link = resolve_readings(&[("a.unf", src)], "triple-star($-half)", &readings);
        let PatternLink::Many(groups) = link else {
            panic!("the two slices name two glyphs, declared apart: {link:?}");
        };
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "triple-star");
        assert_eq!(groups[1].name, "triple-star-half");

        // The unqualified map alone is what the click used to have, and it
        // leaves the token unexpandable.
        assert_eq!(
            expand_link_token(
                "triple-star($-half)",
                &crate::document::collect_name_parts(&refs),
                &[]
            ),
            None
        );
    }

    /// The slices are an outer loop, not one more alternation: a target that
    /// writes both a group of its own and a scoped part names every
    /// combination, and never zips the two. Folding the slices into one map
    /// would pair `0` with `wide` and `1` with `narrow` and lose half the
    /// glyphs the line maps.
    #[test]
    fn the_slices_of_a_qualifier_multiply_the_target_rather_than_zip_it() {
        let src = "\
name-parts wide : $-half = ``
name-parts narrow : $-half = -half
glyph (0|1)-circled 8 16
glyph (0|1)-circled-half 8 16
map wide|narrow : ⓪|① = ($0..1)-circled($-half)
";
        let docs = [parse(src)];
        let refs: Vec<&crate::document::Document> = docs.iter().collect();
        let scoped = crate::document::SliceNameParts::with_base(
            &refs,
            crate::document::collect_name_parts(&refs),
        );
        let lines: Vec<DocLine> = src.lines().map(|s| DocLine::Text(s.to_string())).collect();
        let readings = readings_at_line(&lines, 4, &scoped);
        let names: Vec<String> = readings
            .iter()
            .flat_map(|r| {
                expand_link_token("($0..1)-circled($-half)", r.parts, &r.captures)
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            names,
            vec!["0-circled", "1-circled", "0-circled-half", "1-circled-half"],
        );
    }

    /// A `$-N` on a `ref` names a group of the header *above* it, which the
    /// line itself does not say.
    #[test]
    fn the_captures_in_force_come_from_the_block_header() {
        let lines: Vec<DocLine> = [
            "glyph other-(x|y) 8 16",
            "glyph han-xxxx-(g|h|t):15x16 15 16",
            "ref han-yyyy-($-1):15x16 0 0",
        ]
        .iter()
        .map(|s| DocLine::Text((*s).to_string()))
        .collect();
        assert_eq!(
            block_captures_at_line(&lines, 2, &NamePartsMap::default()),
            caps(&[&["g", "h", "t"]]),
        );
        // The header's own groups are its own, not the previous block's.
        assert_eq!(
            block_captures_at_line(&lines, 1, &NamePartsMap::default()),
            caps(&[&["x", "y"]]),
        );
    }

    /// An alias writes its pattern and names it again on the same line, so the
    /// groups a `$-N` after the `=` names are that line's own.
    #[test]
    fn an_alias_line_binds_its_own_groups() {
        let lines: Vec<DocLine> = [
            "glyph han-xxxx-(g|h|t):15x16 15 16",
            "glyph han-yyyy-(j|k):15x16 = han-zzzz-($-1):15x16",
        ]
        .iter()
        .map(|s| DocLine::Text((*s).to_string()))
        .collect();
        assert_eq!(
            block_captures_at_line(&lines, 1, &NamePartsMap::default()),
            caps(&[&["j", "k"]]),
        );
    }
}
