//! Navigation history — the reverse of Ctrl/Cmd+click "go to symbol".
//!
//! The stack behaves exactly like the undo stack: entries accumulate, going
//! back rewinds a position without discarding anything, and pushing a new entry
//! while rewound replaces everything after the current position.
//!
//! What is unlike an undo stack is that each entry stores **two** positions.
//! Following a link is asymmetric: the jump starts at the link and ends at its
//! target, so going back has to reach the link while going forward again has to
//! reach the target. One position per entry would make one of the two
//! directions land a step off.
//!
//! ```text
//!   entries:  [ A ] [ B ] [ C ]
//!                          ^ pos == 3
//!   back  -> pos 2, caret at C.from (the link that led into C's target)
//!   back  -> pos 1, caret at B.from
//!   fwd   -> caret at B.to (the target that link led to), pos 2
//! ```
//!
//! An entry's `from` is the **link's** own line and column, not the caret: a
//! Ctrl+click deliberately leaves the caret where it was, so a `from` taken from
//! the caret would point wherever the user last clicked
//! (`view_tests.rs::following_a_link_reports_the_link_position_not_the_caret`
//! pins this). A hit clicked in the Search pane is the opposite case and does
//! record the caret — the pane is not a position in a document, so the caret is
//! the only place the user can be said to have left.
//!
//! The editor reports each followed link exactly once, as
//! `DocumentViewResult::nav`: a `NavTarget::Local` it already carried out itself,
//! a `NavTarget::CrossFile` only the host can resolve, or a `NavTarget::Search`
//! (see [`super::search`]). Every case is therefore recorded through one path.
//!
//! An entry also remembers **where on the page** each of its two positions was
//! seen, so that going back restores the view the user left and not merely the
//! line: a jump is a "go to symbol", which centres its target, but a return is
//! a request for the page that was there. Only a position somebody was actually
//! looking at can carry one — a jump's target is recorded before it has ever
//! been drawn — so `view_offset` is an `Option`, and a `None` centres.
//!
//! Positions are plain `(document, line, column)` triples and are **not**
//! rewritten when the document is edited underneath them; see
//! [`NavHistory`]'s note. Documents are identified by their index into
//! `UniformApp::open_documents`, which is only ever appended to — no index it
//! hands out can later mean a different file. Opening another folder does clear
//! that list, and clears this history with it.

/// A remembered caret position in one open document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct NavLoc {
    /// Index into `UniformApp::open_documents`.
    pub doc_idx: usize,
    pub line: usize,
    pub col: usize,
    /// How far below the viewport's top `line` was drawn when the user left
    /// it, if that was seen. `None` means nobody watched the position being
    /// left — a jump's *target*, which was never on screen when it was
    /// recorded — and such a position is centred instead of restored.
    pub view_offset: Option<f32>,
}

impl NavLoc {
    pub(super) fn new(doc_idx: usize, line: usize, col: usize) -> Self {
        Self {
            doc_idx,
            line,
            col,
            view_offset: None,
        }
    }

    /// The same position, remembering the page it was left on.
    pub(super) fn seen_at(self, view_offset: f32) -> Self {
        Self {
            view_offset: Some(view_offset),
            ..self
        }
    }

    /// What a return to this position asks of the view.
    pub(super) fn scroll_intent(&self) -> crate::editor::ScrollIntent {
        use crate::editor::ScrollIntent;
        self.view_offset
            .map_or(ScrollIntent::Center, ScrollIntent::Offset)
    }
}

/// One followed link: where it was written, and where it led.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct NavEntry {
    /// The link itself — where "go back" returns to.
    pub from: NavLoc,
    /// The link's target — where "go forward" returns to.
    pub to: NavLoc,
}

/// The go-back/go-forward stack.
///
/// Note: an edit that inserts or deletes lines does *not* shift the positions
/// already recorded here, so a jump remembered before such an edit can come
/// back a few lines off. Navigation clamps to the document, so the position is
/// always valid, just possibly stale.
///
/// Doing better needs anchors that every mutation updates, and there is no one
/// place to update them from yet: `UndoStack::push_lines` is nearly the choke
/// point for line-count changes, but `editor::reconcile` and `app::rename`
/// bypass it.
#[derive(Default)]
pub(super) struct NavHistory {
    entries: Vec<NavEntry>,
    /// Number of entries not yet rewound; `entries[..pos]` are behind us.
    pos: usize,
}

impl NavHistory {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Records a jump, discarding any entries that were rewound past.
    pub(super) fn push(&mut self, entry: NavEntry) {
        self.entries.truncate(self.pos);
        self.entries.push(entry);
        self.pos = self.entries.len();
    }

    pub(super) fn can_go_back(&self) -> bool {
        self.pos > 0
    }

    pub(super) fn can_go_forward(&self) -> bool {
        self.pos < self.entries.len()
    }

