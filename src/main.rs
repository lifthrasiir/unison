mod alias;
#[cfg(feature = "editor")]
mod app;
mod audit;
mod cancel;
mod compose;
mod detail;
mod document;
mod document_io;
#[cfg(feature = "editor")]
mod edit_menu;
#[cfg(feature = "editor")]
mod editor;
mod exists;
mod faces;
mod fix;
mod glyph_flags;
#[cfg(test)]
mod golden;
mod issues;
mod math;
mod merge;
mod meta;
mod on_demand;
mod parallel;
mod pattern;
mod pixel;
#[cfg(feature = "editor")]
mod preview;
mod ref_composite;
mod render;
mod resolve;
mod samples;
mod script_run;
#[cfg(feature = "editor")]
mod sidebar;
#[cfg(feature = "editor")]
mod specimen;
mod startup;
mod ucd;

#[cfg(target_os = "windows")]
extern crate windows_core;

/// Load a font directory, reporting files that failed to parse.
///
/// `load_docs_from_directory` drops them silently, so a single bad file used
/// to produce a font quietly built from everything else. Returns how many files
/// were skipped, so the caller can fail the run over it.
fn load_docs_reporting_errors(dir: &std::path::Path) -> (Vec<document::Document>, usize) {
    let (docs, errors) = render::ttf_builder::load_docs_from_directory_checked(dir);
    for (path, msg) in &errors {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        eprintln!("error: {file}: {msg}");
    }
    if !errors.is_empty() {
        eprintln!("{} file(s) failed to parse and were skipped", errors.len());
    }
    let count = errors.len();
    (docs, count)
}

/// Print the same validation the editor shows, and return how many of the
/// problems were errors. The font build proceeds regardless — a reference that
/// resolves to nothing simply produces no glyph — so without this a broken
/// document only shows up as a missing glyph much later.
///
/// The count is what makes `build` exit non-zero: every output file is still
/// written (a CI run can deploy them), but the run itself is a failure. Only
/// [`issues::Severity::Error`] counts — a `todo` is work that has not been done
/// rather than a defect, and there are expected to be tens of thousands of them
/// (see [`issues::Severity`]).
///
/// Which is also why a `todo` is *counted* here rather than printed: the
/// editor's issue list is where a work queue that size is read, with a filter
/// over it, and a build log that scrolls it past is a build log nobody reads.
/// Every other severity prints its line.
fn report_issues(docs: &[&document::Document], resolution: &resolve::Resolution) -> usize {
    let issues = issues::collect_issues_with(docs, resolution);
    if issues.is_empty() {
        return 0;
    }
    let count = |sev: issues::Severity| issues.iter().filter(|i| i.severity == sev).count();
    let errors = count(issues::Severity::Error);
    let todos = count(issues::Severity::Todo);
    for issue in issues
        .iter()
        .filter(|i| i.severity != issues::Severity::Todo)
    {
        let file = issue
            .file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        eprintln!(
            "{}: {file}:{}: {}",
            issue.severity.label(),
            issue.file_line,
            issue.message
        );
    }
    let todos = if todos > 0 {
        format!(", {todos} todo(s)")
    } else {
        String::new()
    };
    eprintln!("{} problem(s), {} error(s){todos}", issues.len(), errors);
    errors
}

