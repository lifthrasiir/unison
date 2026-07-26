//! Golden snapshots over the `testdata/` project: the diagnostics validation
//! reports, and a digest of what resolution produces.
//!
//! The resolution/validation code was consolidated (issues.rs, ttf_builder and
//! ref_composite each grew their own copy of name expansion and reference
//! collection). Behaviour-preserving changes there are verified by these
//! snapshots; changes that *intentionally* surface previously-swallowed
//! problems update them, so the diff is reviewable.
//!
//! `font/` is deliberately not a test target — cases it turned up are
//! extracted into `testdata/` instead.
//!
//! Regenerate with:
//!
//! ```sh
//! UNIFORM_UPDATE_GOLDEN=1 cargo test golden
//! ```

use std::path::{Path, PathBuf};

use crate::document::Document;
use crate::issues::Issue;

fn testdata_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata")
}

fn golden_path(name: &str) -> PathBuf {
    testdata_dir().join(name)
}

/// One line per issue, fully sorted so the output does not depend on document
/// or item traversal order. `file_line` is deliberately included: a refactor
/// that loses provenance would otherwise pass unnoticed.
fn format_issues(issues: &[Issue]) -> String {
    let mut lines: Vec<String> = issues
        .iter()
        .map(|i| {
            let sev = match i.severity {
                crate::issues::Severity::Error => "error",
                crate::issues::Severity::Warning => "warning",
            };
            let file = i
                .file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("{sev}\t{file}\t{}\t{}", i.file_line, i.message)
        })
        .collect();
    lines.sort();
    lines.push(String::new());
    lines.join("\n")
}

/// Compare `actual` against the stored golden file, or rewrite it when
/// `UNIFORM_UPDATE_GOLDEN=1`. Returns a human-readable diff summary on
/// mismatch.
fn check_golden(name: &str, actual: &str) -> Result<(), String> {
    let path = golden_path(name);
    if std::env::var("UNIFORM_UPDATE_GOLDEN").as_deref() == Ok("1") {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, actual).map_err(|e| e.to_string())?;
        return Ok(());
    }

    let expected = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "cannot read golden {}: {e}\n\
             run `UNIFORM_UPDATE_GOLDEN=1 cargo test golden` to create it",
            path.display(),
        )
    })?;

    if expected == actual {
        return Ok(());
    }

    let exp: Vec<&str> = expected.lines().collect();
    let act: Vec<&str> = actual.lines().collect();
    let missing: Vec<&&str> = exp.iter().filter(|l| !act.contains(l)).collect();
    let added: Vec<&&str> = act.iter().filter(|l| !exp.contains(l)).collect();

    let mut msg = format!(
        "golden {} mismatch: {} expected line(s), {} actual line(s)\n",
        path.display(),
        exp.len(),
        act.len(),
    );
    for l in missing.iter().take(40) {
        msg.push_str(&format!("- {l}\n"));
    }
    if missing.len() > 40 {
        msg.push_str(&format!("  ... and {} more removed\n", missing.len() - 40));
    }
    for l in added.iter().take(40) {
        msg.push_str(&format!("+ {l}\n"));
    }
    if added.len() > 40 {
        msg.push_str(&format!("  ... and {} more added\n", added.len() - 40));
    }
    msg.push_str(
        "\nif this change is intended, re-run with UNIFORM_UPDATE_GOLDEN=1 and review the diff\n",
    );
    Err(msg)
}

fn load_docs(dir: &Path) -> Vec<Document> {
    let (docs, errors) = crate::render::ttf_builder::load_docs_from_directory_checked(dir);
    assert!(errors.is_empty(), "fixtures must parse: {errors:?}");
    assert!(!docs.is_empty(), "no .unf files found in {}", dir.display());
    docs
}

/// A digest of every resolved glyph: name, grid dimensions, a hash of the
/// pixel bytes, scale and anchor names. Consolidating `ref_composite`'s own
/// expansion into the shared one must not change what the editor draws, and
/// only a snapshot at this granularity can show that.
fn format_resolved(
    resolved: &std::collections::HashMap<String, crate::ref_composite::ResolvedGlyph>,
) -> String {
    use std::hash::{Hash, Hasher};

    let mut lines: Vec<String> = resolved
        .iter()
        .map(|(name, g)| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for p in &g.grid.pixels {
                p.0.hash(&mut h);
            }
            g.grid.den.hash(&mut h);
            format!("{:?}", g.grid.details).hash(&mut h);
            let mut anchors: Vec<&str> =
                g.declared_anchors.iter().map(|p| p.position.as_str()).collect();
            anchors.sort();
            format!(
                "{name}\t{}x{}\ts{}\t{:016x}\t{}",
                g.grid.width,
                g.grid.height,
                g.scale,
                h.finish(),
                anchors.join(","),
            )
        })
        .collect();
    lines.sort();
    lines.push(String::new());
    lines.join("\n")
}

#[test]
fn resolved_glyphs_over_testdata_match_golden() {
    let docs = load_docs(&testdata_dir());
    let refs: Vec<&Document> = docs.iter().collect();
    let name_parts = crate::document::collect_name_parts(&refs);
    let (resolved, _) =
        crate::ref_composite::resolve_named_glyphs_with_parts(&refs, &name_parts);
    if let Err(e) = check_golden("testdata-resolved.golden", &format_resolved(&resolved)) {
        panic!("{e}");
    }
}

/// `testdata/` is a small font project deliberately broken in every way we
/// know how to report, so this catches a consolidation that silently *drops*
/// a check.
#[test]
fn issues_over_testdata_match_golden() {
    let docs = load_docs(&testdata_dir());
    let refs: Vec<&Document> = docs.iter().collect();
    let issues = crate::issues::collect_issues(&refs);
    if let Err(e) = check_golden("testdata-issues.golden", &format_issues(&issues)) {
        panic!("{e}");
    }
}



