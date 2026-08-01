//! Pass over the collected glyphs that emits glyf outlines, metrics, cmap
//! entries and the color layer glyphs.

use super::*;
use super::hints::generate_grid_snap_hints;
use super::tables::glyph_bounds;

/// glyf/loca input plus everything accumulated while adding glyph outlines:
/// metrics, cmap mappings, name→GID assignments and the `maxp` counters.
pub(super) struct OutlineBuild {
    pub(super) glyf_builder: GlyfLocaBuilder,
    pub(super) h_metrics: Vec<LongMetric>,
    pub(super) cmap_mappings: Vec<(char, GlyphId)>,
    pub(super) name_to_gid: HashMap<String, GlyphId16>,
    pub(super) max_points: u16,
    pub(super) max_contours: u16,
    pub(super) max_insn_size: u16,
    pub(super) max_stack: u16,
    pub(super) max_composite_points: u16,
    pub(super) max_composite_contours: u16,
    pub(super) max_component_elements: u16,
    pub(super) max_component_depth: u16,
}

/// What a glyph turned into in `glyf`, kept so the composite `maxp` limits can
/// be computed from the outlines that were actually written.
enum Emitted {
    Simple { points: u32, contours: u32 },
    Composite(Vec<GlyphId16>),
}

/// Passes 1 and 2 of the build: assign GIDs (`.notdef` first), collect cmap
/// mappings, and add every glyph's outline (TrueType composite, empty, or
/// hinted simple glyph) with its horizontal metric.  Pass 3 then sizes the
/// composite `maxp` limits from what pass 2 emitted.
pub(super) fn build_glyph_outlines(
    glyphs: &[CollectedGlyph],
    hint_ppem: u16,
    default_aw: u16,
) -> OutlineBuild {
    let mut out = OutlineBuild {
        glyf_builder: GlyfLocaBuilder::new(),
        h_metrics: vec![LongMetric { advance: default_aw, side_bearing: 0 }],
        cmap_mappings: Vec::new(),
        name_to_gid: HashMap::new(),
        max_points: 0,
        max_contours: 0,
        max_insn_size: 0,
        max_stack: 0,
        max_composite_points: 0,
        max_composite_contours: 0,
        max_component_elements: 0,
        max_component_depth: 0,
    };

    // .notdef (empty glyph)
    out.glyf_builder.add_glyph(&Glyph::Empty).unwrap();

    // A cmap maps each character once, so the same character claimed by two
    // glyphs has to be resolved here rather than left for the table writer to
    // reject: `Cmap::from_mappings` fails on the conflict, and this stage has no
    // way to report anything. Validation already calls the source mistakes that
    // get here errors (a character mapped in both the base slice and an included
    // slice, a character mapped twice), so the build's job is only to stay alive
    // and deterministic long enough for that report to be read — a panic here
    // kills the editor's background build thread and takes the diagnostic with
    // it. First glyph in collection order wins, which is source order.
    let mut mapped: HashSet<char> = HashSet::new();

    // Pass 1: build name→GID mapping and cmap
    for (i, g) in glyphs.iter().enumerate() {
        let glyph_id = GlyphId::new((i + 1) as u32);
        let glyph_id16 = GlyphId16::new((i + 1) as u16);

        for &cp in &g.codepoints {
            if let Some(ch) = char::from_u32(cp)
                && mapped.insert(ch)
            {
                out.cmap_mappings.push((ch, glyph_id));
            }
        }

        out.name_to_gid.entry(g.name.clone()).or_insert(glyph_id16);
    }

    // Which glyphs end up with nothing in `glyf` at all — no contours and no
    // surviving components.  A `ref` at one of these is the deliberate blank
    // placeholder idiom (`ref sp`), and emitting it as a component makes OTS
    // warn "empty gid N used as component in glyph M" for every parent; when it
    // is the only component OTS cannot even repair it, and DirectWrite chokes.
    // So they are dropped from the component list below.
    //
    // Deliberately a fixpoint over the *emitted* shape rather than a recursion,
    // so a chain of blank refs collapses in one place and deep composites
    // cannot grow the stack.  A composite whose contours cancel out to nothing
    // but whose components are real (negated refs) is not empty.
    let mut empty_glyphs: HashSet<&str> = HashSet::new();
    loop {
        let mut changed = false;
        for (i, g) in glyphs.iter().enumerate() {
            // Duplicate names resolve to the first occurrence's GID; only that
            // one describes what a component reference actually points at.
            if out.name_to_gid.get(&g.name) != Some(&GlyphId16::new((i + 1) as u16))
                || empty_glyphs.contains(g.name.as_str())
                || !g.contours.is_empty()
            {
                continue;
            }
            let stays_composite = g
                .composite_refs
                .iter()
                .all(|cr| out.name_to_gid.contains_key(&cr.component_name))
                && g.composite_refs
                    .iter()
                    .any(|cr| !empty_glyphs.contains(cr.component_name.as_str()));
            if !stays_composite {
                empty_glyphs.insert(g.name.as_str());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Pass 2: build glyph outlines, recording what each one turned into so
    // that pass 3 can size the `maxp` composite limits.
    let mut emitted: Vec<Emitted> = Vec::with_capacity(glyphs.len() + 1);
    emitted.push(Emitted::Simple { points: 0, contours: 0 }); // .notdef
    for g in glyphs.iter() {
        let live_refs: Vec<&CompositeRef> = g
            .composite_refs
            .iter()
            .filter(|cr| !empty_glyphs.contains(cr.component_name.as_str()))
            .collect();
        let is_composite = !live_refs.is_empty()
            && g.composite_refs.iter().all(|cr| out.name_to_gid.contains_key(&cr.component_name));

        if is_composite {
            let mut comp_glyph: Option<CompositeGlyph> = None;
            for cr in &live_refs {
                let comp_gid = out.name_to_gid[&cr.component_name];
                let component = Component::new(
                    comp_gid,
                    Anchor::Offset { x: cr.x_offset, y: cr.y_offset },
                    read_fonts::tables::glyf::Transform::default(),
                    ComponentFlags {
                        round_xy_to_grid: true,
                        overlap_compound: live_refs.len() > 1,
                        ..Default::default()
                    },
                );
                let (gx0, gy0, gx1, gy1) = glyph_bounds(&g.contours);
                let bbox = Bbox { x_min: gx0, y_min: gy0, x_max: gx1, y_max: gy1 };
                match comp_glyph.as_mut() {
                    None => comp_glyph = Some(CompositeGlyph::new(component, bbox)),
                    Some(cg) => cg.add_component(component, bbox),
                }
            }
            let cg = comp_glyph.unwrap();
            out.max_component_elements =
                out.max_component_elements.max(live_refs.len() as u16);
            emitted.push(Emitted::Composite(
                live_refs.iter().map(|cr| out.name_to_gid[&cr.component_name]).collect(),
            ));

            let (gx0, ..) = glyph_bounds(&g.contours);

            out.h_metrics.push(LongMetric {
                advance: g.advance_width,
                side_bearing: gx0,
            });

            out.glyf_builder.add_glyph(&cg).unwrap();
        } else if g.contours.is_empty() {
            emitted.push(Emitted::Simple { points: 0, contours: 0 });
            out.glyf_builder.add_glyph(&Glyph::Empty).unwrap();
            out.h_metrics.push(LongMetric {
                advance: g.advance_width,
                side_bearing: 0,
            });
        } else {
            let mut hinted_contours = g.contours.clone();
            let instructions = if hint_ppem > 0 {
                generate_grid_snap_hints(&mut hinted_contours, hint_ppem)
            } else {
                Vec::new()
            };

            let contours: Vec<Contour> = hinted_contours
                .iter()
                .map(|c| {
                    let points: Vec<CurvePoint> = c
                        .iter()
                        .map(|&(x, y)| CurvePoint::on_curve(x, y))
                        .collect();
                    Contour::from(points)
                })
                .collect();

            let n_points: usize = contours.iter().map(|c| c.len()).sum();
            emitted.push(Emitted::Simple {
                points: n_points as u32,
                contours: contours.len() as u32,
            });
            out.max_points = out.max_points.max(n_points as u16);
            out.max_contours = out.max_contours.max(contours.len() as u16);
            out.max_insn_size = out.max_insn_size.max(instructions.len() as u16);
            if !instructions.is_empty() {
                out.max_stack = out.max_stack.max(2);
            }

            let mut sg = SimpleGlyph {
                bbox: Bbox::default(),
                contours,
                instructions,
            };
            sg.recompute_bounding_box();

            // Extend the base glyph's bbox to cover COLR layers and the
            // full advance width so that renderers clipping COLRv0 to
            // the base glyph's bbox don't cut off coloronly content.
            if !g.color_layers.is_empty() {
                for cl in &g.color_layers {
                    for c in &cl.contours {
                        for &(x, y) in c {
                            sg.bbox.x_min = sg.bbox.x_min.min(x);
                            sg.bbox.y_min = sg.bbox.y_min.min(y);
                            sg.bbox.x_max = sg.bbox.x_max.max(x);
                            sg.bbox.y_max = sg.bbox.y_max.max(y);
                        }
                    }
                }
                sg.bbox.x_max = sg.bbox.x_max.max(g.advance_width as i16);
            }

            out.h_metrics.push(LongMetric {
                advance: g.advance_width,
                side_bearing: sg.bbox.x_min,
            });

            out.glyf_builder.add_glyph(&sg).unwrap();
        }
    }

    // Pass 3: the composite `maxp` limits.  Per the spec these count the glyph
    // *fully decomposed*, so they cannot be read off the parent's own outline:
    // components are emitted with grid-snap hints, which insert points along
    // diagonals that the parent's pre-hinting contours never had.  Nesting
    // depth likewise has to be measured — `map`ping a decomposable codepoint
    // builds composites out of composites, several levels deep.
    //
    // Relaxed to a fixpoint rather than recursed, for the same reasons as the
    // empty-glyph pass above.
    let mut totals: Vec<Option<(u32, u32, u16)>> = emitted
        .iter()
        .map(|e| match e {
            Emitted::Simple { points, contours } => Some((*points, *contours, 0)),
            Emitted::Composite(_) => None,
        })
        .collect();
    loop {
        let mut changed = false;
        for (i, e) in emitted.iter().enumerate() {
            let Emitted::Composite(comps) = e else { continue };
            if totals[i].is_some() {
                continue;
            }
            let mut acc = (0u32, 0u32, 0u16);
            for gid in comps {
                let Some((p, n, d)) = totals[gid.to_u16() as usize] else {
                    acc = (0, 0, 0);
                    break;
                };
                acc = (acc.0 + p, acc.1 + n, acc.2.max(d + 1));
            }
            if acc.2 > 0 {
                totals[i] = Some(acc);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (i, e) in emitted.iter().enumerate() {
        if !matches!(e, Emitted::Composite(_)) {
            continue;
        }
        let Some((points, contours, depth)) = totals[i] else { continue };
        out.max_composite_points = out.max_composite_points.max(points.min(u16::MAX as u32) as u16);
        out.max_composite_contours =
            out.max_composite_contours.max(contours.min(u16::MAX as u32) as u16);
        out.max_component_depth = out.max_component_depth.max(depth);
    }

    out
}

/// COLRv0: appends one extra outline glyph per color layer (allocating GIDs
/// from `num_glyphs` onward) and returns the COLR base-glyph/layer records.
pub(super) fn add_color_layer_glyphs(
    glyphs: &[CollectedGlyph],
    out: &mut OutlineBuild,
    num_glyphs: &mut u16,
) -> (Vec<BaseGlyph>, Vec<ColrLayer>) {
    let mut colr_base_glyphs: Vec<BaseGlyph> = Vec::new();
    let mut colr_layers: Vec<ColrLayer> = Vec::new();

    for (i, g) in glyphs.iter().enumerate() {
        if g.color_layers.is_empty() {
            continue;
        }
        let base_gid = GlyphId16::new((i + 1) as u16);
        let first_layer_index = colr_layers.len() as u16;

        for cl in &g.color_layers {
            let layer_gid_val = *num_glyphs;
            *num_glyphs += 1;

            if cl.contours.is_empty() {
                out.glyf_builder.add_glyph(&Glyph::Empty).unwrap();
                out.h_metrics.push(LongMetric {
                    advance: g.advance_width,
                    side_bearing: 0,
                });
            } else {
                let contours: Vec<Contour> = cl
                    .contours
                    .iter()
                    .map(|c| {
                        let points: Vec<CurvePoint> = c
                            .iter()
                            .map(|&(x, y)| CurvePoint::on_curve(x, y))
                            .collect();
                        Contour::from(points)
                    })
                    .collect();
                // Layer glyphs are real outlines with their own GIDs, so they
                // count towards `maxp` like any other simple glyph.  A merged
                // foreground layer is often the largest outline in the font,
                // its pieces being inlined on-demand refs that exist nowhere
                // else.
                out.max_points =
                    out.max_points.max(contours.iter().map(|c| c.len()).sum::<usize>() as u16);
                out.max_contours = out.max_contours.max(contours.len() as u16);

                let mut sg = SimpleGlyph {
                    bbox: Bbox::default(),
                    contours,
                    instructions: Vec::new(),
                };
                sg.recompute_bounding_box();
                let lsb = sg.bbox.x_min;
                out.glyf_builder.add_glyph(&sg).unwrap();
                out.h_metrics.push(LongMetric {
                    advance: g.advance_width,
                    side_bearing: lsb,
                });
            }

            colr_layers.push(ColrLayer::new(
                GlyphId16::new(layer_gid_val),
                cl.palette_index,
            ));
        }

        colr_base_glyphs.push(BaseGlyph::new(
            base_gid,
            first_layer_index,
            g.color_layers.len() as u16,
        ));
    }

    (colr_base_glyphs, colr_layers)
}

/// Font-wide bounds and metric extremes for `head`/`hhea`.
pub(super) struct GlobalBounds {
    pub(super) x_min: i16,
    pub(super) y_min: i16,
    pub(super) x_max: i16,
    pub(super) y_max: i16,
    pub(super) aw_max: u16,
    pub(super) min_lsb: i16,
    pub(super) min_rsb: i16,
    pub(super) x_max_extent: i16,
}

pub(super) fn compute_global_bounds(
    glyphs: &[CollectedGlyph],
    h_metrics: &[LongMetric],
    default_aw: u16,
    ascender: i16,
    descender: i16,
) -> GlobalBounds {
    let mut b = GlobalBounds {
        x_min: 0,
        y_min: descender,
        x_max: default_aw as i16,
        y_max: ascender,
        aw_max: default_aw,
        min_lsb: 0,
        min_rsb: i16::MAX,
        x_max_extent: 0,
    };

    for (i, g) in glyphs.iter().enumerate() {
        let m = &h_metrics[i + 1];
        b.aw_max = b.aw_max.max(m.advance);
        if !g.contours.is_empty() {
            let (gx0, gy0, gx1, gy1) = glyph_bounds(&g.contours);
            b.x_min = b.x_min.min(gx0);
            b.y_min = b.y_min.min(gy0);
            b.x_max = b.x_max.max(gx1);
            b.y_max = b.y_max.max(gy1);
            b.min_lsb = b.min_lsb.min(gx0);
            let rsb = m.advance as i16 - gx1;
            b.min_rsb = b.min_rsb.min(rsb);
            b.x_max_extent = b.x_max_extent.max(gx1);
        }
    }
    if b.min_rsb == i16::MAX {
        b.min_rsb = 0;
    }
    b
}