/// `uniform fix --input DIR --optimize-clearance [--dry-run]`
///
/// The one command that writes the *source* back. It is deliberately narrow:
/// each `--…` flag names one thing to fix, nothing runs without one, and every
/// line it would rewrite is printed whether or not it is written, so a run can
/// be read as a diff before it is trusted. See [`fix`].
///
/// Returns the process exit code: 1 when a file could not be read or written,
/// 0 otherwise — a source with nothing to fix is a success.
fn run_fix(input: &std::path::Path, optimize_clearance: bool, dry_run: bool) -> i32 {
    let (docs, errors, sources) = render::ttf_builder::load_docs_from_directory_with_sources(input);
    for (path, msg) in &errors {
        eprintln!("error: {}: {msg}", path.display());
    }
    if docs.is_empty() {
        eprintln!("No .unf files found in {}", input.display());
        return 1;
    }
    let refs: Vec<&document::Document> = docs.iter().collect();

    let mut planned = 0usize;
    let mut written = 0usize;
    let mut failures = errors.len();
    if optimize_clearance {
        for doc_fixes in fix::clearance::optimize_clearance(&refs) {
            let doc = refs[doc_fixes.doc_idx];
            let Some((_, bytes)) = sources.get(doc_fixes.doc_idx) else {
                continue;
            };
            let text = String::from_utf8_lossy(bytes).into_owned();
            let lines: Vec<&str> = text.split('\n').collect();
            let file = doc
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            let mut edits: Vec<(usize, String)> = Vec::new();
            for f in &doc_fixes.fixes {
                let Some(line) = fix::compose_file_line(doc, f.item_idx, f.compose_idx, &lines)
                else {
                    eprintln!(
                        "warning: {file}: glyph '{}': its IDC line moved; not rewritten",
                        f.glyph,
                    );
                    continue;
                };
                eprintln!(
                    "{file}:{}: glyph '{}': {} -> {}  ({} -> {}{}{})",
                    line + 1,
                    f.glyph,
                    lines[line].trim(),
                    f.new_line,
                    // A line with no layout of its own was never measured;
                    // there is no number to have improved on. Which of the two
                    // reasons it is matters to whoever reads the diff: a
                    // component with no variant picked is work not yet done,
                    // and one the check errors on is work done wrong.
                    match (f.before, f.faulty) {
                        (Some(before), _) => before.to_string(),
                        (None, true) => "error".to_string(),
                        (None, false) => "todo".to_string(),
                    },
                    f.after,
                    // A line whose clearances were already right is rewritten
                    // for the other thing the check warns about, and then the
                    // scores alone would read as a no-op.
                    match f.mismatched {
                        Some((before, after)) if before != after => {
                            format!(", {before} -> {after} in the wrong slot")
                        }
                        _ => String::new(),
                    },
                    // A pattern line is scored over the family it stands for,
                    // where the score is the tie-break and the count of glyphs
                    // still warning is the point.
                    match f.glyphs_warning {
                        Some((before, after)) => format!(", {before} -> {after} glyphs warning"),
                        None => String::new(),
                    },
                );
                edits.push((line, f.new_line.clone()));
                planned += 1;
            }
            if edits.is_empty() || dry_run {
                continue;
            }
            match std::fs::write(&doc.path, fix::rewrite_lines(&text, &edits)) {
                Ok(()) => written += edits.len(),
                Err(e) => {
                    eprintln!("error: {file}: {e}");
                    failures += 1;
                }
            }
        }
    }

    if dry_run {
        eprintln!("{planned} line(s) would be rewritten (--dry-run).");
    } else {
        eprintln!("{written} line(s) rewritten.");
    }
    i32::from(failures > 0)
}

/// One rebuild's stages, in the shape `app::background::UniformApp::rebuild`
/// runs them: the expansion once, the font build beside validation, and the
/// recomposition last.
#[cfg(feature = "editor")]
struct RebuildTiming {
    label: &'static str,
    expand: std::time::Duration,
    font: std::time::Duration,
    validate: std::time::Duration,
    flags: std::time::Duration,
    recompose: std::time::Duration,
    /// What the specimen reads out of the documents, which the editor collects
    /// beside the recomposition and only when its tab is open.
    specimen: std::time::Duration,
}

#[cfg(feature = "editor")]
impl RebuildTiming {
    /// What the edit costs, with the pairs that overlap counted once: the font
    /// build runs beside validation, and the recomposition beside the
    /// specimen's data.
    fn total(&self) -> std::time::Duration {
        self.expand + self.font.max(self.validate + self.flags) + self.recompose.max(self.specimen)
    }
}

