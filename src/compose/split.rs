//! Expanding a one-dimensional IDC line (`expand_compose`) into the `ref`s it stands for.

use super::clearance::check_clearances;
use super::enclosure::expand_enclosure;
use super::fit::{ClearanceRule, ComposeExpansion, FamilyLookup, fits_slot, misfit_variants};
use super::nested::{nested_key, nested_line};
use super::op::Raster;
use super::variant::{PartDims, VariantSpec, is_undecided, padding_across};
use crate::document::{ComposeItem, GlyphCompose, GlyphRef, Margin};
use crate::issues::Severity;

/// Turn one IDC line into the `ref`s it stands for, plus what is wrong with it.
///
/// Best effort: a component whose size is unknown is placed where the walk has
/// got to and advances it by nothing, so the parts that *are* known still land
/// where they belong and the editor can draw the glyph while it is being filled
/// in. The diagnostics are what stop a wrong glyph from passing
/// for a right one.
///
/// `parent` is the enclosing `glyph` header's box, `dims` answers for a
/// component name, and `family` — where the caller has one — answers what the
/// *family* of an undecided component draws, which is what separates a line
/// waiting for a decision from one whose decision cannot be made. Messages come
/// back with a severity and no location — the caller owns the
/// [`crate::resolve::ItemRef`].
///
/// Every length here is in *declared* units, the ones the `glyph` header
/// writes: the layout is the same at any `scale`, and a component drawn at a
/// different scale than its parent still fills the same box. Only the derived
/// offsets leave in the parent's raster units ([`Raster`]), because that is
/// what a `ref` offset means.
pub fn expand_compose(
    glyph_name: &str,
    parent: Option<(u16, u16)>,
    raster: Raster,
    compose: &GlyphCompose,
    dims: &dyn Fn(&str) -> PartDims,
    family: Option<&FamilyLookup>,
    clearance: Option<&ClearanceRule>,
) -> ComposeExpansion {
    let scale = raster.scale.max(1) as i32;
    let frame = Frame {
        context: format!("glyph '{glyph_name}'"),
        nested: false,
    };
    let (mut refs, mut issues) =
        expand_line(&frame, parent, scale, compose, dims, family, clearance);
    // The layout is worked out in the box; a `ref` offset is counted from the
    // grid's corner, which is the box's only when no `origin` moves it.
    let (col, row) = raster.origin;
    if (col, row) != (0, 0) {
        for r in &mut refs {
            let (c, w) = r.offset.unwrap_or((0, 0));
            r.offset = Some((
                clamp_offset(c as i32 + col as i32 * scale),
                clamp_offset(w as i32 + row as i32 * scale),
            ));
        }
    }
    // What `assume` takes on trust is the clearances and nothing else. See the
    // module docs.
    if compose.assumed {
        issues.retain(|(severity, _)| *severity != Severity::Chore);
    }
    (refs, issues)
}

/// Where a line being laid out sits, for its messages: the glyph's own line,
/// or the line a [nested split](crate::compose#nested-splits) stands for inside it.
pub(super) struct Frame {
    /// What a message is prefixed with, up to the operator.
    pub(super) context: String,
    /// Whether the line is a nested split's, whose box is inferred rather than
    /// declared.
    pub(super) nested: bool,
}

impl Frame {
    /// What the line's box belongs to, as a message names it.
    pub(super) fn whole(&self) -> &'static str {
        match self.nested {
            true => "nested split",
            false => "glyph",
        }
    }
}

