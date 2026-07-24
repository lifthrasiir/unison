#[cfg(feature = "editor")]
mod app;
mod migrate;
mod detail;
mod document;
mod document_io;
#[cfg(feature = "editor")]
mod edit_menu;
mod pixel;
mod ref_composite;
#[cfg(feature = "editor")]
mod preview;
mod render;
#[cfg(feature = "editor")]
mod editor;
mod issues;
#[cfg(feature = "editor")]
mod sidebar;
#[cfg(feature = "editor")]
mod specimen;

#[cfg(target_os = "windows")]
extern crate windows_core;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Check for migrate subcommand: uniform migrate --input DIR --output DIR
    if args.get(1).map(|s| s.as_str()) == Some("migrate") {
        let mut input_dir = None;
        let mut output_dir = None;
        let mut i = 2;
        while i < args.len() {
            match args[i].as_str() {
                "--input" | "-i" => {
                    i += 1;
                    input_dir = args.get(i).map(std::path::PathBuf::from);
                }
                "--output" | "-o" => {
                    i += 1;
                    output_dir = args.get(i).map(std::path::PathBuf::from);
                }
                _ => {
                    eprintln!("Unknown migrate option: {}", args[i]);
                    std::process::exit(1);
                }
            }
            i += 1;
        }

        let Some(input) = input_dir else {
            eprintln!("Usage: uniform migrate --input <DIR> --output <DIR>");
            std::process::exit(1);
        };
        let Some(output) = output_dir else {
            eprintln!("Usage: uniform migrate --input <DIR> --output <DIR>");
            std::process::exit(1);
        };

        match migrate::migrate_directory(&input, &output) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Migration failed: {e:#}");
                std::process::exit(1);
            }
        }
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
            eprintln!("Usage: uniform build --input <DIR> --output <FILE.ttf|.woff2> [--output ...]");
            std::process::exit(1);
        };
        if output_files.is_empty() {
            eprintln!("Usage: uniform build --input <DIR> --output <FILE.ttf|.woff2> [--output ...]");
            std::process::exit(1);
        }

        let docs = render::load_docs_from_directory(&input);
        if docs.is_empty() {
            eprintln!("No .unf files found in {}", input.display());
            std::process::exit(1);
        }
        let refs: Vec<&document::Document> = docs.iter().collect();
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

        let docs = render::load_docs_from_directory(&input);
        if docs.is_empty() {
            eprintln!("No .unf files found in {}", input.display());
            std::process::exit(1);
        }
        let refs: Vec<&document::Document> = docs.iter().collect();

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
        eprintln!("Usage: uniform <build|migrate> [options...]");
        eprintln!("GUI mode requires the 'editor' feature.");
        std::process::exit(1);
    }
}
