mod alias;
#[cfg(feature = "editor")]
mod app;
mod cancel;
mod compose;
mod detail;
mod document;
mod document_io;
#[cfg(feature = "editor")]
mod edit_menu;
#[cfg(feature = "editor")]
mod editor;
mod faces;
#[cfg(test)]
mod golden;
mod issues;
mod math;
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
fn report_issues(docs: &[&document::Document]) -> usize {
    let issues = issues::collect_issues(docs);
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

fn main() {
    startup::init();
    let args: Vec<String> = std::env::args().collect();

    // Probe subcommand: uniform probe --input DIR [--repeat N]
    if args.get(1).map(|s| s.as_str()) == Some("probe") {
        let mut input_dir = None;
        let mut repeats = 1usize;
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
                _ => {
                    eprintln!("Unknown probe option: {}", args[i]);
                    std::process::exit(1);
                }
            }
            i += 1;
        }
        let Some(input) = input_dir else {
            eprintln!("Usage: uniform probe --input <DIR> [--repeat N]");
            std::process::exit(1);
        };
        run_probe(&input, repeats);
        return;
    }

    // Build subcommand: uniform build --input DIR --output FILE [--output FILE ...]
    //   [--sample-html FILE] [--sample-png FILE] [--live-html FILE]
    if args.get(1).map(|s| s.as_str()) == Some("build") {
        let mut input_dir = None;
        let mut output_files: Vec<std::path::PathBuf> = Vec::new();
        let mut sample_html = None;
        let mut sample_png = None;
        let mut live_html = None;
        let mut data_dir = None;
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
                "--sample-html" => {
                    i += 1;
                    sample_html = args.get(i).map(std::path::PathBuf::from);
                }
                "--sample-png" => {
                    i += 1;
                    sample_png = args.get(i).map(std::path::PathBuf::from);
                }
                "--live-html" => {
                    i += 1;
                    live_html = args.get(i).map(std::path::PathBuf::from);
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
                "Usage: uniform build --input <DIR> --output <FILE.ttf|.woff2> [--output ...]"
            );
            std::process::exit(1);
        };
        if output_files.is_empty() {
            eprintln!(
                "Usage: uniform build --input <DIR> --output <FILE.ttf|.woff2> [--output ...]"
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
        let (issue_errors, built) = std::thread::scope(|scope| {
            let build = scope.spawn(|| render::build_faces(&refs));
            (report_issues(&refs), build.join().unwrap())
        });
        // Counted now, acted on at the very end: the outputs are all written
        // first, so a CI run can still publish them while the run itself fails.
        let error_count = parse_errors + issue_errors;
        let Some(built) = built else {
            eprintln!("Font build failed");
            std::process::exit(1);
        };
        // The primary face is what the sample and preview outputs below show;
        // they are one document, not one per typeface.
        let font_bytes = built[0].1.clone();

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

        let mut writes: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();
        for plan in plans {
            match plan {
                faces::OutputPlan::Collection(path) => {
                    let fonts: Vec<Vec<u8>> = built.iter().map(|(_, b)| b.clone()).collect();
                    match render::build_collection(&fonts) {
                        Ok(bytes) => writes.push((path, bytes)),
                        Err(e) => {
                            eprintln!("error: {e}");
                            std::process::exit(1);
                        }
                    }
                }
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
                        let bytes = if is_woff2 {
                            match render::ttf_to_woff2(ttf) {
                                Ok(b) => b,
                                Err(e) => {
                                    eprintln!("{e}");
                                    std::process::exit(1);
                                }
                            }
                        } else {
                            ttf.clone()
                        };
                        writes.push((path, bytes));
                    }
                }
            }
        }

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

        if let Some(path) = sample_html {
            let mut f = std::fs::File::create(&path).unwrap_or_else(|e| {
                eprintln!("Failed to create {}: {e}", path.display());
                std::process::exit(1);
            });
            if let Err(e) = render::sample::write_sample_html(&mut f, &refs) {
                eprintln!("Failed to write sample HTML: {e}");
                std::process::exit(1);
            }
            eprintln!("Wrote {}", path.display());
        }

        if let Some(path) = sample_png {
            let mut f = std::fs::File::create(&path).unwrap_or_else(|e| {
                eprintln!("Failed to create {}: {e}", path.display());
                std::process::exit(1);
            });
            if let Err(e) = render::sample::write_sample_png(&mut f, &refs) {
                eprintln!("Failed to write sample PNG: {e}");
                std::process::exit(1);
            }
            eprintln!("Wrote {}", path.display());
        }

        if let Some(path) = live_html {
            let mut f = std::fs::File::create(&path).unwrap_or_else(|e| {
                eprintln!("Failed to create {}: {e}", path.display());
                std::process::exit(1);
            });
            // The live page embeds whichever WOFF2 the outputs produced, so
            // that it shows the same bytes a browser would load; with none it
            // falls back to the primary face's TTF.
            let woff2_written = writes.iter().find(|(p, _)| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("woff2"))
            });
            let result = if let Some((_, w2)) = woff2_written {
                render::sample::write_live_html_woff2(
                    &mut f,
                    &refs,
                    &font_bytes,
                    w2,
                    data_dir.as_deref(),
                )
            } else {
                render::sample::write_live_html(&mut f, &refs, &font_bytes, data_dir.as_deref())
            };
            if let Err(e) = result {
                eprintln!("Failed to write live HTML: {e}");
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
        let error_count = parse_errors + report_issues(&refs);

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
        eprintln!("Usage: uniform <build|test> [options...]");
        eprintln!("GUI mode requires the 'editor' feature.");
        std::process::exit(1);
    }
}
