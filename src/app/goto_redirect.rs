//! Carrying a jump on past its first landing: the `goto` ref.
//!
//! A wrapper glyph written as a pattern — `font/han.unf`'s `glyph han-($1)` over
//! `ref ($0) 1 0 goto`, under an `exists han-…:15x16` — declares thousands of
//! names on one line, so a jump to any of them would arrive at that same line
//! rather than at the drawing. `goto` on one of the block's refs says which
//! target to carry the jump on to, and this is what carries it: every hop is a
//! landing of its own, so Go Back walks the way in backwards (see
//! [`super::history`]).
//!
//! Where the redirect is read from is the whole question here, and the answer is
//! the **source**, one file at a time, exactly as the rest of the click path
//! reads it ([`super::goto_pattern`], [`super::search`]). The block that
//! declares the name is the block the jump just landed on, so its header, the
//! `exists` above it and its `ref … goto` are all on lines already in memory;
//! what the header's `($1)` stood for is recovered from the name that was jumped
//! to ([`crate::exists::template_captures`]).
//!
//! This used to read the *resolved* glyph instead, which has the expansion's own
//! answer and needs no inverse — but only while a resolve is current. A rebuild
//! is in flight after every keystroke and takes seconds over `font/`, and the
//! redirect was dropped for that whole window: the reader who clicked a
//! `// (see also han-2001c)` landed on the pattern line, and a second click a
//! moment later went where the first should have. The resolve is still the
//! fallback for a block the text cannot answer for, since it costs nothing to
//! ask when it happens to be current.

use crate::document::{DocLine, NamePartsMap};
use crate::document_io::{split_comment, starts_item, tokenize_tokens};
use crate::editor::doc_links::{LinkTargetKind, pattern_denotes};

use super::NavLoc;

impl super::UniformApp {
    /// Where a jump that has landed on `name`, at `at`, really belongs: the
    /// target of `name`'s `goto` ref, and then of *that* glyph's, for as long as
    /// the chain runs. Reports each further landing, in order, having already
    /// carried them out.
    pub(super) fn follow_goto_chain(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        at: (usize, usize),
    ) -> Vec<NavLoc> {
        /// Bounds a chain that is merely long, so that one gesture cannot
        /// fill the history.
        const MAX_HOPS: usize = 8;

        let mut landed = Vec::new();
        let mut seen: Vec<String> = vec![name.to_string()];
        let mut at = at;
        while landed.len() < MAX_HOPS {
            let Some(next) = self.goto_redirect(seen.last().expect("seeded above"), at) else {
                break;
            };
            // Defensive, not a case a source can write today: `goto` rides on
            // a `ref`, and a ref cycle never resolves. Stopping at the second
            // visit leaves the jump on the last new glyph rather than looping.
            if seen.contains(&next) {
                break;
            }
            // A target nothing declares stops the chain rather than opening
            // the Search pane: the reader asked for the glyph they clicked,
            // and it is the *source* that is inconsistent, which the issue
            // report is the place to say.
            let Some((doc_idx, line)) = self.goto_glyph(ctx, &next, &LinkTargetKind::Glyph) else {
                break;
            };
            landed.push(NavLoc::new(doc_idx, line, 0));
            at = (doc_idx, line);
            seen.push(next);
        }
        landed
    }

    /// The name the `goto` ref of the block at `at` — the one that declares
    /// `name` — points at, if it has one.
    ///
    /// `at` is `(document index, line)`, and the document is open by
    /// construction: every caller has just landed there.
    fn goto_redirect(&self, name: &str, at: (usize, usize)) -> Option<String> {
        let (doc_idx, line) = at;
        // The buffer as it stands, which is what the reader is looking at.
        if let Some(open) = self.open_documents.get(doc_idx)
            && let Some(next) = redirect_at_declaration(&open.lines, line, name, &self.name_parts)
        {
            return Some(next);
        }
        // A block the text cannot answer for — a slice-qualified `name-parts`
        // on the ref, a pattern this cannot invert — where the resolve holds the
        // expansion's own answer. Only when it is current: `named_glyphs` is one
        // build behind while a rebuild is in flight, and a stale answer would
        // send the reader to the glyph a since-edited ref used to name.
        if self.named_glyphs_gen != self.font_build_gen {
            return None;
        }
        let refs = &self.named_glyphs.get(name)?.inline_source.as_ref()?.refs;
        // The first, where a source wrote more than one; `issues::flags`
        // reports that so the two agree on which one wins.
        refs.iter().find(|r| r.goto).map(|r| r.name.clone())
    }
}

