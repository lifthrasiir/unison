//! Control-click as a secondary click on macOS.
//!
//! The Mac's secondary-click emulation is AppKit's: a native control reads a
//! Control-click as a right click, but winit reports it as what it is, a left
//! button with Control held, and egui does no emulation of its own. So every
//! surface that is egui's rather than AppKit's — the pixel grid, the context
//! menus — saw a left click. The translation is done once, on the raw input
//! before egui sees it, rather than in each widget that reads the secondary
//! button: `secondary_down` and `secondary_clicked` then just work, drags
//! included.
//!
//! Only the press decides. Its release is rewritten as well whether or not
//! Control is still held by then, so letting go of Control first does not
//! leave egui with a secondary button that never comes up (or a primary one
//! that never went down). The modifiers are left alone, Control included, as
//! AppKit leaves them on the emulated event; nothing reads Control on a click.

/// Tracks whether the primary button currently down was turned into the
/// secondary one, so its release is turned too.
#[derive(Default)]
pub(super) struct CtrlClick {
    translating: bool,
}

impl CtrlClick {
    /// Rewrite the primary-button events of `raw` in place. `enabled` is
    /// whether the platform has the convention at all.
    pub(super) fn translate(&mut self, raw: &mut egui::RawInput, enabled: bool) {
        if !enabled {
            return;
        }
        for event in &mut raw.events {
            match event {
                egui::Event::PointerButton {
                    button: button @ egui::PointerButton::Primary,
                    pressed,
                    modifiers,
                    ..
                } => {
                    if *pressed {
                        self.translating = modifiers.ctrl;
                    }
                    if self.translating {
                        *button = egui::PointerButton::Secondary;
                    }
                    if !*pressed {
                        self.translating = false;
                    }
                }
                // egui releases every button itself when the pointer leaves.
                egui::Event::PointerGone => self.translating = false,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: egui::Modifiers = egui::Modifiers {
        ctrl: true,
        ..egui::Modifiers::NONE
    };

    fn button(
        button: egui::PointerButton,
        pressed: bool,
        modifiers: egui::Modifiers,
    ) -> egui::Event {
        egui::Event::PointerButton {
            pos: egui::pos2(10.0, 10.0),
            button,
            pressed,
            modifiers,
        }
    }

    fn run(tr: &mut CtrlClick, events: Vec<egui::Event>, enabled: bool) -> Vec<egui::Event> {
        let mut raw = egui::RawInput {
            events,
            ..Default::default()
        };
        tr.translate(&mut raw, enabled);
        raw.events
    }

    use egui::PointerButton::{Primary, Secondary};

    #[test]
    fn a_control_click_is_a_secondary_click() {
        let mut tr = CtrlClick::default();
        let out = run(
            &mut tr,
            vec![button(Primary, true, CTRL), button(Primary, false, CTRL)],
            true,
        );
        assert_eq!(
            out,
            vec![
                button(Secondary, true, CTRL),
                button(Secondary, false, CTRL)
            ]
        );
    }

    #[test]
    fn the_release_follows_the_press_across_frames_and_modifier_changes() {
        let mut tr = CtrlClick::default();
        let pressed = run(&mut tr, vec![button(Primary, true, CTRL)], true);
        assert_eq!(pressed, vec![button(Secondary, true, CTRL)]);
        // Control let go mid-drag: still the secondary button coming up.
        let moved = run(
            &mut tr,
            vec![egui::Event::PointerMoved(egui::pos2(20.0, 10.0))],
            true,
        );
        assert_eq!(
            moved,
            vec![egui::Event::PointerMoved(egui::pos2(20.0, 10.0))]
        );
        let released = run(
            &mut tr,
            vec![button(Primary, false, egui::Modifiers::NONE)],
            true,
        );
        assert_eq!(
            released,
            vec![button(Secondary, false, egui::Modifiers::NONE)]
        );
        // And a plain click after it is a plain click again.
        let plain = vec![
            button(Primary, true, egui::Modifiers::NONE),
            button(Primary, false, CTRL),
        ];
        assert_eq!(run(&mut tr, plain.clone(), true), plain);
    }

    #[test]
    fn a_plain_or_command_click_and_a_real_right_click_are_untouched() {
        let mut tr = CtrlClick::default();
        let events = vec![
            button(Primary, true, egui::Modifiers::NONE),
            button(Primary, false, egui::Modifiers::NONE),
            button(Primary, true, egui::Modifiers::MAC_CMD),
            button(Primary, false, egui::Modifiers::MAC_CMD),
            button(Secondary, true, CTRL),
            button(Secondary, false, CTRL),
        ];
        assert_eq!(run(&mut tr, events.clone(), true), events);
    }

    #[test]
    fn pointer_gone_ends_the_translation() {
        let mut tr = CtrlClick::default();
        run(
            &mut tr,
            vec![button(Primary, true, CTRL), egui::Event::PointerGone],
            true,
        );
        let later = vec![button(Primary, false, egui::Modifiers::NONE)];
        assert_eq!(run(&mut tr, later.clone(), true), later);
    }

    #[test]
    fn off_the_mac_control_click_stays_a_control_click() {
        let mut tr = CtrlClick::default();
        let events = vec![button(Primary, true, CTRL), button(Primary, false, CTRL)];
        assert_eq!(run(&mut tr, events.clone(), false), events);
    }
}