/// `uniform probe --input DIR --edit`
///
/// What *one edit* costs the editor, which is a different question from what
/// starting it costs: by the time an edit lands the contour and composite
/// caches are warm, so the startup numbers say nothing about the wait between
/// clicking a pixel and seeing the font.
///
/// Four passes, in the order that makes the numbers readable: a cold build, a
/// second build with nothing changed (which is the warm floor), then one pixel
/// and then one whole glyph block. The last two are what a source is actually
/// edited by, and comparing them to the warm floor is the point — a floor as
/// high as the edits means nothing is being reused between rebuilds.
///
/// The stages are the editor's own, in the order `app::background` runs them:
/// one expansion, then the font build beside validation, then the
/// recomposition that consumes the expansion. The font build and validation
/// overlap in the editor, so the total counts the longer of the two rather than
/// both.
///
/// Which glyph is edited is deliberately arbitrary — the first one with a grid
/// — because the cost of a rebuild does not depend on it: nothing downstream
/// of the caches is keyed on what changed.
#[cfg(feature = "editor")]
fn run_edit_probe(input: &std::path::Path) {
    let never = cancel::CancelToken::never();
    let (mut docs, errors, _sources) =
        render::ttf_builder::load_docs_from_directory_with_sources(input);
    if !errors.is_empty() {
        eprintln!("{} file(s) failed to parse", errors.len());
    }
    if docs.is_empty() {
        eprintln!("no .unf files in {}", input.display());
        std::process::exit(1);
    }

    let contour_cache = render::new_contour_cache();
    let mut grid_cache = ref_composite::CompositeGridCache::default();
    let mut rows: Vec<RebuildTiming> = Vec::new();

    let rebuild = |docs: &[document::Document],
                   contour_cache: &render::SharedContourCache,
                   grid_cache: &mut ref_composite::CompositeGridCache,
                   label: &'static str| {
        let refs: Vec<&document::Document> = docs.iter().collect();

        // The one expansion both halves below read.
        let t = std::time::Instant::now();
        let Some(resolution) = resolve::Resolution::compute_cancellable(&refs, &never) else {
            eprintln!("resolution produced nothing; is `meta height` set?");
            std::process::exit(1);
        };
        let expand = t.elapsed();

        // Timed one after the other rather than at once: what is wanted here is
        // what each stage costs, and the report combines them the way the
        // editor's threads do.
        let t = std::time::Instant::now();
        let font =
            render::build_font_pair_cached_from(&refs, contour_cache, &resolution, None, &never);
        let font_took = t.elapsed();

        let t = std::time::Instant::now();
        let issues = issues::collect_issues_with(&refs, &resolution);
        let validate = t.elapsed();

        let t = std::time::Instant::now();
        let _flags = glyph_flags::collect(&refs, &issues, &resolution.expansion);
        let flags = t.elapsed();

        let name_parts = resolution.name_parts;
        let t = std::time::Instant::now();
        let _ = ref_composite::resolve_expansion_cached(
            resolution.expansion,
            &name_parts,
            &never,
            Some(grid_cache),
        );
        let recompose = t.elapsed();

        // What the editor collects when the specimen tab is open, which is when
        // it is on the critical path at all.
        let t = std::time::Instant::now();
        let _specimen = font.as_ref().map(|f| {
            let (exists, _) = exists::resolve_scopes(&refs, &name_parts);
            let aliases = alias::AliasMap::collect_with_merges(&refs, &name_parts, &exists);
            specimen::SpecimenData::collect(
                &refs,
                &name_parts,
                &exists,
                &aliases,
                &f.name_to_gid,
                None,
                &_flags,
            )
        });
        let specimen = t.elapsed();

        RebuildTiming {
            label,
            expand,
            font: font_took,
            validate,
            flags,
            recompose,
            specimen,
        }
    };

    rows.push(rebuild(
        &docs,
        &contour_cache,
        &mut grid_cache,
        "cold (first build)",
    ));
    rows.push(rebuild(
        &docs,
        &contour_cache,
        &mut grid_cache,
        "warm (nothing changed)",
    ));

    if !edit_one_pixel(&mut docs) {
        eprintln!("no glyph with a pixel grid to edit");
        std::process::exit(1);
    }
    rows.push(rebuild(&docs, &contour_cache, &mut grid_cache, "one pixel"));

    add_one_glyph_block(&mut docs);
    rows.push(rebuild(
        &docs,
        &contour_cache,
        &mut grid_cache,
        "one glyph block",
    ));

    print!("{}", edit_probe_report(&rows));
}

