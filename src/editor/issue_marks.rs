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
//!
//! # Following edits
//!
//! A report is about the buffer the build read, and it lands seconds later —
//! after the reader may have opened or deleted lines above the one it names.
//! Drawn at its reported index, a finding would then tint whatever line had
//! slid under it. So an issue is located through the buffer's
//! [`LineId`]s instead: the id the *snapshot* had at the reported index, looked
//! up in the buffer as it is now ([`crate::document::line_id`] says why the id
//! travels with the line). A line whose id is gone was deleted, or rebuilt by
//! an edit that did not continue it, and its findings are hidden rather than
//! pinned to a stranger.
//!
//! This is all it has to be. The next build replaces the report within
//! seconds; what matters in between is that the highlights react to an edit
//! the way the text does. The Issues tab reads the same positions
//! ([`IssueMarks::position`]), so the list and the tint never disagree about
//! where a finding is, or whether it is still there.

use crate::hash::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::document::{Document, LineId};
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

/// One file's lines, keyed by `DocLine` index — the index the editor addresses
/// its lines by, in the buffer as it is now.
pub(crate) type LineIssues = HashMap<usize, LineIssue>;

/// The line ids of every document a report was computed from, by path. Taken
/// beside the report (the documents are cloned for the build anyway, and an
/// `Arc` is what each of them holds).
pub(crate) type LineIdSnapshot = HashMap<PathBuf, Arc<[LineId]>>;

pub(crate) fn snapshot_line_ids<'a>(
    docs: impl IntoIterator<Item = &'a Document>,
) -> LineIdSnapshot {
    docs.into_iter()
        .map(|doc| (doc.path.clone(), Arc::clone(&doc.line_ids)))
        .collect()
}

const SEVERITIES: usize = Severity::ALL.len();

/// Where a report's issues are in the buffers as they are now, and what that
/// makes of each file's lines.
///
/// The issues themselves stay with their owner; an issue is named here by its
/// index across every report handed to [`IssueMarks::new`], which is the order
/// the Issues tab lists them in.
#[derive(Default)]
pub(crate) struct IssueMarks {
    files: HashMap<PathBuf, FileMarks>,
    /// Every issue's `(doc_line, 1-based file_line)` now, by index; `None` for
    /// one whose line is gone.
    at: Vec<Option<(usize, usize)>>,
    /// Handed out for a file nothing was reported about, so that a caller
    /// takes the same `&LineIssues` either way and no `Option` reaches the
    /// paint loop.
    empty: LineIssues,
}

#[derive(Default)]
struct FileMarks {
    /// One run per report that named this file, in report order.
    runs: Vec<Run>,
    /// What `lines` were last computed for. `None` until the first
    /// [`IssueMarks::follow`].
    seen: Option<Seen>,
    lines: LineIssues,
}

struct Seen {
    /// The buffer's line ids, or `None` for a file no document was found for.
    buffer: Option<Arc<[LineId]>>,
    shown: [bool; SEVERITIES],
}

struct Run {
    report: usize,
    /// The line ids of the buffer the report read, if the report's snapshot
    /// had this file at all.
    snapshot: Option<Arc<[LineId]>>,
    issues: Vec<usize>,
}

impl IssueMarks {
    /// Groups the reports' issues by file, each report with the snapshot it
    /// was computed from. Positions start out as reported; [`Self::follow`]
    /// brings them up to date.
    pub(crate) fn new<'a>(
        reports: impl IntoIterator<Item = (&'a [Issue], &'a LineIdSnapshot)>,
    ) -> Self {
        let mut files: HashMap<PathBuf, FileMarks> = HashMap::default();
        let mut at = Vec::new();
        for (report_idx, (report, snapshot)) in reports.into_iter().enumerate() {
            for issue in report {
                let idx = at.len();
                at.push(Some((issue.line, issue.file_line)));
                let file = match files.get_mut(&issue.file) {
                    Some(file) => file,
                    None => files.entry(issue.file.clone()).or_default(),
                };
                if file.runs.last().is_none_or(|run| run.report != report_idx) {
                    file.runs.push(Run {
                        report: report_idx,
                        snapshot: snapshot.get(&issue.file).cloned(),
                        issues: Vec::new(),
                    });
                }
                file.runs.last_mut().unwrap().issues.push(idx);
            }
        }
        Self {
            files,
            at,
            empty: LineIssues::default(),
        }
    }

    /// Relocates every file's issues into the buffer `current` returns for it,
    /// and re-reduces its lines keeping only the severities `shown` accepts.
    ///
    /// Meant to run every frame: a file whose buffer is the same `Arc` of ids
    /// as last time, under the same filter, costs one comparison. `issue`
    /// hands back an issue by its index. Returns whether any file's lines
    /// were recomputed.
    pub(crate) fn follow<'i, 'd>(
        &mut self,
        issue: impl Fn(usize) -> &'i Issue,
        current: impl Fn(&Path) -> Option<&'d Document>,
        shown: impl Fn(Severity) -> bool,
    ) -> bool {
        let mut changed = false;
        let shown: [bool; SEVERITIES] = std::array::from_fn(|i| shown(Severity::ALL[i]));
        for (path, file) in &mut self.files {
            let doc = current(path);
            let ids = doc.map(|doc| &doc.line_ids);
            let moved = match &file.seen {
                None => true,
                Some(seen) => match (&seen.buffer, ids) {
                    (Some(seen), Some(ids)) => !Arc::ptr_eq(seen, ids),
                    (None, None) => false,
                    _ => true,
                },
            };
            if !moved && file.seen.as_ref().is_some_and(|seen| seen.shown == shown) {
                continue;
            }
            if moved {
                file.relocate(doc, &issue, &mut self.at);
            }
            file.lines = file.reduce(&issue, &self.at, &shown);
            file.seen = Some(Seen {
                buffer: ids.cloned(),
                shown,
            });
            changed = true;
        }
        changed
    }

    pub(crate) fn for_file(&self, path: &Path) -> &LineIssues {
        self.files.get(path).map_or(&self.empty, |file| &file.lines)
    }

    /// Where issue `idx` is now, as `(doc_line, 1-based file_line)`; `None`
    /// once its line is gone.
    pub(crate) fn position(&self, idx: usize) -> Option<(usize, usize)> {
        self.at.get(idx).copied().flatten()
    }
}

