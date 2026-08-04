//! Transient notices shown over the editor area.
//!
//! The status bar already carries one line of feedback, but it is overwritten
//! by the next thing that happens and it is at the far edge of the window —
//! neither of which suits "the file you are editing changed underneath you",
//! which the user has to see even if it lands during a burst of other
//! messages. A toast stacks instead of replacing, sits over the editor, and
//! goes away on its own ([`TOAST_TTL`]) or on a click.
//!
//! Toasts are drawn last in the frame, so they are over the panes rather than
//! under them, and the area they are anchored to is
//! `Context::available_rect` — what is left after the panels, which is exactly
//! the editor area.
//!
//! # Sticky toasts
//!
//! A notice about something that *happened* can expire, because the thing it
//! reports is already done. A notice about something the editor is **holding
//! back** cannot: it stands for a state that is still true, and a timeout would
//! leave the user with a postponed change and no sign of it. So a sticky toast
//! ([`Toasts::set_sticky`]) has no TTL at all — its owner puts it up and takes
//! it down, keyed by a `&'static str`, and a click on it comes back out of
//! [`Toasts::show`] as that key rather than merely dismissing it. The only one
//! today is the watch's "a change is waiting for the pointer to leave"; see
//! [`super::watch`].

use std::time::{Duration, Instant};

/// How long a toast stays up. Long enough to be read after the click that
/// caused it, short enough not to pile up while a `git checkout` runs.
pub(super) const TOAST_TTL: Duration = Duration::from_secs(10);

/// How long it takes to fade out at the end of its life.
const FADE: Duration = Duration::from_millis(600);

const WIDTH: f32 = 340.0;

struct Toast {
    text: String,
    born: Instant,
    /// Set on a sticky toast: it never expires, and a click on it reports this
    /// key to the caller instead of dismissing it.
    key: Option<&'static str>,
}

pub(super) struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    pub(super) fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Adds a notice. An identical one still on screen is refreshed rather
    /// than duplicated: a file written twice in a row is one thing to say.
    pub(super) fn push(&mut self, text: impl Into<String>) {
        let text = text.into();
        if let Some(existing) = self.items.iter_mut().find(|t| t.text == text) {
            existing.born = Instant::now();
            return;
        }
        self.items.push(Toast {
            text,
            born: Instant::now(),
            key: None,
        });
    }

    /// Puts up (or takes down, with `None`) the sticky notice named `key`.
    ///
    /// Called every frame by whoever owns the state it reports, so the notice
    /// is on screen exactly while that state holds. Re-stating the same text
    /// leaves the toast alone rather than restarting it — a sticky toast has no
    /// lifetime to restart, and rebuilding it would flicker.
    pub(super) fn set_sticky(&mut self, key: &'static str, text: Option<String>) {
        let existing = self.items.iter_mut().find(|t| t.key == Some(key));
        match (existing, text) {
            (Some(toast), Some(text)) => toast.text = text,
            (Some(_), None) => self.items.retain(|t| t.key != Some(key)),
            (None, Some(text)) => self.items.push(Toast {
                text,
                born: Instant::now(),
                key: Some(key),
            }),
            (None, None) => {}
        }
    }

    #[cfg(test)]
    pub(super) fn texts(&self) -> Vec<&str> {
        self.items.iter().map(|t| t.text.as_str()).collect()
    }

    /// Drops the notices whose time is up. A sticky one has none.
    fn expire(&mut self) {
        self.items
            .retain(|t| t.key.is_some() || t.born.elapsed() < TOAST_TTL);
    }

    /// Draws the notices and reports the key of a sticky one that was clicked;
    /// a click on an ordinary notice only dismisses it.
    pub(super) fn show(&mut self, ctx: &egui::Context) -> Option<&'static str> {
        self.expire();
        if self.items.is_empty() {
            return None;
        }
        // Only for the ones that age: a sticky toast has nothing to animate,
        // and asking for a frame every 100ms forever would spin the CPU for as
        // long as it is up. What it waits on (the pointer leaving) already
        // keeps frames coming; see [`super::watch`].
        if self.items.iter().any(|t| t.key.is_none()) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let area = ctx.available_rect();
        let mut dismissed = None;
        egui::Area::new(egui::Id::new("uniform_toasts"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(area.right() - WIDTH - 12.0, area.top() + 12.0))
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_max_width(WIDTH);
                for (idx, toast) in self.items.iter().enumerate() {
                    let remaining = TOAST_TTL.saturating_sub(toast.born.elapsed());
                    let alpha = if toast.key.is_none() && remaining < FADE {
                        remaining.as_secs_f32() / FADE.as_secs_f32()
                    } else {
                        1.0
                    };
                    let visuals = ui.visuals();
                    let frame = egui::Frame::popup(ui.style())
                        .fill(visuals.window_fill.gamma_multiply(alpha))
                        .stroke(egui::Stroke::new(
                            1.0,
                            visuals.warn_fg_color.gamma_multiply(alpha),
                        ));
                    let color = visuals.text_color().gamma_multiply(alpha);
                    let resp = frame
                        .show(ui, |ui| {
                            ui.set_width(WIDTH - 24.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    egui::RichText::new("\u{26A0}")
                                        .color(ui.visuals().warn_fg_color.gamma_multiply(alpha)),
                                );
                                ui.label(egui::RichText::new(&toast.text).color(color));
                            });
                        })
                        .response;
                    let click = ui.interact(
                        resp.rect,
                        ui.id().with(("toast", idx)),
                        egui::Sense::click(),
                    );
                    if click.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if click.clicked() {
                        dismissed = Some(idx);
                    }
                }
            });
        let idx = dismissed?;
        match self.items[idx].key {
            // Its owner takes it down once the state it reports is gone —
            // removing it here would say the work was done before it was.
            Some(key) => Some(key),
            None => {
                self.items.remove(idx);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two writes to the same file must not stack two identical notices.
    #[test]
    fn an_identical_notice_is_refreshed_not_repeated() {
        let mut toasts = Toasts::new();
        toasts.push("num.unf changed on disk");
        toasts.push("num.unf changed on disk");
        toasts.push("latin.unf changed on disk");
        assert_eq!(
            toasts.texts(),
            ["num.unf changed on disk", "latin.unf changed on disk"],
        );
    }

    /// A sticky notice stands for a state that is still true, so no timeout may
    /// take it away — only its owner, by stating that the state is gone.
    #[test]
    fn a_sticky_notice_outlives_the_timeout_and_only_its_owner_removes_it() {
        let mut toasts = Toasts::new();
        toasts.push("transient");
        toasts.set_sticky("held", Some("a change is waiting".into()));
        for item in &mut toasts.items {
            item.born = Instant::now() - TOAST_TTL - Duration::from_secs(1);
        }

        toasts.expire();
        assert_eq!(toasts.texts(), ["a change is waiting"]);

        // Restating it updates the text in place, so the notice can follow the
        // state it reports without flickering.
        toasts.set_sticky("held", Some("two changes are waiting".into()));
        assert_eq!(toasts.texts(), ["two changes are waiting"]);

        toasts.set_sticky("held", None);
        assert!(toasts.texts().is_empty());
    }
}