/// Flip the first pixel of the first glyph that has a grid, the way the editor's
/// own pixel-only path does — the grid in place, both generations bumped.
#[cfg(feature = "editor")]
fn edit_one_pixel(docs: &mut [document::Document]) -> bool {
    for doc in docs.iter_mut() {
        for item in doc.items.iter_mut() {
            let document::DocumentItem::Glyph { body, .. } = item else {
                continue;
            };
            let Some(grid) = body.pixels.as_mut() else {
                continue;
            };
            if grid.width == 0 || grid.height == 0 {
                continue;
            }
            let was_empty = grid.get(0, 0).shape_id() == pixel::PX_EMPTY;
            grid.set(
                0,
                0,
                if was_empty {
                    pixel::PixelShape::new(pixel::PX_FULL, true)
                } else {
                    pixel::PixelShape::new(pixel::PX_EMPTY, false)
                },
            );
            doc.pixel_gen += 1;
            doc.content_gen += 1;
            return true;
        }
    }
    false
}

/// Append one glyph block — the smallest change that alters the *structure*
/// rather than a drawing, and so the one that tells a cache keyed on the glyph
/// set from a cache keyed on the pixels.
#[cfg(feature = "editor")]
fn add_one_glyph_block(docs: &mut [document::Document]) {
    let Some(doc) = docs.first_mut() else {
        return;
    };
    let body = doc.items.iter().find_map(|item| match item {
        document::DocumentItem::Glyph { body, .. } if body.pixels.is_some() => Some(body.clone()),
        _ => None,
    });
    let Some(body) = body else { return };
    doc.items.push(document::DocumentItem::Glyph {
        name: document::GlyphName("probe-added-glyph".to_string()),
        body,
    });
    doc.item_line_starts.push(0);
    doc.content_gen += 1;
}

#[cfg(feature = "editor")]
fn edit_probe_report(rows: &[RebuildTiming]) -> String {
    fn ms(d: std::time::Duration) -> String {
        format!("{:.1} ms", d.as_secs_f64() * 1000.0)
    }
    let mut out = String::new();
    out.push_str("Rebuild timing after one edit\n");
    out.push_str("=============================\n\n");
    if cfg!(debug_assertions) {
        out.push_str(
            "(debug build \u{2014} compute stages are 10-30x slower than a release build)\n\n",
        );
    }
    out.push_str(
        "One expansion feeds the font build and validation, which run at once;\n\
         the recomposition and the specimen's data then run at once too. The\n\
         total counts each pair once. The specimen is only collected when its\n\
         tab is open, which is the case measured here.\n\n",
    );
    out.push_str(
        "pass                            expand   font build     validate      \
         flags   recompose     specimen        total\n",
    );
    for r in rows {
        out.push_str(&format!(
            "  {:<24}{:>12}{:>13}{:>13}{:>11}{:>12}{:>13}{:>13}\n",
            r.label,
            ms(r.expand),
            ms(r.font),
            ms(r.validate),
            ms(r.flags),
            ms(r.recompose),
            ms(r.specimen),
            ms(r.total()),
        ));
    }
    out
}

/// `uniform probe --input DIR [--repeat N]`
///
/// The startup path with no window in the way: how long the process took to
/// reach `main`, how long enumerating and reading the directory takes, and how
/// long the initial font build takes. Repeating the directory load separates a
/// cold cache (the first pass) from a warm one — on a share that difference is
/// usually the whole story. See `startup.rs`.
fn run_probe(input: &std::path::Path, repeats: usize) {
    for pass in 0..repeats {
        if pass > 0 {
            startup::restart_collection();
            eprintln!("\n--- pass {} (caches warm) ---\n", pass + 1);
        }
        startup::mark("start");
        let (docs, errors, _sources) =
            render::ttf_builder::load_docs_from_directory_with_sources(input);
        startup::mark(format!("load {} file(s)", docs.len()));
        if !errors.is_empty() {
            eprintln!("{} file(s) failed to parse", errors.len());
        }
        let refs: Vec<&document::Document> = docs.iter().collect();

        let name_parts = document::collect_name_parts(&refs);
        let _ = ref_composite::resolve_named_glyphs_with_parts(&refs, &name_parts);
        startup::mark("resolve");

        // Exactly what the editor runs before its first frame.
        #[cfg(feature = "editor")]
        {
            let cache = render::new_contour_cache();
            let built = render::build_font_pair_cached(&refs, &cache).is_some();
            startup::mark(if built {
                "initial font build"
            } else {
                "initial font build (failed)"
            });
        }

        let _ = issues::collect_issues(&refs);
        startup::mark("validation");

        startup::first_frame_done();
        print!("{}", startup::report());
    }
}

