//! The Search pane's other two lists: the **issues**, and the **unsaved
//! changes**.
//!
//! The two buttons at the right end of the pane's header row swap the list
//! under it for one of these, and Ctrl/Cmd+G then walks whichever list is
//! showing ([`SearchSource`]). Clicking the lit button again brings the search
//! results back, and so does running a search. The changes list is what
//! reviewing an edit that touched many places at once needs — a clearance
//! optimization, above all, which leaves its rewrites unsaved for exactly that
//! review ([`super::fix`]).
//!
//! **File order, always.** Both lists are in the order the files sort in and,
//! within a file, in line order. The Issues tab's list is in report order, which
//! puts the errors first; this one walks the font the way a reader goes over
//! it, and a severity is only a filter — the Issues tab's own, shared, so the
//! two lists and the editors' highlights always agree on what is shown.
//!
//! **A step goes on from the caret**, unless the caret is still on the entry
//! the last step landed on. Both lists are re-derived whenever they are drawn
//! or stepped — an edit relocates the issues, a build replaces them, every
//! keystroke changes the changes — so an index into the last list says
//! nothing about the next one. What is remembered instead is the entry's
//! address, [`ListStop`], and it is used only while the caret has not left it:
//! once it has, Ctrl/Cmd+G means "the next one after here", as a next-error
//! command does in any editor.
//!
//! **A change is a hunk** ([`change_marks::ChangeHunk`]): the changes close
//! enough together to be on screen at once are one stop, so a glyph whose
//! header and IDC line both moved is reviewed once. Only an open document can
//! have unsaved changes, so the list never looks past them. The hunks are kept
//! per document and rebuilt only when that document's marks are — see
//! [`UniformApp::refresh_change_list`].

use std::path::Path;

use super::panels::{IssueFilter, severity_color, severity_icon, show_issue_filter};
use super::*;
use crate::editor::change_marks::{self, ChangeHunk, ChangeMarks};
use crate::issues::Severity;

/// Which list the Search pane shows, and so what Ctrl/Cmd+G walks.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum SearchSource {
    /// The last search's results; see [`super::search`].
    #[default]
    Search,
    Issues,
    Changes,
}

impl SearchSource {
    /// The two the header row offers, left to right.
    pub(super) const LISTS: [SearchSource; 2] = [SearchSource::Issues, SearchSource::Changes];

    pub(super) fn label(self) -> &'static str {
        match self {
            SearchSource::Search => "Search",
            SearchSource::Issues => "Issues",
            SearchSource::Changes => "Changes",
        }
    }
}

/// One entry of a list by what it is about: the file, the buffer line, and
/// which of the entries on that line — two issues can share one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ListStop {
    pub path: PathBuf,
    pub line: usize,
    pub rank: usize,
}

/// The order both lists are in, and what the caret is compared by. By the
/// path's bytes rather than its components: this runs every frame the list is
/// drawn (see `UniformApp::refresh_issue_marks` for the same choice).
fn order_key(path: &Path, line: usize) -> (&std::ffi::OsStr, usize) {
    (path.as_os_str(), line)
}

/// The address of entry `i` of `keys`.
pub(super) fn stop_at(keys: &[(&Path, usize)], i: usize) -> ListStop {
    let (path, line) = keys[i];
    let rank = keys[..i]
        .iter()
        .rev()
        .take_while(|&&k| k == keys[i])
        .count();
    ListStop {
        path: path.to_path_buf(),
        line,
        rank,
    }
}

/// Where `stop` is in `keys`, if it is still there.
pub(super) fn find_stop(keys: &[(&Path, usize)], stop: &ListStop) -> Option<usize> {
    let at = order_key(&stop.path, stop.line);
    let i = keys.partition_point(|&(p, l)| order_key(p, l) < at) + stop.rank;
    keys.get(i)
        .is_some_and(|&(p, l)| p == stop.path && l == stop.line)
        .then_some(i)
}

