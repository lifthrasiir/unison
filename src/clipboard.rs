//! What answers a copy keystroke, and the log sink that says when a copy was
//! refused.
//!
//! Several surfaces answer one [`egui::Event::Copy`] in the same frame — each
//! pane's editor, the pixel grid, the specimen's hovered cell — and each one
//! pushes its own `OutputCommand::CopyText`. `egui-winit` applies them in
//! order, so the copy that *wins* is whichever ran last in the frame, not the
//! one the keystroke was meant for. `egui` is one of those surfaces itself:
//! see [`release_label_selection`].
//!
//! A copy that never reaches the clipboard looks identical from the outside,
//! because `egui-winit` only `log::error!`s what `arboard` refused. That is
//! what [`install_log_sink`] is for.

/// Drops the text selection a selectable `Label` is holding, when the copy
/// keystroke in this frame belongs to something else.
///
/// `egui` answers a copy keystroke on behalf of whatever label holds a
/// selection, and it does so in `Context::end_pass` — after every panel, so
/// after the focused surface has already answered, and `egui-winit` applies
/// that last command. A drag a reader did not mean to make (a click on a
/// search hit that moved a pixel) therefore takes over every Ctrl/Cmd+C for as
/// long as the selection lives, which is until Escape or a click on nothing:
/// the clipboard holds that one line and nothing else, however much is copied
/// elsewhere. `egui` keeps the selection across frames on purpose — the reset
/// in its `begin_pass` is commented out — so it does not end on its own.
///
/// The keystroke belongs to whatever holds the keyboard. Called at the top of
/// the frame — before any label is drawn — with `focused` set when an editor
/// or the preview has it, this leaves the label with nothing to copy.
pub(crate) fn release_label_selection(ctx: &egui::Context, focused: bool) {
    if !focused {
        return;
    }
    // Only on the keystroke itself: a selection a reader made on purpose stays
    // on screen, and stays copyable, right up until they ask for a copy while
    // typing somewhere else.
    let asked = ctx.input(|i| {
        i.events
            .iter()
            .any(|e| matches!(e, egui::Event::Copy | egui::Event::Cut))
    });
    if !asked {
        return;
    }
    let mut state = egui::text_selection::LabelSelectionState::load(ctx);
    if state.has_selection() {
        state.clear_selection();
        state.store(ctx);
    }
}

/// Sends the `log` records of the crates the editor rides on to stderr.
///
/// It exists for two of them: `arboard` failing to initialize (warn) and
/// failing to copy (error), both raised by `egui-winit` and both otherwise
/// invisible — this binary has no other logger, and a clipboard that silently
/// stops taking copies is a long afternoon.
///
/// **Warnings and errors only.** `eframe` traces every windowing event it
/// handles, and a sink that accepts those writes to stderr faster than a
/// console can take it: the main thread ends up blocked on its own log.
pub(crate) fn install_log_sink() {
    struct Stderr;
    impl log::Log for Stderr {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Warn
        }
        fn log(&self, record: &log::Record<'_>) {
            if !self.enabled(record.metadata()) {
                return;
            }
            eprintln!(
                "[{}] {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
        fn flush(&self) {}
    }
    if log::set_logger(&Stderr).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pass over a central panel holding one selectable label, with `events`
    /// fed to it. Returns the label's rect and the pass's output commands.
    fn pass(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        focused: bool,
        body: impl FnOnce(&egui::Context),
    ) -> (egui::Rect, Vec<egui::OutputCommand>) {
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 200.0),
            )),
            events,
            ..Default::default()
        });
        release_label_selection(ctx, focused);
        let mut rect = egui::Rect::NOTHING;
        egui::CentralPanel::default().show(ctx, |ui| {
            rect = ui.label("the label a stray drag selected").rect;
        });
        body(ctx);
        (rect, ctx.end_pass().platform_output.commands)
    }

    /// A drag over a label leaves a selection that `egui` copies from
    /// `Context::end_pass` — after every panel, so after the editor has
    /// answered the same keystroke, and it is the one the clipboard keeps.
    /// While an editor holds the keyboard, the keystroke is the editor's.
    #[test]
    fn a_label_selection_does_not_outrank_the_focused_editors_copy() {
        let ctx = egui::Context::default();
        let (rect, _) = pass(&ctx, vec![], false, |_| {});
        let left = egui::pos2(rect.left() + 1.0, rect.center().y);
        let right = egui::pos2(rect.right() - 1.0, rect.center().y);
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        pass(&ctx, vec![egui::Event::PointerMoved(left)], false, |_| {});
        pass(&ctx, vec![press(left, true)], false, |_| {});
        pass(&ctx, vec![egui::Event::PointerMoved(right)], false, |_| {});
        pass(&ctx, vec![press(right, false)], false, |_| {});
        assert!(
            egui::text_selection::LabelSelectionState::load(&ctx).has_selection(),
            "the drag did not select the label, so the case is not being tested",
        );

        let (_, commands) = pass(&ctx, vec![egui::Event::Copy], true, |ctx| {
            ctx.copy_text("glyph han-0001 8 16".to_string());
        });
        let copied: Vec<&str> = commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(copied, ["glyph han-0001 8 16"]);
    }

    /// The selection a reader made on purpose is theirs until they copy from
    /// somewhere else: nothing clears it just because an editor has the focus.
    #[test]
    fn a_label_selection_survives_a_frame_with_no_copy_in_it() {
        let ctx = egui::Context::default();
        let (rect, _) = pass(&ctx, vec![], false, |_| {});
        let press = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let left = egui::pos2(rect.left() + 1.0, rect.center().y);
        let right = egui::pos2(rect.right() - 1.0, rect.center().y);
        pass(&ctx, vec![egui::Event::PointerMoved(left)], false, |_| {});
        pass(&ctx, vec![press(left, true)], false, |_| {});
        pass(&ctx, vec![egui::Event::PointerMoved(right)], false, |_| {});
        pass(&ctx, vec![press(right, false)], false, |_| {});

        pass(&ctx, vec![], true, |_| {});
        assert!(egui::text_selection::LabelSelectionState::load(&ctx).has_selection());
    }
}