/// Windows only, and for one reason: the work here is allocation-bound —
/// name expansion, validation and the glyph graph are millions of short-lived
/// `String`s and small maps — and the platform heap serializes on a lock that
/// two busy threads contend for. Measured against this same source on macOS,
/// the compute-bound stages (exact geometry) run about 4x slower on the test
/// Windows machine while the allocation-bound ones run 7x slower; the gap
/// between those two numbers is the heap, not the CPU.
///
/// Nothing else on the platform is worth swapping the allocator for, so the
/// other targets keep theirs: macOS's is already a per-thread magazine
/// allocator and shows no such gap.
#[cfg(target_os = "windows")]
#[global_allocator]
static GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    startup::init();
    let args: Vec<String> = std::env::args().collect();

    // Probe subcommand: uniform probe --input DIR [--repeat N] [--edit]
    if args.get(1).map(|s| s.as_str()) == Some("probe") {
        let mut input_dir = None;
        let mut repeats = 1usize;
        let mut edit = false;
        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "--input" | "-i" => {
                    i += 1;
                    input_dir = args.get(i).map(std::path::PathBuf::from);
                }
                "--repeat" | "-n" => {
                    i += 1;
                    repeats = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(1).max(1);
                }
                "--edit" => edit = true,
                _ => {
                    eprintln!("Unknown probe option: {}", args[i]);
                    std::process::exit(1);
                }
            }
            i += 1;
        }
        let Some(input) = input_dir else {
            eprintln!("Usage: uniform probe --input <DIR> [--repeat N] [--edit]");
            std::process::exit(1);
        };
        if edit {
            #[cfg(feature = "editor")]
            run_edit_probe(&input);
            #[cfg(not(feature = "editor"))]
            eprintln!("`--edit` measures the editor's rebuild and needs the `editor` feature");
        } else {
            run_probe(&input, repeats);
        }
        return;
    }

    // Fix subcommand: uniform fix --input DIR --optimize-clearance [--dry-run]
    if args.get(1).map(|s| s.as_str()) == Some("fix") {
        let mut input_dir = None;
        let mut optimize_clearance = false;
        let mut dry_run = false;
        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "--input" | "-i" => {
                    i += 1;
                    input_dir = args.get(i).map(std::path::PathBuf::from);
                }
                "--optimize-clearance" => optimize_clearance = true,
                "--dry-run" | "-n" => dry_run = true,
                _ => {
                    eprintln!("Unknown fix option: {}", args[i]);
                    std::process::exit(1);
                }
            }
            i += 1;
        }
        let (Some(input), true) = (input_dir, optimize_clearance) else {
            // Nothing runs without a flag saying what to fix: a `fix` that
            // guesses is a `fix` nobody can run twice with confidence.
            eprintln!("Usage: uniform fix --input <DIR> --optimize-clearance [--dry-run]");
            std::process::exit(1);
        };
        std::process::exit(run_fix(&input, optimize_clearance, dry_run));
    }

    // Build subcommand: uniform build --input DIR --output FILE [--output FILE ...]
    //   [--demo-html FILE]
    if args.get(1).map(|s| s.as_str()) == Some("build") {
        let mut input_dir = None;
        let mut output_files: Vec<std::path::PathBuf> = Vec::new();
        let mut demo_html = None;
        let mut data_dir = None;
        let mut woff2_quality = render::Woff2Quality::default();
        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "--input" | "-i" => {
                    i += 1;
                    input_dir = args.get(i).map(std::path::PathBuf::from);
                }
                "--output" | "-o" => {
                    i += 1;
                    if let Some(p) = args.get(i) {
                        output_files.push(std::path::PathBuf::from(p));
                    }
                }
                "--demo-html" => {
                    i += 1;
                    demo_html = args.get(i).map(std::path::PathBuf::from);
                }
                "--woff2-quality" => {
                    i += 1;
                    let Some(q) = args.get(i).and_then(|s| render::Woff2Quality::parse(s)) else {
                        eprintln!("--woff2-quality takes `fast` or `max`");
                        std::process::exit(1);
                    };
                    woff2_quality = q;
                }
                "--data-dir" | "-d" => {
                    i += 1;
                    data_dir = args.get(i).map(std::path::PathBuf::from);
                }
                _ => {
                    eprintln!("Unknown build option: {}", args[i]);
                    std::process::exit(1);
                }
            }
            i += 1;
        }

        let Some(input) = input_dir else {
            eprintln!(
                "Usage: uniform build --input <DIR> --output <FILE.ttf|.woff2> [--output ...] \
                 [--woff2-quality fast|max]"
            );
            std::process::exit(1);
        };
        if output_files.is_empty() {
            eprintln!(
                "Usage: uniform build --input <DIR> --output <FILE.ttf|.woff2> [--output ...] \
                 [--woff2-quality fast|max]"
            );
            std::process::exit(1);
        }

        let (docs, parse_errors) = load_docs_reporting_errors(&input);
        if docs.is_empty() {
            eprintln!("No .unf files found in {}", input.display());
            std::process::exit(1);
        }
        let refs: Vec<&document::Document> = docs.iter().collect();
        let faces = faces::FaceSet::collect(&refs);
        // Validation reads the documents and nothing the build writes, so it
        // runs alongside it rather than in front of it — on a font this size it
        // is a second of the wall clock either way. The report is still printed
        // before anything the build has to say, so a log reads as it always did.
        //
        // The demo page resolves its own glyphs, from the same documents and
        // sharing nothing with either, so that goes here too rather than in
        // front of the output that wants it.
        let wants_sample = demo_html.is_some();
        // The build, validation and the sample all want the same expansion —
        // the union of every slice, which is face-independent (see
        // `faces::FaceSet::union`) — and it is the larger half of what any of
        // them costs. So it is computed once, up front, and lent to all three;
        // it used to be computed twice, once inside the build and once beside
        // it, which is also how a line only a secondary face includes came to
        // be built but never validated.
        let resolution = {
            let _t = startup::PerfStage::new("resolve");
            resolve::Resolution::compute(&refs)
        };
        let (issue_errors, built, sample_source) = std::thread::scope(|scope| {
            let build = scope.spawn(|| {
                let _t = startup::PerfStage::new("build faces");
                render::build_faces_from(&refs, &resolution.expansion)
            });
            let sample = wants_sample.then(|| {
                scope.spawn(|| {
                    let _t = startup::PerfStage::new("sample resolve");
                    render::sample::SampleSource::collect_with(&refs, &resolution)
                })
            });
            let errors = {
                let _t = startup::PerfStage::new("validate");
                report_issues(&refs, &resolution)
            };
            (
                errors,
                build.join().unwrap(),
                sample.map(|h| h.join().unwrap()),
            )
        });
        let sample_source = match sample_source {
            Some(None) => {
                eprintln!("Failed to build the sample: no glyph data");
                std::process::exit(1);
            }
            other => other.flatten(),
        };
        // Counted now, acted on at the very end: the outputs are all written
        // first, so a CI run can still publish them while the run itself fails.
        let error_count = parse_errors + issue_errors;
        let Some(built) = built else {
            eprintln!("Font build failed");
            std::process::exit(1);
        };
        // Every `--output` is planned before anything is written, so a wrong
        // combination fails before it has half-produced a set of files.
        let plans: Vec<faces::OutputPlan> = output_files
            .iter()
            .map(|o| match faces::plan_output(o, &faces.faces) {
                Ok(plan) => plan,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            })
            .collect();

        // What each output is, before any of it is produced. Splitting the plan
        // from the work is what lets the work run all at once below: a WOFF2 is
        // a second or more of brotli per face, and none of them needs
        // another's result.
        enum OutputWork<'a> {
            Collection,
            /// Brotli over one face — by far the most expensive of these.
            Woff2(&'a [u8]),
            Plain(&'a [u8]),
        }
        let mut works: Vec<(std::path::PathBuf, OutputWork)> = Vec::new();
        for plan in plans {
            match plan {
                faces::OutputPlan::Collection(path) => works.push((path, OutputWork::Collection)),
                faces::OutputPlan::PerFace(targets) => {
                    for (face_id, path) in targets {
                        let Some((_, ttf)) = built.iter().find(|(id, _)| *id == face_id) else {
                            eprintln!("error: no built face `{face_id}`");
                            std::process::exit(1);
                        };
                        let is_woff2 = path
                            .extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| e.eq_ignore_ascii_case("woff2"));
                        works.push((
                            path,
                            if is_woff2 {
                                OutputWork::Woff2(ttf)
                            } else {
                                OutputWork::Plain(ttf)
                            },
                        ));
                    }
                }
            }
        }

        // Every font output at once: a WOFF2 is a second or more of brotli per
        // face, and none of them needs another's result.
        //
        // Nothing here is written to disk yet — a failure in any one of these
        // still leaves the run's files as they were, rather than half replaced.
        let writes = std::thread::scope(|scope| {
            let font_jobs: Vec<_> = works
                .iter()
                .map(|(_path, work)| {
                    let built = &built;
                    scope.spawn(move || {
                        let _t = startup::PerfStage::new(match work {
                            OutputWork::Collection => "output collection",
                            OutputWork::Woff2(_) => "output woff2",
                            OutputWork::Plain(_) => "output ttf",
                        });
                        match work {
                            OutputWork::Collection => {
                                let fonts: Vec<Vec<u8>> =
                                    built.iter().map(|(_, b)| b.clone()).collect();
                                render::build_collection(&fonts).unwrap_or_else(|e| {
                                    eprintln!("error: {e}");
                                    std::process::exit(1);
                                })
                            }
                            OutputWork::Woff2(ttf) => render::ttf_to_woff2(ttf, woff2_quality)
                                .unwrap_or_else(|e| {
                                    eprintln!("{e}");
                                    std::process::exit(1);
                                }),
                            OutputWork::Plain(ttf) => ttf.to_vec(),
                        }
                    })
                })
                .collect();
            let writes: Vec<(std::path::PathBuf, Vec<u8>)> = works
                .iter()
                .map(|(path, _)| path.clone())
                .zip(font_jobs.into_iter().map(|h| h.join().unwrap()))
                .collect();
            writes
        });

        for (output, data) in &writes {
            match std::fs::write(output, data) {
                Ok(()) => {
                    eprintln!(
                        "Wrote {} ({} bytes from {} files)",
                        output.display(),
                        data.len(),
                        docs.len(),
                    );
                }
                Err(e) => {
                    eprintln!("Write error for {}: {e}", output.display());
                    std::process::exit(1);
                }
            }
        }

        if let Some(path) = demo_html {
            // The demo page embeds the font rather than pictures of it, and
            // it embeds the primary face as one *variable* font: the bitmap
            // drawing for the small sizes and the vector one for the large,
            // switched by the `BMAP` axis. It is not one of the `--output`
            // files — those are the shipping faces, and whether *they* carry
            // the axis is `meta bitmap-axis`'s to say — so it is built here
            // rather than borrowed from the outputs above.
            let Some(ttf) = render::build_face_variable(&refs, faces.primary()) else {
                eprintln!("Failed to build the demo page: no glyph data");
                std::process::exit(1);
            };
            let woff2 = render::ttf_to_woff2(&ttf, woff2_quality).unwrap_or_else(|e| {
                eprintln!("Failed to write the demo page: {e}");
                std::process::exit(1);
            });
            let mut f = std::fs::File::create(&path).unwrap_or_else(|e| {
                eprintln!("Failed to create {}: {e}", path.display());
                std::process::exit(1);
            });
            let fonts = render::demo::DemoFonts {
                woff2: &woff2,
                ttf: &ttf,
            };
            if let Err(e) = render::demo::write_demo_html(
                &mut f,
                sample_source.as_ref().unwrap(),
                &refs,
                fonts,
                data_dir.as_deref(),
            ) {
                eprintln!("Failed to write the demo HTML: {e}");
                std::process::exit(1);
            }
            eprintln!("Wrote {}", path.display());
        }

        if error_count > 0 {
            eprintln!(
                "\nbuild finished with {error_count} error(s); \
                 the written files are missing whatever those errors dropped."
            );
            std::process::exit(1);
        }

        return;
    }

    // Test subcommand: uniform test --input DIR
    if args.get(1).map(|s| s.as_str()) == Some("test") {
        let mut input_dir = None;
        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "--input" | "-i" => {
                    i += 1;
                    input_dir = args.get(i).map(std::path::PathBuf::from);
                }
                _ => {
                    eprintln!("Unknown test option: {}", args[i]);
                    std::process::exit(1);
                }
            }
            i += 1;
        }

        let Some(input) = input_dir else {
            eprintln!("Usage: uniform test --input <DIR>");
            std::process::exit(1);
        };

        let (docs, parse_errors) = load_docs_reporting_errors(&input);
        if docs.is_empty() {
            eprintln!("No .unf files found in {}", input.display());
            std::process::exit(1);
        }
        let refs: Vec<&document::Document> = docs.iter().collect();
        // Same rule as `build`: a validation error fails the run even when
        // every assertion passes.
        let error_count = parse_errors + report_issues(&refs, &resolve::Resolution::compute(&refs));

        let name_parts = document::collect_name_parts(&refs);
        let (resolved, _) = ref_composite::resolve_named_glyphs_with_parts(&refs, &name_parts);

        // One build per face the assertions actually reach — including the
        // primary one, which used to be built here and then a second time as
        // itself when a `for SLICE` assertion named it.
        let shape_result = render::assert::run_assertions(&refs, &mut |face| {
            render::build_font_with_gid_map_for(&refs, face)
        });
        let sd_result = render::assert::run_same_distinct_assertions(&refs, &resolved);

        let total = shape_result.total + sd_result.total;
        let passed = shape_result.passed + sd_result.passed;

        if total == 0 {
            eprintln!("No assertions found.");
            if error_count > 0 {
                std::process::exit(1);
            }
            return;
        }

        for issue in shape_result.issues.iter().chain(sd_result.issues.iter()) {
            let file_name = issue
                .file
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            eprintln!("FAIL {}:{}: {}", file_name, issue.file_line, issue.message);
        }

        let failed = total - passed;
        eprintln!(
            "\n{} assertion(s): {} passed, {} failed.",
            total, passed, failed,
        );

        if failed > 0 || error_count > 0 {
            std::process::exit(1);
        }
        return;
    }

    // GUI mode
    #[cfg(feature = "editor")]
    {
        let font_dir = args.get(1).map(std::path::PathBuf::from);

        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1200.0, 800.0])
                .with_title("Uniform")
                // Both the directory the settings live in (see
                // `app/settings.rs`) and, on Wayland, the id a compositor
                // matches against a `.desktop` entry — which is why it is in
                // reverse-DNS form rather than just "Uniform". Changing it
                // orphans every setting saved under the old one.
                .with_app_id("org.mearie.Uniform"),
            ..Default::default()
        };

        // The marks bracket eframe's own window + wgpu setup, which is the one
        // startup cost that is neither ours nor the loader's.
        startup::mark("main() reached, entering eframe");
        eframe::run_native(
            "uniform",
            options,
            Box::new(move |cc| {
                startup::mark("eframe window + renderer ready");
                let app = app::UniformApp::new(cc, font_dir);
                startup::mark("app constructed");
                Ok(Box::new(app))
            }),
        )
        .expect("failed to run eframe");
    }

    #[cfg(not(feature = "editor"))]
    {
        eprintln!("Usage: uniform <build|test|fix> [options...]");
        eprintln!("GUI mode requires the 'editor' feature.");
        std::process::exit(1);
    }
}