/// Which entry of `keys` a step lands on: the neighbour of `current` while the
/// caret is still on it, else the first entry past the caret (or the last one
/// before it), wrapping at both ends. See the module note.
pub(super) fn step_index(
    keys: &[(&Path, usize)],
    current: Option<usize>,
    caret: Option<(&Path, usize)>,
    forward: bool,
) -> Option<usize> {
    let n = keys.len();
    if n == 0 {
        return None;
    }
    if let Some(i) = current
        && caret.is_some_and(|c| c == keys[i])
    {
        return Some(if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        });
    }
    let Some((path, line)) = caret else {
        return Some(if forward { 0 } else { n - 1 });
    };
    let at = order_key(path, line);
    Some(if forward {
        let i = keys.partition_point(|&(p, l)| order_key(p, l) <= at);
        if i == n { 0 } else { i }
    } else {
        match keys.partition_point(|&(p, l)| order_key(p, l) < at) {
            0 => n - 1,
            i => i - 1,
        }
    })
}

/// The issues `filter` shows, in file order. The sort is stable, so issues on
/// one line keep the order they were reported in.
pub(super) fn file_ordered_issues(
    located: Vec<(&Issue, usize, usize)>,
    filter: IssueFilter,
) -> Vec<(&Issue, usize, usize)> {
    let mut shown: Vec<_> = located
        .into_iter()
        .filter(|(issue, ..)| filter.shows(issue.severity))
        .collect();
    shown.sort_by(|a, b| order_key(&a.0.file, a.1).cmp(&order_key(&b.0.file, b.1)));
    shown
}

pub(super) fn issue_keys<'a>(rows: &[(&'a Issue, usize, usize)]) -> Vec<(&'a Path, usize)> {
    rows.iter()
        .map(|(issue, line, _)| (issue.file.as_path(), *line))
        .collect()
}

/// One row of the changes list.
pub(super) struct ChangeRow {
    pub hunk: ChangeHunk,
    /// 1-based, for display only.
    pub file_line: usize,
    /// The first changed text line, trimmed, or what stands for one when the
    /// hunk changed no text.
    pub text: String,
    /// The header of the item the hunk is in — `glyph han-4e00` — when that is
    /// not the line shown already: an IDC line on its own does not say whose
    /// it is.
    pub context: Option<String>,
}

/// One open document's rows, and the marks they were made from.
pub(super) struct DocChanges {
    pub path: PathBuf,
    marks: Arc<ChangeMarks>,
    pub rows: Vec<ChangeRow>,
}

fn change_rows(doc: &Document, lines: &[DocLine], marks: &ChangeMarks) -> Vec<ChangeRow> {
    change_marks::hunks(marks, lines, change_marks::HUNK_GAP)
        .into_iter()
        .map(|hunk| {
            let changed = (hunk.line..hunk.end.min(lines.len()))
                .find(|&i| marks.line(i) != change_marks::LineChange::Unchanged);
            let text = match changed.map(|i| &lines[i]) {
                Some(DocLine::Text(text)) => text.trim().to_string(),
                Some(DocLine::Grid(_)) => "(pixels)".to_string(),
                None => "(lines deleted)".to_string(),
            };
            let shown = changed.unwrap_or(hunk.line);
            // The document's item table may be a rederive behind the buffer;
            // this is a label, and a stale one is still near enough.
            let item = doc.item_line_starts.partition_point(|&s| s <= shown);
            let context = item
                .checked_sub(1)
                .map(|i| doc.item_line_starts[i])
                .filter(|&start| start != shown)
                .and_then(|start| lines.get(start)?.as_text())
                .map(|header| {
                    let words: Vec<&str> = header.split_whitespace().take(2).collect();
                    words.join(" ")
                })
                .filter(|header| !header.is_empty());
            ChangeRow {
                file_line: doc.docline_file_line(hunk.line),
                hunk,
                text,
                context,
            }
        })
        .collect()
}

pub(super) fn change_keys(changes: &[DocChanges]) -> Vec<(&Path, usize)> {
    changes
        .iter()
        .flat_map(|doc| {
            doc.rows
                .iter()
                .map(|row| (doc.path.as_path(), row.hunk.line))
        })
        .collect()
}

pub(super) fn change_count(changes: &[DocChanges]) -> usize {
    changes.iter().map(|doc| doc.rows.len()).sum()
}