    /// Rewinds one entry and reports the link position to return to.
    pub(super) fn go_back(&mut self) -> Option<NavLoc> {
        self.pos = self.pos.checked_sub(1)?;
        Some(self.entries[self.pos].from)
    }

    /// Replays the entry last rewound and reports its target position.
    pub(super) fn go_forward(&mut self) -> Option<NavLoc> {
        let to = self.entries.get(self.pos)?.to;
        self.pos += 1;
        Some(to)
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(from_line: usize, to_line: usize) -> NavEntry {
        NavEntry {
            from: NavLoc::new(0, from_line, 4),
            to: NavLoc::new(1, to_line, 0),
        }
    }

    #[test]
    fn empty_history_goes_nowhere() {
        let mut h = NavHistory::new();
        assert!(!h.can_go_back());
        assert!(!h.can_go_forward());
        assert_eq!(h.go_back(), None);
        assert_eq!(h.go_forward(), None);
    }

    #[test]
    fn back_reaches_the_link_and_forward_the_target() {
        let mut h = NavHistory::new();
        h.push(entry(10, 100));
        assert!(h.can_go_back());
        assert!(!h.can_go_forward());

        // Back lands on the link that was followed...
        assert_eq!(h.go_back(), Some(NavLoc::new(0, 10, 4)));
        assert!(!h.can_go_back());
        assert!(h.can_go_forward());

        // ...and forward on the target that link led to. The asymmetry is the
        // whole reason an entry stores two positions.
        assert_eq!(h.go_forward(), Some(NavLoc::new(1, 100, 0)));
        assert!(!h.can_go_forward());
    }

    #[test]
    fn walks_a_chain_of_jumps_in_both_directions() {
        let mut h = NavHistory::new();
        h.push(entry(10, 100));
        h.push(entry(20, 200));
        h.push(entry(30, 300));

        assert_eq!(h.go_back(), Some(NavLoc::new(0, 30, 4)));
        assert_eq!(h.go_back(), Some(NavLoc::new(0, 20, 4)));
        assert_eq!(h.go_back(), Some(NavLoc::new(0, 10, 4)));
        assert_eq!(h.go_back(), None);

        assert_eq!(h.go_forward(), Some(NavLoc::new(1, 100, 0)));
        assert_eq!(h.go_forward(), Some(NavLoc::new(1, 200, 0)));
        assert_eq!(h.go_forward(), Some(NavLoc::new(1, 300, 0)));
        assert_eq!(h.go_forward(), None);
    }

    #[test]
    fn a_new_jump_replaces_the_rewound_tail() {
        let mut h = NavHistory::new();
        h.push(entry(10, 100));
        h.push(entry(20, 200));
        h.push(entry(30, 300));
        h.go_back();
        h.go_back();
        assert!(h.can_go_forward());

        h.push(entry(40, 400));
        assert!(
            !h.can_go_forward(),
            "entries after the rewind point are gone"
        );
        assert_eq!(h.go_back(), Some(NavLoc::new(0, 40, 4)));
        assert_eq!(h.go_back(), Some(NavLoc::new(0, 10, 4)));
        assert_eq!(h.go_back(), None);
    }

    #[test]
    fn clear_drops_everything() {
        let mut h = NavHistory::new();
        h.push(entry(10, 100));
        h.go_back();
        h.clear();
        assert!(!h.can_go_back());
        assert!(!h.can_go_forward());
    }
}

/// The two entries one jump through a `goto` ref leaves behind.
///
/// Driven through the whole app rather than through [`NavHistory`] alone: the
/// redirect is read off the *resolved* glyph (`goto_redirect`), so the question
/// this asks — does a jump to a wrapper end up at the drawing, with the wrapper
/// one Go Back away — cannot be asked of anything smaller than a running
/// pipeline.
#[cfg(test)]
mod goto_tests {
    use crate::app::UniformApp;
    use crate::app::background::startup_tests::TempDir;
    use crate::app::settings::Settings;
    use crate::editor::doc_links::LinkTargetKind;
    use crate::editor::document_view::{GotoGlyph, NavRequest, NavTarget};

