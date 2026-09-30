//! Undeclared and out-of-order `color` uses.

use super::*;

/// A `fill` naming a color that no `color` line declares silently fell
/// back to `fg` in the build; it has to be reported here instead.
#[test]
fn a_fill_naming_an_undeclared_color_is_a_warning() {
    let issues = issues_for("glyph a 1 1\n@@\n\nglyph b\nref a fill missing\n\nmap A = b\n");
    assert!(
        has(&issues, Severity::Warning, "undeclared color `missing`"),
        "{issues:?}"
    );
}

/// `color` aliases resolve in document order (see
/// `render::ttf_builder::color::collect_color_aliases`), so a value naming
/// a color declared later never resolves — silently, before this check.
#[test]
fn a_color_alias_used_before_its_declaration_is_a_warning() {
    let issues = issues_for("color x = y\ncolor y = #ff0000\n");
    assert!(has(&issues, Severity::Warning, "color `x`"), "{issues:?}");
}

#[test]
fn declared_color_uses_are_quiet() {
    let issues = issues_for(
        "color red = #ff0000\ncolor also-red = red\n\nglyph a 1 1\n@@\n\n\
             glyph b\nref a fill also-red\n\nmap A = b\n",
    );
    assert!(
        !issues.iter().any(|i| i.message.contains("color")),
        "{issues:?}"
    );
}