/// A lit list button's `n/m`, or an unlit one's plain count.
pub(super) fn list_button_label(
    source: SearchSource,
    count: usize,
    current: Option<usize>,
) -> String {
    match current {
        Some(i) => format!("\u{23F7} {} {}/{count}", source.label(), i + 1),
        None => format!("\u{23F7} {} {count}", source.label()),
    }
}

/// A clickable list row tinted when it is the current entry, as the search
/// results' rows are. Returns whether it was clicked.
fn list_row(
    ui: &mut egui::Ui,
    id: egui::Id,
    current: bool,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    let mut frame = egui::Frame::NONE;
    if current {
        frame = frame.fill(ui.visuals().selection.bg_fill.gamma_multiply(0.25));
    }
    let resp = frame.show(ui, |ui| {
        ui.horizontal(add_contents);
    });
    let click = ui.interact(resp.response.rect, id, egui::Sense::click());
    if click.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    click.clicked()
}

fn location_label(ui: &mut egui::Ui, path: &Path, file_line: usize, context: Option<&str>) {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let location = match context {
        Some(context) => format!("{context} \u{00B7} {file_name}:{file_line}"),
        None => format!("{file_name}:{file_line}"),
    };
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(
            egui::RichText::new(location)
                .size(16.0)
                .color(ui.visuals().weak_text_color()),
        );
    });
}

/// The issues, in file order, under the Issues tab's own severity filter.
pub(super) fn show_issue_list(
    ui: &mut egui::Ui,
    rows: &[(&Issue, usize, usize)],
    counts: [usize; Severity::ALL.len()],
    filter: &mut IssueFilter,
    current: Option<&ListStop>,
    click: &mut Option<ListStop>,
) {
    show_issue_filter(ui, filter, counts);
    ui.separator();
    if rows.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(if counts.iter().all(|&n| n == 0) {
                "No issues"
            } else {
                "Nothing to show: every severity with an issue in it is filtered out"
            });
        });
        return;
    }
    let keys = issue_keys(rows);
    let current = current.and_then(|stop| find_stop(&keys, stop));
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, &(issue, _, file_line)) in rows.iter().enumerate() {
            let id = ui.id().with(("issue_list_row", i));
            if list_row(ui, id, current == Some(i), |ui| {
                ui.label(
                    egui::RichText::new(severity_icon(issue.severity))
                        .color(severity_color(ui, issue.severity))
                        .size(16.0),
                );
                ui.label(egui::RichText::new(&issue.message).size(16.0));
                location_label(ui, &issue.file, file_line, None);
            }) {
                *click = Some(stop_at(&keys, i));
            }
        }
    });
}

/// The unsaved changes, one row a hunk, each led by a bar in the gutter's own
/// colour for what it mostly did.
pub(super) fn show_change_list(
    ui: &mut egui::Ui,
    changes: &[DocChanges],
    current: Option<&ListStop>,
    click: &mut Option<ListStop>,
) {
    if changes.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label("No unsaved changes");
        });
        return;
    }
    let keys = change_keys(changes);
    let current = current.and_then(|stop| find_stop(&keys, stop));
    let pal = crate::editor::colors::Palette::get(ui);
    egui::ScrollArea::vertical().show(ui, |ui| {
        let rows = changes
            .iter()
            .flat_map(|doc| doc.rows.iter().map(move |row| (doc, row)));
        for (i, (doc, row)) in rows.enumerate() {
            let hunk = &row.hunk;
            let color = if hunk.modified > 0 {
                pal.change_modified
            } else if hunk.added > 0 {
                pal.change_added
            } else {
                pal.change_deleted
            };
            let id = ui.id().with(("change_list_row", i));
            if list_row(ui, id, current == Some(i), |ui| {
                ui.label(egui::RichText::new("\u{2503}").color(color).size(16.0));
                ui.label(egui::RichText::new(&row.text).size(16.0));
                location_label(ui, &doc.path, row.file_line, row.context.as_deref());
            }) {
                *click = Some(stop_at(&keys, i));
            }
        }
    });
}