/// The target of the `goto` ref of the `glyph` block written at `line`, read off
/// the source, for a jump that landed there looking for `name`.
///
/// Three written lines answer it: the `exists` in force, the block header, and
/// the first `ref … goto` of the block's body. The header and the name it was
/// reached by give the `($N)` slots, the header's own pattern gives the `$-N`
/// back-references, and the ref's name token with both substituted is the
/// target — unless something is left unbound, which is where the caller's
/// fallback comes in.
fn redirect_at_declaration(
    lines: &[DocLine],
    line: usize,
    name: &str,
    name_parts: &NamePartsMap,
) -> Option<String> {
    let mut exists = crate::exists::Carry::default();
    for text in lines.get(..line)?.iter().filter_map(DocLine::as_text) {
        exists.enter(text);
    }
    let header = tokens_of(lines.get(line)?.as_text()?)?;
    // An alias (`glyph A = B`) is one line with no body, so it has no `goto`
    // ref and nothing below it is its own — the rule [`crate::exists::Carry`]
    // reads by too.
    if header.first().is_none_or(|t| t != "glyph") || header[2..].iter().any(|t| t == "=") {
        return None;
    }
    // The block has to be the one that declares `name` — the same test the jump
    // itself was carried out by ([`crate::editor::doc_links::find_link_target_in_doc`]),
    // so a landing this cannot recognize is one that never happened.
    let written = header.get(1)?;
    if written != name && !pattern_denotes(written, true, name, name_parts, exists.pattern(), &[]) {
        return None;
    }

    let (goto_line, target) = goto_ref_of_block(lines, line)?;
    let mut bound = name_parts.clone();
    if let Some(pattern) = exists.pattern() {
        // A header with no slot on it binds none, and the block is then an
        // ordinary one that happens to sit under an `exists`.
        for (slot, value) in crate::exists::template_captures(pattern, written, name)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            if let Some(value) = value {
                bound.insert(format!("${slot}"), vec![value]);
            }
        }
    }
    let captures = super::goto_pattern::block_captures_at_line(lines, goto_line, &bound);
    let substituted =
        crate::pattern::substitute_name_parts_and_captures(&target, &bound, &captures);
    // A `$` still standing names something nothing here binds; a pattern naming
    // several glyphs is no single target either. Both are the fallback's.
    if substituted.contains('$') {
        return None;
    }
    let mut names = crate::pattern::NamePattern::parse_element(&substituted)
        .ok()?
        .into_vec();
    (names.len() == 1).then(|| names.remove(0))
}

/// The first `ref … goto` of the block headed at `line`, as `(line, name)`.
///
/// The body is read exactly as [`crate::document_io`] parses it: the optional
/// pixel grid, then `ref`, `anchor` and IDC lines for as long as they run.
fn goto_ref_of_block(lines: &[DocLine], line: usize) -> Option<(usize, String)> {
    for (i, doc_line) in lines.iter().enumerate().skip(line + 1) {
        let DocLine::Text(text) = doc_line else {
            // The block's own pixel grid, which the body follows.
            continue;
        };
        let tokens = tokens_of(text)?;
        // A blank line ends the block, as it ends it for the parser.
        let keyword = tokens.first()?;
        if keyword == "ref" {
            if tokens[2..].iter().any(|t| t == "goto") {
                return Some((i, tokens.get(1)?.clone()));
            }
            continue;
        }
        // `anchor` and the IDC lines are body too; anything else — a directive,
        // a new block, a blank line — is past the end of it.
        let is_body = keyword == "anchor"
            || crate::compose::IdcOp::of_line(tokens.iter().map(String::as_str)).is_some();
        if !is_body || starts_item(keyword) {
            return None;
        }
    }
    None
}

