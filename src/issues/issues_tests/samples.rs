//! `sample` and its `||` continuations

use super::*;

fn sample_errors(input: &str) -> Vec<String> {
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    collect_issues(&[&doc])
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.message)
        .collect()
}

/// The one thing a `sample` line cannot do without. Without the check it is a
/// text that silently never appears anywhere.
#[test]
fn a_sample_with_no_continuation_is_an_error() {
    let msgs = sample_errors("sample Latin pangram\n");
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(msgs[0].contains("at least one `||` line"), "{}", msgs[0]);
}

#[test]
fn a_well_formed_sample_says_nothing() {
    assert!(
        sample_errors("sample Latin pangram\n|| The quick brown fox.\nsample Latin\n|| head\n")
            .is_empty()
    );
}

/// The second of two texts of one name is unreachable — the list shows one
/// entry per name — so it is named rather than quietly dropped.
#[test]
fn a_duplicate_sample_name_is_an_error() {
    let msgs = sample_errors("sample L a\n|| one\nsample L a\n|| two\n");
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(msgs[0].contains("more than once"), "{}", msgs[0]);

    let msgs = sample_errors("sample L\n|| one\nsample L\n|| two\n");
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(msgs[0].contains("give this one a sublabel"), "{}", msgs[0]);
}

/// A mode is a word off a list, so one that is not on it is a mistake and not
/// a no-op — the text would otherwise be offered read the wrong way round.
#[test]
fn a_mode_that_names_nothing_is_an_error() {
    let msgs = sample_errors("sample L a : vertical\n|| text\n");
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(
        msgs[0].contains("unknown sample mode `vertical`"),
        "{}",
        msgs[0]
    );
    assert!(msgs[0].contains("`matrix`"), "{}", msgs[0]);

    assert!(
        sample_errors("sample L a : matrix\n|| ab\n|| cd\n").is_empty(),
        "a mode on the list is not a complaint"
    );
}

/// A generated mode writes its own text, so the two rules about text are the
/// other way round there: nothing under it is right, and a `||` line under it
/// is text nothing will ever show.
#[test]
fn a_generated_sample_takes_no_continuation() {
    assert!(
        sample_errors("sample `UDHR Article 1` : udhr-article1\n").is_empty(),
        "a generated sample is written with no text"
    );
    assert!(
        sample_errors("sample F `Subdivisions` : subdivision-flags\n").is_empty(),
        "and under a sublabel just the same"
    );
    let msgs = sample_errors("sample U : udhr-article1\n|| mine\n");
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(msgs[0].contains("writes its own text"), "{}", msgs[0]);
}

/// `udhr-article1` stands for a list and names the entries itself, so a
/// sublabel — written on the line or beside it — is a name that is never read.
#[test]
fn a_list_writing_mode_owns_its_labels() {
    let msgs = sample_errors("sample U one : udhr-article1\n");
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(msgs[0].contains("write it with no sublabel"), "{}", msgs[0]);

    let msgs = sample_errors("sample U : udhr-article1\nsample U mine\n|| text\n");
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(
        msgs[0].contains("writes its own list of texts"),
        "{}",
        msgs[0]
    );
}

/// A `||` with nothing above it to continue is reported as what it is, rather
/// than as an unrecognized directive that leaves the author rereading a line
/// that is spelled perfectly well.
#[test]
fn an_orphan_continuation_names_itself() {
    let msgs = sample_errors("meta family Foo\n|| stranded\n");
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(
        msgs[0].contains("continues the command above it"),
        "{}",
        msgs[0]
    );
}

/// `goto` says which one `ref` a jump to the enclosing glyph carries on to,
/// so a second one is not a refinement but a question with no answer. The
/// navigation takes the first and says so here.
#[test]
fn two_goto_refs_on_one_glyph_are_a_warning() {
    let issues = issues_for(
        "glyph a 1 1\n@@\n\nglyph b 1 1\n@@\n\nglyph c\nref a 0 0 goto\nref b 0 0 goto\n\nmap C = c\n",
    );
    assert!(has(&issues, Severity::Warning, "`goto`"), "{issues:?}");
}

#[test]
fn a_single_goto_ref_is_quiet() {
    let issues = issues_for("glyph a 1 1\n@@\n\nglyph c\nref a 0 0 goto\n\nmap C = c\n");
    assert!(
        !issues.iter().any(|i| i.message.contains("goto")),
        "{issues:?}"
    );
}