impl UniformApp {
    /// Brings the changes list up to date with the open documents: a
    /// comparison per document on a frame where none of them changed, the
    /// marks being cached on the buffer's revision.
    pub(super) fn refresh_change_list(&mut self) {
        let mut order: Vec<usize> = (0..self.open_documents.len()).collect();
        order.sort_by(|&a, &b| {
            let path = |i: usize| self.open_documents[i].document.path.as_os_str();
            path(a).cmp(path(b))
        });
        let mut old: Vec<Option<DocChanges>> = std::mem::take(&mut self.search.changes)
            .into_iter()
            .map(Some)
            .collect();
        for i in order {
            let doc = &mut self.open_documents[i];
            let marks = doc.editor_state.change_marks(&doc.document, &doc.lines);
            if marks.is_clean() {
                continue;
            }
            let reused = old
                .iter_mut()
                .find(|c| {
                    c.as_ref().is_some_and(|c| {
                        Arc::ptr_eq(&c.marks, &marks) && c.path == doc.document.path
                    })
                })
                .and_then(Option::take);
            let entry = reused.unwrap_or_else(|| DocChanges {
                path: doc.document.path.clone(),
                rows: change_rows(&doc.document, &doc.lines, &marks),
                marks,
            });
            self.search.changes.push(entry);
        }
    }

    /// Where the caret is, for a step to go on from.
    fn caret_list_pos(&self) -> Option<(&Path, usize)> {
        let doc = self.active_doc()?;
        Some((doc.document.path.as_path(), doc.editor_state.cursor_line()))
    }

    /// Ctrl/Cmd+G and Ctrl/Cmd+Shift+G while the pane shows the issues or the
    /// changes.
    pub(super) fn step_list(&mut self, ctx: &egui::Context, source: SearchSource, forward: bool) {
        let target = match source {
            SearchSource::Search => return,
            SearchSource::Issues => {
                let rows = file_ordered_issues(
                    Self::located_issues(&self.issues, &self.assert_issues, &self.issue_marks),
                    self.issue_filter,
                );
                let keys = issue_keys(&rows);
                let current = self
                    .search
                    .issues_current
                    .as_ref()
                    .and_then(|s| find_stop(&keys, s));
                step_index(&keys, current, self.caret_list_pos(), forward)
                    .map(|i| stop_at(&keys, i))
            }
            SearchSource::Changes => {
                self.refresh_change_list();
                let keys = change_keys(&self.search.changes);
                let current = self
                    .search
                    .changes_current
                    .as_ref()
                    .and_then(|s| find_stop(&keys, s));
                step_index(&keys, current, self.caret_list_pos(), forward)
                    .map(|i| stop_at(&keys, i))
            }
        };
        match target {
            Some(stop) => self.goto_list_stop(ctx, source, stop),
            None => {
                self.search.message = Some(match source {
                    SearchSource::Changes => "No unsaved changes".to_string(),
                    _ => "No issues to go to".to_string(),
                });
            }
        }
    }

    /// Opens the file a list entry is in and puts the caret at the start of
    /// its line, recording the jump as a search hit's is.
    pub(super) fn goto_list_stop(
        &mut self,
        ctx: &egui::Context,
        source: SearchSource,
        stop: ListStop,
    ) {
        self.search.message = None;
        let from = self.caret_nav_loc();
        self.open_file(stop.path.clone());
        let Some(idx) = self
            .open_documents
            .iter()
            .position(|d| d.document.path == stop.path)
        else {
            return;
        };
        self.panes.show_document(idx);
        let doc = &mut self.open_documents[idx];
        doc.editor_state.goto_caret(&doc.lines, stop.line, 0);
        let line = doc.editor_state.cursor_line();
        if let Some(from) = from {
            self.nav_history.push(NavEntry {
                from,
                to: NavLoc::new(idx, line, 0),
            });
        }
        match source {
            SearchSource::Issues => self.search.issues_current = Some(stop),
            SearchSource::Changes => self.search.changes_current = Some(stop),
            SearchSource::Search => {}
        }
        self.focus_pane_editor(ctx);
    }
}