/// One source line's tokens, the trailing comment dropped — a `// goto` is prose
/// and not a flag.
fn tokens_of(text: &str) -> Option<Vec<String>> {
    tokenize_tokens(split_comment(text.trim()).0).ok()
}

/// What one gesture reaches when nothing has resolved yet, which is the state
/// the editor is in after every keystroke and for the whole of a cold start.
///
/// The `font/` shape is what these are written over — `han.unf`'s one pattern
/// block above the per-character `han-XXXX.unf` files — because that is the one
/// the flag exists for, and the redirect the resolve used to answer for it was
/// the part that went missing.
#[cfg(test)]
mod tests {
    use super::redirect_at_declaration;
    use crate::app::UniformApp;
    use crate::app::background::startup_tests::TempDir;
    use crate::app::settings::Settings;
    use crate::document::{DocLine, NamePartsMap};
    use crate::document_io::parse_doclines;
    use crate::editor::doc_links::LinkTargetKind;
    use crate::editor::document_view::{GotoGlyph, NavRequest, NavTarget};

    /// The wrapper block, as `font/han.unf` writes it.
    const HAN: &str = "meta height 16\nmeta ascent 13\nmeta descent 3\n\n\
                       exists han-([0-9a-f]{4,5}(?:-[ghtjkpv])?):15x16\n\
                       glyph han-($1) 16 16 advance 16\n\
                       ref ($0) 1 0 goto // skip this glyph when jumped\n";

    /// Where the redirect lands: the file, and the line the caret is on.
    fn click(app: &mut UniformApp, ctx: &egui::Context, name: &str) -> (String, usize) {
        let from_doc = app
            .open_documents
            .iter()
            .position(|d| d.document.path.ends_with("prose.unf"))
            .expect("the file the link is written in is open");
        app.follow_nav_request(
            ctx,
            from_doc,
            NavRequest {
                from: crate::editor::caret::Caret::new(0, 20),
                from_offset: 0.0,
                target: NavTarget::CrossFile(GotoGlyph {
                    name: name.to_string(),
                    kind: LinkTargetKind::Glyph,
                }),
            },
        );
        let at = app
            .panes
            .active_doc_idx()
            .expect("the jump landed somewhere");
        let doc = &app.open_documents[at];
        (
            doc.document
                .path
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned(),
            doc.editor_state.cursor_line(),
        )
    }

    /// A Ctrl/Cmd+click on a name written in prose — `// (see also han-2001c)` —
    /// goes through the pattern block that declares it to the drawing, with
    /// nothing resolved: a freshly opened directory has read its files and
    /// built nothing.
    #[test]
    fn a_pattern_wrapper_redirects_with_no_resolve_at_all() {
        let dir = TempDir::new("goto-redirect-cold");
        std::fs::write(dir.0.join("han.unf"), HAN).unwrap();
        std::fs::write(
            dir.0.join("han-2001c.unf"),
            "glyph han-2001c:15x16 15 16 // 𠀜\n",
        )
        .unwrap();
        std::fs::write(
            dir.0.join("prose.unf"),
            "// drawn like 亜 (see also han-2001c)\nglyph other 2 2\n@@\n.@\n",
        )
        .unwrap();

        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        assert!(
            app.named_glyphs.is_empty(),
            "nothing has resolved yet, which is the state this is about"
        );
        app.open_file(dir.0.join("prose.unf"));

        assert_eq!(
            click(&mut app, &ctx, "han-2001c"),
            ("han-2001c.unf".to_string(), 0),
            "the jump goes through `glyph han-($1)` to `glyph han-2001c:15x16`"
        );
        // The wrapper the jump passed through is still one Go Back away.
        app.navigate_history(&ctx, false);
        let at = app.panes.active_doc_idx().unwrap();
        assert!(app.open_documents[at].document.path.ends_with("han.unf"));
        assert_eq!(app.open_documents[at].editor_state.cursor_line(), 5);
    }

