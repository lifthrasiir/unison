#[cfg(feature = "editor")]
mod app;
mod detail;
mod document;
mod document_io;
mod pattern;
#[cfg(feature = "editor")]
mod edit_menu;
mod pixel;
mod ref_composite;
#[cfg(feature = "editor")]
mod preview;
mod render;
mod script_run;
mod stackmon;
#[cfg(feature = "editor")]
mod editor;
mod issues;
mod resolve;
#[cfg(test)]
mod golden;
#[cfg(feature = "editor")]
mod sidebar;
#[cfg(feature = "editor")]
mod specimen;

#[cfg(target_os = "windows")]
extern crate windows_core;

/// Load a font directory, reporting files that failed to parse.
///
/// `load_docs_from_directory` drops them silently, so a single bad file used
/// to produce a font quietly built from everything else.
fn load_docs_reporting_errors(dir: &std::path::Path) -> Vec<document::Document> {
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
    docs
}

/// Print the same validation the editor shows. The font build proceeds
/// regardless — a reference that resolves to nothing simply produces no glyph
/// — so without this a broken document only shows up as a missing glyph much
/// later.
fn report_issues(docs: &[&document::Document]) {
    let issues = issues::collect_issues(docs);
    if issues.is_empty() {
        return;
    }
    let errors = issues
        .iter()
        .filter(|i| i.severity == issues::Severity::Error)
        .count();
    for issue in &issues {
        let label = match issue.severity {
            issues::Severity::Error => "error",
            issues::Severity::Warning => "warning",
        };
        let file = issue
            .file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        eprintln!("{label}: {file}:{}: {}", issue.file_line, issue.message);
    }
    eprintln!("{} problem(s), {} error(s)", issues.len(), errors);
}

fn main() {
    stackmon::init();

    let args: Vec<String> = std::env::args().collect();

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
            eprintln!("Usage: uniform build --input <DIR> --output <FILE.ttf|.woff2> [--output ...]");
            std::process::exit(1);
        };
        if output_files.is_empty() {
            eprintln!("Usage: uniform build --input <DIR> --output <FILE.ttf|.woff2> [--output ...]");
            std::process::exit(1);
        }

        let docs = load_docs_reporting_errors(&input);
        if docs.is_empty() {
            eprintln!("No .unf files found in {}", input.display());
            std::process::exit(1);
        }
        let refs: Vec<&document::Document> = docs.iter().collect();
        report_issues(&refs);
        let Some(font_bytes) = render::build_font_from_documents(&refs) else {
            eprintln!("Font build failed");
            std::process::exit(1);
        };

        let mut woff2_bytes: Option<Vec<u8>> = None;
        for output in &output_files {
            let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("ttf");
            let data = match ext {
                "woff2" => {
                    if woff2_bytes.is_none() {
                        match render::ttf_to_woff2(&font_bytes) {
                            Ok(b) => woff2_bytes = Some(b),
                            Err(e) => {
                                eprintln!("{e}");
                                std::process::exit(1);
                            }
                        }
                    }
                    woff2_bytes.as_ref().unwrap().as_slice()
                }
                _ => &font_bytes,
            };
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
            let result = if let Some(ref w2) = woff2_bytes {
                render::sample::write_live_html_woff2(&mut f, &refs, &font_bytes, w2, data_dir.as_deref())
            } else {
                render::sample::write_live_html(&mut f, &refs, &font_bytes, data_dir.as_deref())
            };
            if let Err(e) = result {
                eprintln!("Failed to write live HTML: {e}");
                std::process::exit(1);
            }
            eprintln!("Wrote {}", path.display());
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

        let docs = load_docs_reporting_errors(&input);
        if docs.is_empty() {
            eprintln!("No .unf files found in {}", input.display());
            std::process::exit(1);
        }
        let refs: Vec<&document::Document> = docs.iter().collect();
        report_issues(&refs);

        let Some(built) = render::build_font_with_gid_map(&refs) else {
            eprintln!("Font build failed");
            std::process::exit(1);
        };

        let name_parts = document::collect_name_parts(&refs);
        let (resolved, _) = ref_composite::resolve_named_glyphs_with_parts(&refs, &name_parts);

        let shape_result = render::assert::run_assertions(&refs, &built.ttf, &built.gid_to_name, built.height);
        let sd_result = render::assert::run_same_distinct_assertions(&refs, &resolved);

        let total = shape_result.total + sd_result.total;
        let passed = shape_result.passed + sd_result.passed;

        if total == 0 {
            eprintln!("No assertions found.");
            return;
        }

        for issue in shape_result.issues.iter().chain(sd_result.issues.iter()) {
            let file_name = issue.file.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            eprintln!("FAIL {}:{}: {}", file_name, issue.file_line, issue.message);
        }

        let failed = total - passed;
        eprintln!(
            "\n{} assertion(s): {} passed, {} failed.",
            total, passed, failed,
        );

        if failed > 0 {
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
                .with_title("Uniform"),
            ..Default::default()
        };

        eframe::run_native(
            "uniform",
            options,
            Box::new(move |cc| Ok(Box::new(app::UniformApp::new(cc, font_dir)))),
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