/// [`expand_compose`] in the parent's box, before an `origin` moves what it
/// places and an `assume` what it reports; `scale` is at least 1.
pub(super) fn expand_line(
    frame: &Frame,
    parent: Option<(u16, u16)>,
    scale: i32,
    compose: &GlyphCompose,
    dims: &dyn Fn(&str) -> PartDims,
    family: Option<&FamilyLookup>,
    clearance: Option<&ClearanceRule>,
) -> ComposeExpansion {
    let op = compose.op;
    let mut issues: Vec<(Severity, String)> = Vec::new();
    let mut refs: Vec<GlyphRef> = Vec::new();
    let here = format!("{}: `{}`", frame.context, op.as_char());
    let at = |msg: String| format!("{here} {msg}");
    let whole = frame.whole();

    let parts = compose.slot_count();
    // A nested split may also be one part: gaps on either side of a member
    // shorter than the slot, which is how a part is padded across the axis
    // without a glyph of its own to do it.
    match frame.nested {
        false if parts != op.arity() => issues.push((
            Severity::Error,
            at(format!("takes {} components, not {parts}", op.arity())),
        )),
        true if !(1..=3).contains(&parts) => issues.push((
            Severity::Error,
            at(format!("takes 1 to 3 components, not {parts}")),
        )),
        _ => {}
    }
    // A lone member is at both ends of its axis at once, so it claims neither.
    let slot_direction = |slot| match parts {
        1 if frame.nested => None,
        _ => op.slot_direction(slot),
    };

    let Some((parent_w, parent_h)) = parent else {
        issues.push((
            Severity::Error,
            at(
                "needs the enclosing `glyph` header to declare its `W H`: the parts are \
                placed by filling that box"
                    .to_string(),
            ),
        ));
        return (refs, issues);
    };

    // An enclosure lays out on both axes at once and has no cursor to walk, so
    // it is its own pass from here. Everything above it — the arity, the
    // parent's box — is what every IDC line answers for alike.
    if let Some(walls) = op.walls() {
        let (encl_refs, encl_issues) = expand_enclosure(
            walls,
            (parent_w, parent_h),
            scale,
            compose,
            dims,
            family,
            clearance,
        );
        refs.extend(encl_refs);
        issues.extend(encl_issues.into_iter().map(|(s, m)| (s, at(m))));
        return (refs, issues);
    }

    let (axis_extent, cross_extent) = if op.horizontal() {
        (parent_w, parent_h)
    } else {
        (parent_h, parent_w)
    };

    // A component that has not picked its variant yet leaves the line
    // *unresolved* rather than wrong: the width the slot was going to be
    // filled with is simply not chosen. Everything a measurement of such a
    // line would say is a consequence of that one gap, so the clearance check
    // and the unpicked component's own checks stand down and the line reports
    // one TODO per unpicked component instead. See [`Severity::Todo`].
    let unresolved = compose.part_names().any(is_undecided);

    let mut cursor: i32 = 0;
    let mut slot = 0usize;
    // Where each component landed along the axis and across it, for the
    // clearance check below. Collected on the way through because that walk is
    // what knows it.
    let mut placed_parts: Vec<(String, i32, i32)> = Vec::new();
    // Each nested split's written token and its `nested_key`.
    let mut nested_keys: Vec<(String, String)> = Vec::new();
    for item in &compose.items {
        let (name, raw_name) = match item {
            ComposeItem::Gap(gap) => {
                cursor += *gap as i32;
                continue;
            }
            ComposeItem::Part { name, raw_name } => (name, raw_name),
            ComposeItem::Nested(members) => {
                let (line, size) = nested_line(op, members, dims);
                let inner = Frame {
                    context: format!(
                        "{here} nested split '{}'",
                        ComposeItem::nested_token(members, false)
                    ),
                    nested: true,
                };
                // Laid out in a box of its own and moved into the slot, as the
                // glyph it stands for would be. Its own clearances are not
                // measured: see the module docs.
                let (nested_refs, nested_issues) = expand_line(
                    &inner,
                    Some(size.box_),
                    scale,
                    &line,
                    dims,
                    // What a member could be is a question about the nested split's
                    // box, which is only inferred from the members themselves.
                    None,
                    None,
                );
                issues.extend(nested_issues);
                let placed = cursor * scale;
                refs.extend(nested_refs.into_iter().map(|mut r| {
                    let (c, w) = r.offset.unwrap_or((0, 0));
                    let (dc, dr) = if op.horizontal() {
                        (placed, 0)
                    } else {
                        (0, placed)
                    };
                    r.offset = Some((clamp_offset(c as i32 + dc), clamp_offset(w as i32 + dr)));
                    r
                }));
                // Named as written in what the line says about it, and looked
                // up under the key its ink is kept by.
                let written = ComposeItem::nested_token(members, false);
                nested_keys.push((written.clone(), nested_key(op, members)));
                placed_parts.push((written, cursor, 0));
                slot += 1;
                let (w, h) = size.box_;
                let (along, across) = if op.horizontal() { (w, h) } else { (h, w) };
                if size.complete && across != cross_extent {
                    issues.push((
                        Severity::Error,
                        at(format!(
                            "nested split '{}' adds up to {across} {}, not the {whole}'s {cross_extent}",
                            ComposeItem::nested_token(members, false),
                            if op.horizontal() { "tall" } else { "wide" },
                        )),
                    ));
                }
                cursor += along as i32;
                continue;
            }
        };
        // The size and the position a name states are claims the *author*
        // made, so they are read off the name as written: a component named
        // through an alias has had `name` pointed at the drawing, and the
        // drawing's own name says which slot it was made for, not which slot
        // this line picked it for. Everything else here — what the component
        // resolves to, and whether it resolves at all — is the resolved name's
        // to answer, including whether a variant has been picked, since an
        // alias may name one without saying so.
        let spec = VariantSpec::parse(raw_name.as_deref().unwrap_or(name));
        let unpicked = is_undecided(name);
        if unpicked {
            // A nested split's box is inferred from its members, so it says nothing
            // about what size one of them should be.
            let slot_size = match (frame.nested, op.horizontal()) {
                (true, _) => "WxH".to_string(),
                (false, true) => format!("{axis_extent}x{cross_extent}"),
                (false, false) => format!("{cross_extent}x{axis_extent}"),
            };
            // A family that draws nothing this slot could hold is not a
            // decision waiting to be made: whatever is picked, the line cannot
            // be laid out, and the drawing that would let it be does not exist
            // yet. Saying TODO there loses it — a TODO flags no glyph and is
            // hidden by default — so it is a warning, and it names what the
            // family does draw, since that is the thing to be looked at.
            let fits =
                |size, margin| fits_slot(size, margin, axis_extent, cross_extent, op.horizontal());
            match misfit_variants(name, family, &fits) {
                Some(sizes) => issues.push((
                    Severity::Warning,
                    at(format!(
                        "component '{name}' has no variant that fits a {axis_extent}-{} slot; \
                         its family draws {sizes}",
                        if op.horizontal() { "wide" } else { "tall" },
                    )),
                )),
                None => issues.push((
                    Severity::Todo,
                    at(format!(
                        "component '{name}' has no variant picked yet; a component names the \
                         sized variant it wants, as in `{name}:{slot_size}`"
                    )),
                )),
            }
        }
        if let Some(slot_dir) = slot_direction(slot)
            && let Some(dir) = spec.direction
            && dir != slot_dir
        {
            issues.push((
                Severity::Warning,
                at(format!(
                    "component '{name}' is drawn for `-{}` but sits in the `-{}` slot",
                    dir.as_str(),
                    slot_dir.as_str(),
                )),
            ));
        }
        let part_dims = dims(name);
        // A nested split is the written-out form of a padding, and its members
        // are taken at their boxes: see the module docs (`# A part's margin`).
        let margin = match part_dims {
            PartDims::Size(_, _, margin) if !frame.nested => margin,
            _ => Margin::default(),
        };
        let cross_at = match part_dims {
            PartDims::Size(w, h, _) => {
                padding_across((w, h), margin, cross_extent, op.horizontal()).unwrap_or(0)
            }
            _ => 0,
        } as i32;
        placed_parts.push((name.clone(), cursor, cross_at));
        let (placed, across_at) = (cursor * scale, cross_at * scale);
        let (col, row) = if op.horizontal() {
            (placed, across_at)
        } else {
            (across_at, placed)
        };
        refs.push(GlyphRef {
            name: name.clone(),
            raw_name: raw_name.clone(),
            offset: Some((clamp_offset(col), clamp_offset(row))),
            negated: false,
            inherit: false,
            // A jump-through is a `ref` keyword; a derived one carries none.
            goto: false,
            // The derived ref is not conditional: the line already answered
            // that question for the whole of itself above.
            fill: None,
            visibility: None,
            comment: None,
        });
        slot += 1;

        // An unpicked component is still *placed* — the cursor walks over
        // whatever box the bare name happens to have, so the parts that are
        // decided still land where they belong and the editor can draw the
        // glyph as it is filled in — but it says nothing yet, so nothing it
        // says can be wrong.
        if unpicked {
            if let PartDims::Size(w, h, _) = part_dims {
                cursor += if op.horizontal() { w } else { h } as i32;
            }
            continue;
        }
        match part_dims {
            PartDims::Unknown => issues.push((
                Severity::Error,
                at(format!("component '{name}' is not defined")),
            )),
            PartDims::Undeclared => issues.push((
                Severity::Error,
                at(format!(
                    "component '{name}' declares no `W H` on its `glyph` header, so it has \
                     no box to fill a slot with"
                )),
            )),
            PartDims::Size(w, h, _) => {
                if let Some(size) = spec.size
                    && size != (w, h)
                {
                    issues.push((
                        Severity::Error,
                        at(format!(
                            "component '{name}' names {}x{} but the glyph is {w}x{h}",
                            size.0, size.1
                        )),
                    ));
                }
                let (along, across) = if op.horizontal() { (w, h) } else { (h, w) };
                if padding_across((w, h), margin, cross_extent, op.horizontal()).is_none() {
                    // What the margin would have made it, where it has one: the
                    // number to compare with the slot is then that one.
                    let padded = margin
                        .across(op.horizontal())
                        .map_or_else(String::new, |m| {
                            format!(
                                " ({} with its `{}`)",
                                across as u32 + m.0 as u32 + m.1 as u32,
                                Margin::flag(!op.horizontal(), m),
                            )
                        });
                    issues.push((
                        Severity::Error,
                        at(format!(
                            "component '{name}' is {} {across}, not the {whole}'s {cross_extent}{padded}",
                            if op.horizontal() { "tall" } else { "wide" },
                        )),
                    ));
                }
                cursor += along as i32;
            }
        }
    }

    // Only over a line that is otherwise sound: on a line whose parts are not
    // chosen, or that names something no glyph answers to, every clearance is
    // measured against a layout nobody meant, and the warnings would be noise
    // on top of whatever matters.
    if let Some(rule) = clearance
        && !unresolved
        && !issues.iter().any(|(s, _)| *s == Severity::Error)
    {
        let placed_parts: Vec<(&str, i32, i32)> = placed_parts
            .iter()
            .map(|(n, at, across)| (n.as_str(), *at, *across))
            .collect();
        let ink = |name: &str| match nested_keys.iter().find(|(written, _)| written == name) {
            Some((_, key)) => (rule.ink)(key),
            None => (rule.ink)(name),
        };
        let rule = ClearanceRule { ink: &ink, ..*rule };
        issues.extend(
            check_clearances(op, axis_extent, &placed_parts, &rule)
                .into_iter()
                .map(|(severity, message)| (severity, at(message))),
        );
    }
    (refs, issues)
}

/// A derived offset is an `i16` like any other; a source absurd enough to
/// overflow one gets a saturated offset, and a glyph as visibly wrong as the
/// line that asked for it.
pub(super) fn clamp_offset(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}
