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
        });
    }

    #[cfg(test)]
    pub(super) fn texts(&self) -> Vec<&str> {
        self.items.iter().map(|t| t.text.as_str()).collect()
    }

    pub(super) fn show(&mut self, ctx: &egui::Context) {
        self.items.retain(|t| t.born.elapsed() < TOAST_TTL);
        if self.items.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(100));

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
                    let alpha = if remaining < FADE {
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
        if let Some(idx) = dismissed {
            self.items.remove(idx);
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
}