impl FileMarks {
    fn relocate<'i>(
        &self,
        doc: Option<&Document>,
        issue: &impl Fn(usize) -> &'i Issue,
        at: &mut [Option<(usize, usize)>],
    ) {
        // Built on the first issue that needs it, and only once per file.
        let mut index: Option<HashMap<LineId, usize>> = None;
        for run in &self.runs {
            for &idx in &run.issues {
                let reported = issue(idx);
                at[idx] = match (&run.snapshot, doc) {
                    // Nothing to map through: a file the report's snapshot did
                    // not hold (it failed to parse), or one no longer loaded.
                    (None, _) | (_, None) => Some((reported.line, reported.file_line)),
                    (Some(snapshot), Some(doc)) if Arc::ptr_eq(snapshot, &doc.line_ids) => {
                        Some((reported.line, reported.file_line))
                    }
                    (Some(snapshot), Some(doc)) => match snapshot.get(reported.line) {
                        None => Some((reported.line, reported.file_line)),
                        Some(id) => {
                            let index = index.get_or_insert_with(|| {
                                let mut index = HashMap::with_capacity_and_hasher(
                                    doc.line_ids.len(),
                                    Default::default(),
                                );
                                // A line cloned beside its original shares its
                                // id; the first of them is the one kept.
                                for (line, id) in doc.line_ids.iter().enumerate() {
                                    index.entry(*id).or_insert(line);
                                }
                                index
                            });
                            index
                                .get(id)
                                .map(|&line| (line, doc.docline_file_line(line)))
                        }
                    },
                };
            }
        }
    }

    /// Reduces this file's issues to one entry per line. Order matters: the
    /// first finding of the worst severity on a line is the one kept, so this
    /// walks the runs in report order.
    fn reduce<'i>(
        &self,
        issue: &impl Fn(usize) -> &'i Issue,
        at: &[Option<(usize, usize)>],
        shown: &[bool; SEVERITIES],
    ) -> LineIssues {
        let mut lines = LineIssues::default();
        for &idx in self.runs.iter().flat_map(|run| &run.issues) {
            let issue = issue(idx);
            let visible = Severity::ALL
                .iter()
                .position(|s| *s == issue.severity)
                .is_some_and(|i| shown[i]);
            let Some((line, _)) = at[idx].filter(|_| visible) else {
                continue;
            };
            match lines.get_mut(&line) {
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
                        line,
                        LineIssue {
                            severity: issue.severity,
                            message: issue.message.clone(),
                            extra: 0,
                        },
                    );
                }
            }
        }
        lines
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

    /// The reduction alone: every issue where it was reported.
    fn collect(issues: &[Issue], shown: impl Fn(Severity) -> bool) -> IssueMarks {
        let snapshot = LineIdSnapshot::default();
        let mut marks = IssueMarks::new([(issues, &snapshot)]);
        marks.follow(|i| &issues[i], |_| None, shown);
        marks
    }

    #[test]
    fn one_finding_is_shown_whole() {
        let issues = [issue(Severity::Warning, 3, "sits in the `-l` slot")];
        let marks = collect(&issues, |_| true);
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
            let issues: Vec<Issue> = order.iter().map(|i| all[*i].clone()).collect();
            let marks = collect(&issues, |_| true);
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
        let marks = collect(&issues, |_| true);
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
        let marks = collect(&issues, |s| s != Severity::Todo);
        let lines = marks.for_file(Path::new("a.unf"));
        assert!(!lines.contains_key(&1));
        assert_eq!(lines[&2].display(), "error");
    }

    #[test]
    fn a_file_nothing_was_said_about_has_no_lines() {
        let marks = collect(&[], |_| true);
        assert!(marks.for_file(Path::new("b.unf")).is_empty());
    }

    // -----------------------------------------------------------------------
    // Following a buffer edited after the report
    // -----------------------------------------------------------------------

    use crate::document::DocLine;

    fn derive(lines: &[DocLine]) -> Document {
        crate::document_io::derive_document(lines, PathBuf::from("a.unf"))
            .unwrap()
            .0
    }

    /// A report on `source` naming `lines`, and the snapshot it read.
    fn report(source: &str, at: &[usize]) -> (Vec<DocLine>, Vec<Issue>, LineIdSnapshot) {
        let lines = crate::document_io::parse_doclines(source);
        let doc = derive(&lines);
        let issues = at
            .iter()
            .map(|&line| Issue {
                file_line: doc.docline_file_line(line),
                ..issue(Severity::Error, line, &format!("on {line}"))
            })
            .collect();
        (lines, issues, snapshot_line_ids([&doc]))
    }

    fn follow(marks: &mut IssueMarks, issues: &[Issue], doc: &Document) -> bool {
        marks.follow(|i| &issues[i], |_| Some(doc), |_| true)
    }

    /// Lines opened above a finding move it down, lines deleted above move it
    /// up, and a deleted reported line takes its finding out of both the
    /// highlights and the positions the Issues tab reads.
    #[test]
    fn positions_follow_insertions_and_deletions() {
        let (mut lines, issues, snapshot) = report("a\nb\nc\nd\n", &[1, 3]);
        let mut marks = IssueMarks::new([(&issues[..], &snapshot)]);

        lines.insert(0, DocLine::text("new"));
        lines.insert(0, DocLine::text("new"));
        follow(&mut marks, &issues, &derive(&lines));
        assert_eq!(marks.position(0), Some((3, 4)));
        assert_eq!(marks.position(1), Some((5, 6)));

        lines.remove(3); // "b"
        follow(&mut marks, &issues, &derive(&lines));
        assert_eq!(marks.position(0), None);
        assert_eq!(marks.position(1), Some((4, 5)));
        let lines_now: Vec<usize> = marks.for_file(Path::new("a.unf")).keys().copied().collect();
        assert_eq!(lines_now, vec![4]);
    }

    /// The file line is the buffer's own: a grid above a finding is as many
    /// file lines as it has rows, and growing one moves the number, not the
    /// `DocLine` index.
    #[test]
    fn the_file_line_is_read_from_the_buffer_now() {
        let (mut lines, issues, snapshot) = report("glyph a 2 1\n@@@@\nb\n", &[2]);
        let mut marks = IssueMarks::new([(&issues[..], &snapshot)]);
        assert_eq!(marks.position(0), Some((2, 3)));

        let DocLine::Grid(grid) = &mut lines[1] else {
            panic!()
        };
        grid.resize(2, 3);
        lines[0] = DocLine::text("glyph a 2 3");
        follow(&mut marks, &issues, &derive(&lines));
        assert_eq!(marks.position(0), Some((2, 5)));
    }

    /// Two reports read two snapshots: each is located through its own, and
    /// the tab's order — the first report's, then the second's — decides a
    /// tie on one line.
    #[test]
    fn each_report_is_located_through_its_own_snapshot() {
        let (mut lines, build, build_snapshot) = report("a\nb\n", &[1]);
        lines.insert(0, DocLine::text("new"));
        let later = derive(&lines);
        // The assertion run read the buffer after the insertion.
        let asserts = vec![Issue {
            file_line: 3,
            ..issue(Severity::Error, 2, "assert")
        }];
        let assert_snapshot = snapshot_line_ids([&later]);
        let mut marks = IssueMarks::new([
            (&build[..], &build_snapshot),
            (&asserts[..], &assert_snapshot),
        ]);
        let all: Vec<Issue> = build.iter().chain(&asserts).cloned().collect();

        lines.insert(0, DocLine::text("newer"));
        follow(&mut marks, &all, &derive(&lines));
        assert_eq!(marks.position(0), Some((3, 4)));
        assert_eq!(marks.position(1), Some((3, 4)));
        assert_eq!(
            marks.for_file(Path::new("a.unf"))[&3].display(),
            "on 1  [+1]"
        );
    }

    /// Nothing moved, nothing recomputed; the filter alone re-reduces without
    /// relocating.
    #[test]
    fn an_unchanged_buffer_costs_nothing() {
        let (lines, issues, snapshot) = report("a\nb\n", &[0]);
        let doc = derive(&lines);
        let mut marks = IssueMarks::new([(&issues[..], &snapshot)]);
        assert!(follow(&mut marks, &issues, &doc));
        assert!(!follow(&mut marks, &issues, &doc));
        assert!(marks.follow(|i| &issues[i], |_| Some(&doc), |_| false));
        assert!(marks.for_file(Path::new("a.unf")).is_empty());
        assert_eq!(marks.position(0), Some((0, 1)), "hidden is not gone");
    }
}