    /// Runs the background pipeline until the resolve behind `named_glyphs` has
    /// landed, which is what a redirect is read out of.
    fn settle(app: &mut UniformApp, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            app.pump_background_pipeline(ctx);
            if app.named_glyphs_gen == app.font_build_gen && app.font_build_gen != u64::MAX {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the pipeline never delivered a resolve"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_jump_through_a_goto_ref_lands_on_the_target_and_records_both_steps() {
        let dir = TempDir::new("goto-nav");
        // The wrapper and the drawing are in different files, which is the
        // shape the flag exists for: `han.unf`'s one pattern block over the
        // per-block `han-XXXX.unf` files.
        std::fs::write(
            dir.0.join("a.unf"),
            "meta height 4\nmeta ascent 3\nmeta descent 1\n\n\
             glyph wrapper\nref drawing 0 0 goto\n\nmap A = wrapper\n",
        )
        .unwrap();
        std::fs::write(dir.0.join("b.unf"), "glyph drawing 2 2\n@@\n.@\n").unwrap();

        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        settle(&mut app, &ctx);

        // A link written somewhere else in `a.unf`, followed.
        app.open_file(dir.0.join("a.unf"));
        let from_doc = app
            .open_documents
            .iter()
            .position(|d| d.document.path.ends_with("a.unf"))
            .unwrap();
        app.follow_nav_request(
            &ctx,
            from_doc,
            NavRequest {
                from: crate::editor::caret::Caret::new(6, 8),
                from_offset: 0.0,
                target: NavTarget::CrossFile(GotoGlyph {
                    name: "wrapper".into(),
                    kind: LinkTargetKind::Glyph,
                }),
            },
        );

        let landed = app.panes.active_doc_idx().unwrap();
        let landed_path = app.open_documents[landed].document.path.clone();
        assert!(
            landed_path.ends_with("b.unf"),
            "the jump goes through the wrapper to the drawing, not to the wrapper's own line"
        );
        assert_eq!(
            app.open_documents[landed].editor_state.cursor_line(),
            0,
            "and lands on `glyph drawing`"
        );

        // Back once for the wrapper the jump passed through...
        app.navigate_history(&ctx, false);
        let at = app.panes.active_doc_idx().unwrap();
        assert!(app.open_documents[at].document.path.ends_with("a.unf"));
        assert_eq!(
            app.open_documents[at].editor_state.cursor_line(),
            4,
            "`glyph wrapper` is the first Go Back"
        );

        // ...and once more for the link itself.
        app.navigate_history(&ctx, false);
        let at = app.panes.active_doc_idx().unwrap();
        assert_eq!(app.open_documents[at].editor_state.cursor_line(), 6);
        assert!(!app.nav_history.can_go_back(), "two entries, both walked");
    }

    /// A specimen click starts nowhere, so the drawing it lands on is one Go
    /// Back from the wrapper and no further — the click itself is not a
    /// position anything can return to.
    #[test]
    fn a_specimen_click_records_only_what_the_redirect_passed_through() {
        let dir = TempDir::new("goto-specimen");
        std::fs::write(
            dir.0.join("a.unf"),
            "meta height 4\nmeta ascent 3\nmeta descent 1\n\n\
             glyph wrapper\nref drawing 0 0 goto\n\nmap A = wrapper\n",
        )
        .unwrap();
        std::fs::write(dir.0.join("b.unf"), "glyph drawing 2 2\n@@\n.@\n").unwrap();

        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        settle(&mut app, &ctx);

        app.follow_specimen_click(
            &ctx,
            crate::specimen::SpecimenClick {
                name: "wrapper".into(),
                kind: LinkTargetKind::Glyph,
            },
        );

        let at = app.panes.active_doc_idx().unwrap();
        assert!(app.open_documents[at].document.path.ends_with("b.unf"));

        app.navigate_history(&ctx, false);
        let at = app.panes.active_doc_idx().unwrap();
        assert!(app.open_documents[at].document.path.ends_with("a.unf"));
        assert_eq!(app.open_documents[at].editor_state.cursor_line(), 4);
        assert!(
            !app.nav_history.can_go_back(),
            "the click itself is not a place to go back to"
        );
    }

    /// A wrapper whose target is itself a wrapper is followed again, and each
    /// step is its own Go Back.
    ///
    /// (A `goto` *cycle* cannot arise: `goto` rides on a `ref`, and a ref cycle
    /// never resolves, so `goto_redirect` — which reads the resolve — has
    /// nothing to hand back. `follow_goto_chain`'s visited set is a guard
    /// against that stopping being true, not a case anyone can write today.)
    #[test]
    fn a_chain_of_goto_refs_is_followed_to_its_end() {
        let dir = TempDir::new("goto-chain");
        std::fs::write(
            dir.0.join("a.unf"),
            "meta height 4\nmeta ascent 3\nmeta descent 1\n\n\
             glyph one\nref two 0 0 goto\n\nmap A = one\n",
        )
        .unwrap();
        std::fs::write(dir.0.join("b.unf"), "glyph two\nref three 0 0 goto\n").unwrap();
        std::fs::write(dir.0.join("c.unf"), "glyph three 2 2\n@@\n.@\n").unwrap();

        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        settle(&mut app, &ctx);

        app.follow_specimen_click(
            &ctx,
            crate::specimen::SpecimenClick {
                name: "one".into(),
                kind: LinkTargetKind::Glyph,
            },
        );

        let at = app.panes.active_doc_idx().unwrap();
        assert!(
            app.open_documents[at].document.path.ends_with("c.unf"),
            "both hops are followed"
        );

        for expected in ["b.unf", "a.unf"] {
            app.navigate_history(&ctx, false);
            let at = app.panes.active_doc_idx().unwrap();
            assert!(
                app.open_documents[at].document.path.ends_with(expected),
                "expected {expected}"
            );
        }
        assert!(!app.nav_history.can_go_back());
    }
}