    /// Two slots, and a `$0` rebuilt from both: `han.unf`'s variation-selector
    /// block (`glyph han-($1).($2)`).
    #[test]
    fn a_header_with_two_slots_rebuilds_the_name_the_search_matched() {
        let dir = TempDir::new("goto-redirect-slots");
        std::fs::write(
            dir.0.join("han.unf"),
            "meta height 16\nmeta ascent 13\nmeta descent 3\n\n\
             exists han-([0-9a-f]{4,5})\\.([0-1]?[0-9a-f]):15x16\n\
             glyph han-($1).($2) 16 16 advance 16\nref ($0) 1 0 goto\n",
        )
        .unwrap();
        std::fs::write(
            dir.0.join("han-4e00.unf"),
            "glyph han-4e00.01:15x16 15 16\n",
        )
        .unwrap();
        std::fs::write(dir.0.join("prose.unf"), "// see also han-4e00.01\n").unwrap();

        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        app.open_file(dir.0.join("prose.unf"));

        assert_eq!(
            click(&mut app, &ctx, "han-4e00.01"),
            ("han-4e00.unf".to_string(), 0)
        );
    }

    /// An edit the resolve has not seen yet is what the source is read for: the
    /// jump follows the `goto` ref that is on screen, not the one that was built.
    #[test]
    fn the_redirect_follows_an_unsaved_edit() {
        let dir = TempDir::new("goto-redirect-edit");
        std::fs::write(
            dir.0.join("a.unf"),
            "meta height 4\nmeta ascent 3\nmeta descent 1\n\n\
             glyph wrapper\nref one 0 0 goto\n",
        )
        .unwrap();
        std::fs::write(dir.0.join("b.unf"), "glyph one 2 2\n@@\n.@\n").unwrap();
        std::fs::write(dir.0.join("c.unf"), "glyph two 2 2\n@@\n.@\n").unwrap();
        std::fs::write(dir.0.join("prose.unf"), "// see also wrapper\n").unwrap();

        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        app.open_file(dir.0.join("prose.unf"));
        assert_eq!(click(&mut app, &ctx, "wrapper"), ("b.unf".to_string(), 0));

        // The ref is retargeted in the buffer, with nothing rebuilt.
        app.open_file(dir.0.join("a.unf"));
        let a = app
            .open_documents
            .iter_mut()
            .find(|d| d.document.path.ends_with("a.unf"))
            .unwrap();
        a.lines[5] = DocLine::text("ref two 0 0 goto".to_string());

        assert_eq!(click(&mut app, &ctx, "wrapper"), ("c.unf".to_string(), 0));
    }

    /// The block's body ends where the parser ends it, and a `goto` that is
    /// prose is not a flag.
    #[test]
    fn what_counts_as_the_blocks_goto_ref() {
        let redirect = |source: &str, name: &str| {
            let lines = parse_doclines(source);
            let decl = lines
                .iter()
                .position(|l| l.as_text().is_some_and(|t| t.starts_with("glyph")))
                .expect("a header");
            redirect_at_declaration(&lines, decl, name, &NamePartsMap::default())
        };

        assert_eq!(
            redirect("glyph w\nref a 0 0 goto\n", "w").as_deref(),
            Some("a")
        );
        assert_eq!(
            redirect("glyph w\nanchor + 0 0\nref a 0 0\nref b 0 0 goto\n", "w").as_deref(),
            Some("b"),
            "the first ref that is flagged, past the body lines before it"
        );
        assert_eq!(
            redirect("glyph w 2 2\n@@@@\n..@@\nref a 0 0 goto\n", "w").as_deref(),
            Some("a"),
            "the pixel grid is body too"
        );
        assert_eq!(
            redirect("glyph w\nref a 0 0 // goto\n", "w"),
            None,
            "a comment is not a flag"
        );
        assert_eq!(
            redirect("glyph w\nref a 0 0\n\nglyph v\nref b 0 0 goto\n", "w"),
            None,
            "the next block's ref belongs to the next block"
        );
        assert_eq!(
            redirect("glyph w\nref goto 0 0\n", "w"),
            None,
            "a target *named* `goto` is not a flagged ref"
        );
        assert_eq!(
            redirect("glyph w\nref a 0 0 goto\n", "other"),
            None,
            "the block has to be the one that declares the name"
        );
    }
}
