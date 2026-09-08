//! Which document line each reported issue is on, for the editor's line
//! highlights.
//!
//! The Issues tab is a list: it says what is wrong, and clicking a row carries
//! the reader to the line. This is the other direction — the line says it,
//! where the reader already is. A line something was reported about is tinted
//! in its severity's color across the pane, and the message is drawn after the
//! text in the same color, darker.
//!
//! Two reductions happen here, both because a line is one row of pixels and a
//! line's findings are not one finding:
//!
//! - **Worst wins.** A `glyph` line can be an error *and* a chore at once, and
//!   what the tint has to say first is the error. [`Severity`] is ordered
//!   worst-first, so the shown finding is the `min`, and ties go to whichever
//!   check reported first — the report's own order, which is the order the
//!   Issues tab lists them in.
//! - **The rest are counted.** Only the worst message is drawn; the others
//!   become a `[+N]` after it. The count is the point: it says the line has
//!   more to it without pretending a row can hold four messages.
//!
//! The set is built from the *filtered* issue list, not from every finding.
//! The severity filter above the Issues tab is already the reader's control
//! over which findings are theirs today, and a font whose todo queue is tens
//! of thousands long is a font where highlighting every todo tints most of
//! every file. So one control governs both surfaces: hide todos in the tab and
//! the todo lines stop being tinted, which is what a reader hiding them meant.

use crate::hash::HashMap;
use std::path::{Path, PathBuf};

use crate::issues::{Issue, Severity};

/// What one line's findings reduce to: the worst message, and how many more
/// there are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LineIssue {
    pub(crate) severity: Severity,
    pub(crate) message: String,
    /// Findings on this line other than the one above. Drawn as `[+N]`.
    pub(crate) extra: usize,
}

impl LineIssue {
    /// The message as it is drawn: the worst finding, plus what it stands in
    /// for.
    pub(crate) fn display(&self) -> String {
        if self.extra == 0 {
            self.message.clone()
        } else {
            format!("{}  [+{}]", self.message, self.extra)
        }
    }
}

/// One file's lines, keyed by `DocLine` index — the same 0-based index
/// [`Issue::line`] carries and the editor addresses its lines by.
pub(crate) type LineIssues = HashMap<usize, LineIssue>;

/// Every open file's [`LineIssues`], keyed by the path the issue named.
#[derive(Default)]
pub(crate) struct IssueMarks {
    by_file: HashMap<PathBuf, LineIssues>,
    /// Handed out for a file nothing was reported about, so that a caller
    /// takes the same `&LineIssues` either way and no `Option` reaches the
    /// paint loop.
    empty: LineIssues,
}

impl IssueMarks {
    /// Reduces `issues` to one entry per line, keeping only the severities
    /// `shown` accepts. Order matters: the first finding of the worst severity
    /// on a line is the one kept, so this wants the report's own order.
    pub(crate) fn collect<'a>(
        issues: impl IntoIterator<Item = &'a Issue>,
        shown: impl Fn(Severity) -> bool,
    ) -> Self {
        let mut by_file: HashMap<PathBuf, LineIssues> = HashMap::default();
        for issue in issues {
            if !shown(issue.severity) {
                continue;
            }
            let lines = by_file.entry(issue.file.clone()).or_default();
            match lines.get_mut(&issue.line) {
                Some(cur) => {
                    cur.extra += 1;
                    // Worst-first ordering: a lower `Severity` is the graver
                    // one, so this takes over the message and keeps the count.
                    if issue.severity < cur.severity {
                        cur.severity = issue.severity;
                        cur.message = issue.message.clone();
                    }
                }
                None => {
                    lines.insert(
                        issue.line,
                        LineIssue {
                            severity: issue.severity,
                            message: issue.message.clone(),
                            extra: 0,
                        },
                    );
                }
            }
        }
        Self {
            by_file,
            empty: LineIssues::default(),
        }
    }

    pub(crate) fn for_file(&self, path: &Path) -> &LineIssues {
        self.by_file.get(path).unwrap_or(&self.empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(severity: Severity, line: usize, message: &str) -> Issue {
        Issue {
            severity,
            glyph: None,
            message: message.to_string(),
            file: PathBuf::from("a.unf"),
            line,
            file_line: line + 1,
        }
    }

    #[test]
    fn one_finding_is_shown_whole() {
        let issues = [issue(Severity::Warning, 3, "sits in the `-l` slot")];
        let marks = IssueMarks::collect(&issues, |_| true);
        let line = &marks.for_file(Path::new("a.unf"))[&3];
        assert_eq!(line.severity, Severity::Warning);
        assert_eq!(line.extra, 0);
        assert_eq!(line.display(), "sits in the `-l` slot");
    }

    /// The graver finding is the one drawn, whichever order it was reported
    /// in, and the rest are the count after it.
    #[test]
    fn the_worst_severity_on_a_line_wins_and_the_rest_are_counted() {
        for order in [[0, 1, 2], [2, 1, 0], [1, 0, 2]] {
            let all = [
                issue(Severity::Chore, 7, "chore"),
                issue(Severity::Error, 7, "error"),
                issue(Severity::Todo, 7, "todo"),
            ];
            let issues: Vec<&Issue> = order.iter().map(|i| &all[*i]).collect();
            let marks = IssueMarks::collect(issues, |_| true);
            let line = &marks.for_file(Path::new("a.unf"))[&7];
            assert_eq!(line.severity, Severity::Error, "{order:?}");
            assert_eq!(line.display(), "error  [+2]", "{order:?}");
        }
    }

    /// Ties keep the first, which is the order the Issues tab lists.
    #[test]
    fn equal_severities_keep_the_first_message() {
        let issues = [
            issue(Severity::Error, 0, "first"),
            issue(Severity::Error, 0, "second"),
        ];
        let marks = IssueMarks::collect(&issues, |_| true);
        assert_eq!(
            marks.for_file(Path::new("a.unf"))[&0].display(),
            "first  [+1]"
        );
    }

    /// A filtered-out severity is not counted either: a line whose only
    /// finding is hidden is not a highlighted line with an empty message.
    #[test]
    fn hidden_severities_leave_no_trace() {
        let issues = [
            issue(Severity::Todo, 1, "todo"),
            issue(Severity::Error, 2, "error"),
            issue(Severity::Todo, 2, "todo"),
        ];
        let marks = IssueMarks::collect(&issues, |s| s != Severity::Todo);
        let lines = marks.for_file(Path::new("a.unf"));
        assert!(!lines.contains_key(&1));
        assert_eq!(lines[&2].display(), "error");
    }

    #[test]
    fn a_file_nothing_was_said_about_has_no_lines() {
        let marks = IssueMarks::collect(&[], |_| true);
        assert!(marks.for_file(Path::new("b.unf")).is_empty());
    }
}
