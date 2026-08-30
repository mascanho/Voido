//! Application state and Vim-style key handling.

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{self, Receiver};

use chrono::{DateTime, Local, NaiveDate};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use tui_textarea::{CursorMove, TextArea};

use crate::config::{Config, StorageChoice};
use crate::github::{GitHubClient, RepoInfo, SyncClient};
use crate::model::{Attachment, Milestone, Note, Priority, Project, Store, Subtask, Todo};
use crate::util::truncate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Projects,
    /// The middle pane: the todo list, notes, timeline or overview, per `Tab`.
    Content,
    /// The right pane: subtasks of the selected todo (Todos tab) or subnotes of
    /// the selected note (Notes tab).
    Detail,
}

/// Screen rects of the three body panes, refreshed every render so mouse clicks
/// can be routed to the right pane. A zero-sized rect means "not shown".
#[derive(Debug, Clone, Copy, Default)]
pub struct PaneRects {
    pub projects: Rect,
    pub content: Rect,
    pub detail: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Todos,
    Notes,
    Schedule,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Overview, Tab::Todos, Tab::Notes, Tab::Schedule];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Todos => "Todos",
            Tab::Notes => "Notes",
            Tab::Schedule => "Schedule",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlKind {
    Todo,
    Milestone,
}

/// A row in the merged, date-sorted timeline view.
#[derive(Clone)]
#[allow(dead_code)]
pub struct TimelineEntry {
    pub date: NaiveDate,
    pub label: String,
    pub kind: TlKind,
    pub done: bool,
    /// Index into the project's `milestones` vec, when this row is a milestone.
    pub milestone_idx: Option<usize>,
    /// Index into the project's `todos` vec, when this row is a todo.
    pub todo_idx: Option<usize>,
    pub project_idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineFilter {
    All,
    Overdue,
    Today,
    ThisWeek,
}

impl DeadlineFilter {
    /// Cycle All -> Overdue -> Today -> This week -> All.
    fn next(self) -> Self {
        match self {
            DeadlineFilter::All => DeadlineFilter::Overdue,
            DeadlineFilter::Overdue => DeadlineFilter::Today,
            DeadlineFilter::Today => DeadlineFilter::ThisWeek,
            DeadlineFilter::ThisWeek => DeadlineFilter::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DeadlineFilter::All => "all",
            DeadlineFilter::Overdue => "overdue",
            DeadlineFilter::Today => "today",
            DeadlineFilter::ThisWeek => "this week",
        }
    }
}

pub enum InputAction {
    AddProject,
    RenameProject,
    EditDescription,
    AddTodo,
    EditTodo(usize),
    AddSubtask,
    EditSubtask(usize),
    AddNote,
    EditNote(usize),
    /// Add an attachment to this target; returns to the manager afterwards.
    AddAttachment(AttachTarget),
    /// Add one or more (space-separated) tags to this target.
    AddTag(TagTarget),
    AddMilestone,
    EditMilestone(usize),
    RescheduleTodo(usize),
    RescheduleMilestone(usize),
    LinkRepo,
    /// GitHub data-sync setup: repo, then token.
    SyncRepo,
    SyncToken,
}

pub struct InputState {
    pub title: String,
    pub editor: TextArea<'static>,
    pub action: InputAction,
}

impl InputState {
    /// The current single-line contents of the input.
    pub fn value(&self) -> String {
        self.editor.lines().first().cloned().unwrap_or_default()
    }
}

#[allow(clippy::enum_variant_names)]
pub enum ConfirmAction {
    DeleteProject(usize),
    DeleteTodo(usize),
    DeleteSubtask(usize),
    DeleteNote(usize),
    DeleteMilestone(usize),
}

pub struct ConfirmState {
    pub prompt: String,
    pub action: ConfirmAction,
}

/// What a Markdown editor session is writing to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    /// The body of the project note at this index.
    NoteBody(usize),
    /// The note attached to the todo at this index.
    TodoNote(usize),
    /// The note attached to the current todo's subtask at this index.
    SubtaskNote(usize),
}

/// Which note the Todos-tab detail pane is rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailNote {
    /// The selected todo's own note (fills the detail pane).
    Todo,
    /// The note of the todo's subtask at this index (section below the list).
    Subtask(usize),
}

/// Markdown editor for a note body or a todo's attached note.
pub struct EditState {
    pub target: EditTarget,
    pub textarea: TextArea<'static>,
}

/// What a set of attachments hangs off: a todo, or one of its subtasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachTarget {
    pub todo_idx: usize,
    /// `Some(i)` when the manager is for subtask `i` of that todo.
    pub sub_idx: Option<usize>,
}

/// The attachment manager overlay.
pub struct AttachState {
    pub target: AttachTarget,
    pub sel: usize,
}

/// What a set of tags hangs off. Indices are captured when the manager opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagTarget {
    Project,
    Todo(usize),
    Subtask { todo: usize, sub: usize },
}

/// The tag manager overlay.
pub struct TagState {
    pub target: TagTarget,
    pub sel: usize,
}

/// The fuzzy-finder overlay.
pub struct SearchState {
    pub editor: TextArea<'static>,
    pub sel: usize,
}

impl SearchState {
    pub fn query(&self) -> String {
        self.editor.lines().first().cloned().unwrap_or_default()
    }
}

/// Where a fuzzy-search hit points.
#[derive(Debug, Clone, Copy)]
pub enum SearchTarget {
    Project,
    Todo(usize),
    Subtask { todo: usize, sub: usize },
    Note(usize),
}

impl SearchTarget {
    /// Nesting depth: project 0, todo/note 1, subtask 2.
    pub fn depth(self) -> usize {
        match self {
            SearchTarget::Project => 0,
            SearchTarget::Todo(_) | SearchTarget::Note(_) => 1,
            SearchTarget::Subtask { .. } => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub project_idx: usize,
    pub target: SearchTarget,
    pub label: String,
    /// Ancestor names, outermost first — `[]` for a project, `[project]` for a
    /// todo/note, `[project, todo]` for a subtask.
    pub crumbs: Vec<String>,
}

/// Theme picker. Moving the cursor previews the theme live; `esc` restores the
/// one that was active when the picker opened.
pub struct ThemeState {
    /// Index into `theme::registry()` — highlighted and previewed.
    pub idx: usize,
    /// Index to fall back to on cancel.
    pub saved: usize,
}

pub enum Mode {
    Normal,
    Input(Box<InputState>),
    Confirm(ConfirmState),
    EditBody(Box<EditState>),
    Help,
    GitHub,
    Theme(ThemeState),
    /// A dismissible message popup — (title, body). Used for sync results.
    Notice(String, String),
    /// Attachment manager for the current todo.
    Attach(AttachState),
    /// Tag manager for the current project / todo / subtask.
    Tags(TagState),
    /// Global fuzzy finder.
    Search(Box<SearchState>),
}

pub struct App {
    pub store: Store,
    pub focus: Focus,
    pub tab: Tab,
    pub project_idx: usize,
    pub todo_idx: usize,
    pub subtask_idx: usize,
    pub note_idx: usize,
    pub note_scroll: u16,
    pub note_expanded: bool,
    /// `n` toggles the selected todo's note into the detail pane; `n` inside the
    /// Subtasks pane toggles the selected subtask's note into a section below the
    /// list. Independent, each with its own `^d` / `^u` scroll offset.
    pub todo_note_open: bool,
    pub sub_note_open: bool,
    pub todo_note_scroll: u16,
    pub sub_note_scroll: u16,
    /// `i` toggles an inline detail panel (tags, dates, counts…) under the
    /// selected row — a separate flag per pane so expanding todos doesn't also
    /// expand the projects rail.
    pub project_info: bool,
    pub todo_info: bool,
    pub subtask_info: bool,
    /// `m` toggles a stripped-back layout (no hint bar, minimal header).
    pub minimal: bool,
    pub timeline_idx: usize,
    pub deadline_filter: DeadlineFilter,
    pub mode: Mode,
    pub status: String,
    pub should_quit: bool,
    pub dirty: bool,
    /// Set by `^e`; `main`'s loop drops out of the TUI to run `$EDITOR` on the
    /// settings file, then reloads it.
    pub open_settings: bool,
    pending_g: bool,
    pub gh_client: GitHubClient,
    pub gh_cache: Option<RepoInfo>,
    pub gh_loading: bool,
    /// Receiver for an in-flight background GitHub fetch, if any.
    gh_rx: Option<Receiver<Result<RepoInfo, String>>>,
    /// Persisted settings. `github_repo` is written back (as `owner/repo`) after
    /// a successful setup; `main` also persists on exit.
    pub config: Config,
    /// Token resolved at startup (config value, `gh` CLI, or environment).
    pub sync_token: Option<String>,
    /// Blob SHA of the last successfully pulled/pushed sync file.
    pub sync_sha: Option<String>,
    pub sync_in_flight: bool,
    /// Data edits made since the last successful sync this session — drives the
    /// footer's "N unsynced" hint. Zeroed on a successful push.
    pub sync_pending: usize,
    /// When the last successful sync completed this session.
    pub last_sync: Option<DateTime<Local>>,
    /// Repo name captured from the first `^y` prompt, pending a token step.
    pending_sync_repo: Option<String>,
    sync_rx: Option<Receiver<Result<SyncOk, String>>>,
    /// Memoized Markdown render of the note body currently on screen,
    /// keyed by (content hash, pane width).
    md_cache: RefCell<Option<(u64, u16, Vec<Line<'static>>)>>,
    /// Body-pane rects from the last render, for routing mouse clicks.
    pub pane_rects: RefCell<PaneRects>,
}

/// The outcome of a successful sync push: the resolved `owner/repo` and the new
/// blob SHA.
pub struct SyncOk {
    repo: String,
    sha: String,
}

impl App {
    pub fn new(
        mut store: Store,
        config: Config,
        sync_sha: Option<String>,
        sync_token: Option<String>,
    ) -> Self {
        // Tidy whatever we loaded (local DB, legacy import or GitHub sync):
        // enforce "todo with subtasks is done iff they all are", and normalise
        // any hand-edited tags.
        for p in &mut store.projects {
            p.tags = crate::model::normalize_tags(p.tags.drain(..));
            for t in &mut p.todos {
                t.recompute_done();
                t.tags = crate::model::normalize_tags(t.tags.drain(..));
                for s in &mut t.subtasks {
                    s.tags = crate::model::normalize_tags(s.tags.drain(..));
                }
            }
        }
        let gh_client = GitHubClient::new(sync_token.clone());
        Self {
            store,
            focus: Focus::Projects,
            tab: Tab::Todos,
            project_idx: 0,
            todo_idx: 0,
            subtask_idx: 0,
            note_idx: 0,
            note_scroll: 0,
            todo_note_open: false,
            sub_note_open: false,
            todo_note_scroll: 0,
            sub_note_scroll: 0,
            note_expanded: false,
            project_info: false,
            todo_info: false,
            subtask_info: false,
            minimal: false,
            timeline_idx: 0,
            deadline_filter: DeadlineFilter::All,
            mode: Mode::Normal,
            status: String::new(),
            should_quit: false,
            dirty: false,
            open_settings: false,
            pending_g: false,
            gh_client,
            gh_cache: None,
            gh_loading: false,
            gh_rx: None,
            config,
            sync_token,
            sync_sha,
            sync_in_flight: false,
            sync_pending: 0,
            last_sync: None,
            pending_sync_repo: None,
            sync_rx: None,
            md_cache: RefCell::new(None),
            pane_rects: RefCell::new(PaneRects::default()),
        }
    }

    /// GitHub sync is on, has a repo, and we have a token to use.
    pub fn sync_ready(&self) -> bool {
        self.config.sync_configured() && self.sync_token.is_some()
    }

    /// Adopt a settings file re-read from disk (after the user edited it via
    /// `^e`). Re-resolves the token and rebuilds the GitHub client; a changed
    /// repo/file means the next push re-establishes the blob SHA.
    pub fn reload_config(&mut self, config: Config) {
        let repo_changed = config.github_repo != self.config.github_repo
            || config.github_file != self.config.github_file;
        self.config = config;
        self.sync_token = crate::github::resolve_token(self.config.github_token.as_deref());
        self.gh_client = GitHubClient::new(self.sync_token.clone());
        if repo_changed {
            self.sync_sha = None;
        }
    }

    /// Record that the local store diverged from what's on GitHub. Called once
    /// per edit cycle (after the local DB save), so the footer can show how many
    /// edits are waiting for the next `^y` / exit push.
    pub fn note_unsynced_edit(&mut self) {
        if self.sync_ready() {
            self.sync_pending = self.sync_pending.saturating_add(1);
        }
    }

    /// Markdown-render a note body, reusing the last result when the content and
    /// pane width are unchanged (avoids re-parsing on every scroll keypress).
    pub fn note_body_lines(&self, body: &str, width: u16) -> Vec<Line<'static>> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        body.hash(&mut hasher);
        let key = hasher.finish();

        let mut cache = self.md_cache.borrow_mut();
        if let Some((k, w, lines)) = cache.as_ref()
            && *k == key
            && *w == width
        {
            return lines.clone();
        }
        let lines = crate::md::render(body, width);
        *cache = Some((key, width, lines.clone()));
        lines
    }

    /// Poll in-flight background work (GitHub activity fetches and data syncs).
    /// Returns `true` when app state changed and the UI should redraw.
    pub fn poll_background(&mut self) -> bool {
        let mut changed = false;

        if let Some(rx) = &self.gh_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.gh_rx = None;
                    self.gh_loading = false;
                    match result {
                        Ok(info) => {
                            self.gh_cache = Some(info);
                            if matches!(self.mode, Mode::Normal) {
                                self.mode = Mode::GitHub;
                            }
                            self.status = "github data loaded".into();
                        }
                        Err(e) => self.status = format!("github error: {e}"),
                    }
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.gh_rx = None;
                    self.gh_loading = false;
                    self.status = "github fetch failed".into();
                    changed = true;
                }
            }
        }

        if let Some(rx) = &self.sync_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.sync_rx = None;
                    self.sync_in_flight = false;
                    match result {
                        Ok(ok) => {
                            self.sync_sha = Some(ok.sha);
                            self.sync_pending = 0;
                            self.last_sync = Some(Local::now());
                            if self.config.github_repo.as_deref() != Some(ok.repo.as_str()) {
                                self.config.github_repo = Some(ok.repo);
                                let _ = self.config.save();
                            }
                            self.status = "synced with GitHub".into();
                        }
                        Err(e) => {
                            self.status = "sync failed".into();
                            let repo = self.config.github_repo.clone().unwrap_or_default();
                            self.mode = Mode::Notice(
                                "GitHub sync failed".into(),
                                format!("{repo}\n\n{e}\n\nPress ^y to re-run the setup."),
                            );
                        }
                    }
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.sync_rx = None;
                    self.sync_in_flight = false;
                    self.status = "sync failed".into();
                    changed = true;
                }
            }
        }

        changed
    }

    /// `^y`: push now if sync is ready, otherwise start setup. When a token is
    /// already available (config / `gh` / env) the only prompt is the repo.
    fn sync_action(&mut self) {
        if self.sync_ready() {
            self.spawn_sync(false);
            return;
        }
        let pre = self
            .config
            .github_repo
            .clone()
            .unwrap_or_else(|| "voido-data".into());
        let title = if self.sync_token.is_some() {
            "GitHub sync — repo  (name, or owner/repo; Enter = voido-data)"
        } else {
            "GitHub sync — repo  (owner/repo)"
        };
        self.begin_input(title, pre, InputAction::SyncRepo);
    }

    /// Kick off setup with the repo + token now in hand: save config, then push
    /// on a worker thread (creating the repo if needed).
    fn finish_sync_setup(&mut self) {
        let Some(repo) = self.pending_sync_repo.take() else {
            return;
        };
        if self.sync_token.is_none() {
            self.status = "sync setup needs a token".into();
            return;
        }
        self.config.storage = StorageChoice::GitHub;
        self.config.github_repo = Some(repo);
        let _ = self.config.save();
        self.status = "setting up GitHub sync…".into();
        self.spawn_sync(true);
    }

    /// Serialize the store and push it on a worker thread. With `setup`, first
    /// resolve a bare repo name to `owner/repo` and create the repo if missing.
    fn spawn_sync(&mut self, setup: bool) {
        if self.sync_in_flight {
            return;
        }
        let (Some(repo), Some(token)) = (self.config.github_repo.clone(), self.sync_token.clone())
        else {
            self.status = "GitHub sync is not configured — press ^y".into();
            return;
        };
        let json = match serde_json::to_string_pretty(&self.store) {
            Ok(j) => j,
            Err(e) => {
                self.status = format!("sync: {e}");
                return;
            }
        };
        let sha = self.sync_sha.clone();
        let file = self.config.sync_file();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_sync(&token, &repo, &file, &json, sha.as_deref(), setup));
        });
        self.sync_rx = Some(rx);
        self.sync_in_flight = true;
        self.status = "syncing with GitHub…".into();
    }

    // ---- accessors -------------------------------------------------------

    pub fn current_project(&self) -> Option<&Project> {
        self.store.projects.get(self.project_idx)
    }

    pub fn current_project_mut(&mut self) -> Option<&mut Project> {
        self.store.projects.get_mut(self.project_idx)
    }

    pub fn current_todo(&self) -> Option<&Todo> {
        self.current_project()?.todos.get(self.todo_idx)
    }

    fn current_todo_mut(&mut self) -> Option<&mut Todo> {
        let i = self.todo_idx;
        self.current_project_mut()?.todos.get_mut(i)
    }

    /// After a subtask edit, pull the parent todo's `done` back in line with its
    /// subtasks (all done -> todo done; any open -> todo open).
    fn sync_parent_done(&mut self) {
        if let Some(t) = self.current_todo_mut()
            && t.recompute_done()
        {
            self.dirty = true;
        }
    }

    pub fn current_note(&self) -> Option<&Note> {
        self.current_project()?.notes.get(self.note_idx)
    }

    /// `n` at todo level toggled the selected todo's note into the detail pane
    /// (3rd pane), and it has a non-empty note to show.
    pub fn showing_todo_note(&self) -> bool {
        self.tab == Tab::Todos
            && self.focus != Focus::Detail
            && self.todo_note_open
            && self
                .current_todo()
                .is_some_and(|t| !t.note.trim().is_empty())
    }

    /// `n` inside the Subtasks pane toggled the selected subtask's note into a
    /// section below the subtask list (4th pane), and it has one to show.
    pub fn showing_sub_note(&self) -> bool {
        self.tab == Tab::Todos
            && self.focus == Focus::Detail
            && self.sub_note_open
            && self
                .current_todo()
                .and_then(|t| t.subtasks.get(self.subtask_idx))
                .is_some_and(|s| !s.note.trim().is_empty())
    }

    /// `n` on a todo: flip its note in/out of the detail pane.
    fn toggle_todo_note(&mut self) {
        match self.current_todo() {
            Some(t) if !t.note.trim().is_empty() => {
                self.todo_note_open = !self.todo_note_open;
                self.todo_note_scroll = 0;
            }
            Some(_) => self.status = "no note yet — press N to write one".into(),
            None => {}
        }
    }

    /// `n` on a subtask: flip its note in/out of the pane below the list.
    fn toggle_sub_note(&mut self) {
        match self
            .current_todo()
            .and_then(|t| t.subtasks.get(self.subtask_idx))
        {
            Some(s) if !s.note.trim().is_empty() => {
                self.sub_note_open = !self.sub_note_open;
                self.sub_note_scroll = 0;
            }
            Some(_) => self.status = "no note yet — press N to write one".into(),
            None => {}
        }
    }

    /// Scroll whichever detail note is currently on screen.
    fn scroll_detail_note(&mut self, delta: i32) {
        let target = if self.showing_sub_note() {
            &mut self.sub_note_scroll
        } else {
            &mut self.todo_note_scroll
        };
        *target = if delta < 0 {
            target.saturating_sub((-delta) as u16)
        } else {
            target.saturating_add(delta as u16)
        };
    }

    /// Whether the right-hand detail pane has something to show right now.
    fn detail_available(&self) -> bool {
        match self.tab {
            Tab::Todos => self.current_todo().is_some(),
            Tab::Notes => self.current_note().is_some(),
            _ => false,
        }
    }

    /// Todos with a due date + milestones, sorted by date.
    pub fn timeline(&self) -> Vec<TimelineEntry> {
        let mut rows = Vec::new();
        let today = Local::now().date_naive();
        if let Some(p) = self.current_project() {
            for (ti, t) in p.todos.iter().enumerate() {
                if let Some(d) = t.due {
                    let matches = match self.deadline_filter {
                        DeadlineFilter::All => true,
                        DeadlineFilter::Overdue => !t.done && d < today,
                        DeadlineFilter::Today => d == today,
                        DeadlineFilter::ThisWeek => {
                            let diff = (d - today).num_days();
                            (0..=7).contains(&diff)
                        }
                    };
                    if matches {
                        rows.push(TimelineEntry {
                            date: d,
                            label: t.title.clone(),
                            kind: TlKind::Todo,
                            done: t.done,
                            milestone_idx: None,
                            todo_idx: Some(ti),
                            project_idx: self.project_idx,
                        });
                    }
                }
            }
            for (mi, m) in p.milestones.iter().enumerate() {
                let matches = match self.deadline_filter {
                    DeadlineFilter::All => true,
                    DeadlineFilter::Overdue => !m.done && m.date < today,
                    DeadlineFilter::Today => m.date == today,
                    DeadlineFilter::ThisWeek => {
                        let diff = (m.date - today).num_days();
                        (0..=7).contains(&diff)
                    }
                };
                if matches {
                    rows.push(TimelineEntry {
                        date: m.date,
                        label: m.title.clone(),
                        kind: TlKind::Milestone,
                        done: m.done,
                        milestone_idx: Some(mi),
                        todo_idx: None,
                        project_idx: self.project_idx,
                    });
                }
            }
        }
        rows.sort_by(|a, b| a.date.cmp(&b.date));
        rows
    }

    fn selected_milestone_idx(&self) -> Option<usize> {
        self.timeline()
            .get(self.timeline_idx)
            .and_then(|e| e.milestone_idx)
    }

    /// Returns (total, done, overdue, today, this_week) for the current project's deadlines.
    pub fn deadline_stats(&self) -> (usize, usize, usize, usize, usize) {
        let today = Local::now().date_naive();
        let mut total = 0usize;
        let mut done = 0usize;
        let mut overdue = 0usize;
        let mut today_count = 0usize;
        let mut this_week = 0usize;
        if let Some(p) = self.current_project() {
            for t in &p.todos {
                if let Some(d) = t.due {
                    total += 1;
                    if t.done {
                        done += 1;
                    } else if d < today {
                        overdue += 1;
                    }
                    if d == today {
                        today_count += 1;
                    }
                    let diff = (d - today).num_days();
                    if (0..=7).contains(&diff) {
                        this_week += 1;
                    }
                }
            }
            for m in &p.milestones {
                total += 1;
                if m.done {
                    done += 1;
                } else if m.date < today {
                    overdue += 1;
                }
                if m.date == today {
                    today_count += 1;
                }
                let diff = (m.date - today).num_days();
                if (0..=7).contains(&diff) {
                    this_week += 1;
                }
            }
        }
        (total, done, overdue, today_count, this_week)
    }

    /// The currently highlighted deadline entry, if any.
    pub fn current_deadline(&self) -> Option<TimelineEntry> {
        self.timeline().get(self.timeline_idx).cloned()
    }

    /// Keep every selection index inside bounds. Called once per frame.
    pub fn clamp(&mut self) {
        if self.store.projects.is_empty() {
            self.project_idx = 0;
            self.focus = Focus::Projects;
        } else if self.project_idx >= self.store.projects.len() {
            self.project_idx = self.store.projects.len() - 1;
        }
        let todos = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
        self.todo_idx = self.todo_idx.min(todos.saturating_sub(1));
        let subs = self.current_todo().map(|t| t.subtasks.len()).unwrap_or(0);
        self.subtask_idx = self.subtask_idx.min(subs.saturating_sub(1));
        let notes = self.current_project().map(|p| p.notes.len()).unwrap_or(0);
        self.note_idx = self.note_idx.min(notes.saturating_sub(1));
        let body_lines = self
            .current_note()
            .map(|n| n.body.lines().count() as u16 + 8)
            .unwrap_or(0);
        self.note_scroll = self.note_scroll.min(body_lines.saturating_sub(1));
        let todo_note_lines = self
            .current_todo()
            .map(|t| t.note.lines().count() as u16)
            .unwrap_or(0);
        self.todo_note_scroll = self.todo_note_scroll.min(todo_note_lines.saturating_add(4));
        let sub_note_lines = self
            .current_todo()
            .and_then(|t| t.subtasks.get(self.subtask_idx))
            .map(|s| s.note.lines().count() as u16)
            .unwrap_or(0);
        self.sub_note_scroll = self.sub_note_scroll.min(sub_note_lines.saturating_add(4));
        let tl = self.timeline().len();
        self.timeline_idx = self.timeline_idx.min(tl.saturating_sub(1));

        if self.focus == Focus::Detail && !self.detail_available() {
            self.focus = Focus::Content;
        }
    }

    // ---- top level dispatch --------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Normal => self.handle_normal(key),
            Mode::Input(_) => self.handle_input(key),
            Mode::Confirm(_) => self.handle_confirm(key),
            Mode::EditBody(_) => self.handle_edit_body(key),
            Mode::Theme(_) => self.handle_theme(key),
            Mode::Attach(_) => self.handle_attach(key),
            Mode::Tags(_) => self.handle_tags(key),
            Mode::Search(_) => self.handle_search(key),
            Mode::Help | Mode::GitHub | Mode::Notice(..) => self.mode = Mode::Normal,
        }
    }

    /// Route a mouse event. Left-click focuses the pane under the pointer (and
    /// selects the row it landed on); the wheel scrolls the focused pane.
    /// Overlays own the screen, so mouse input is ignored while one is open.
    /// Returns `true` when something changed and the UI should redraw.
    pub fn handle_mouse(&mut self, me: MouseEvent) -> bool {
        if !matches!(self.mode, Mode::Normal) {
            return false;
        }
        let pos = Position::new(me.column, me.row);
        match me.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click_pane(pos),
            MouseEventKind::ScrollDown => {
                self.move_sel(3);
                true
            }
            MouseEventKind::ScrollUp => {
                self.move_sel(-3);
                true
            }
            _ => false,
        }
    }

    fn click_pane(&mut self, pos: Position) -> bool {
        let r = *self.pane_rects.borrow();

        // The tab strip lives on the content pane's top border row.
        if r.content.area() > 0
            && pos.y == r.content.y
            && let Some(t) = tab_at_x(r.content, pos.x)
        {
            self.goto_tab(t);
            return true;
        }

        if r.projects.contains(pos) {
            self.focus = Focus::Projects;
            if let Some(i) = row_at(r.projects, pos.y, self.store.projects.len()) {
                self.select_project_to(i);
            }
            return true;
        }

        // The detail pane, when one is on screen.
        if r.detail.area() > 0 && r.detail.contains(pos) && self.detail_available() {
            self.focus = Focus::Detail;
            if self.tab == Tab::Todos {
                let len = self.current_todo().map(|t| t.subtasks.len()).unwrap_or(0);
                if let Some(i) = row_at(r.detail, pos.y, len) {
                    self.subtask_idx = i;
                    self.sub_note_scroll = 0;
                }
            }
            return true;
        }

        if r.content.area() > 0 && r.content.contains(pos) && !self.store.projects.is_empty() {
            self.focus = Focus::Content;
            match self.tab {
                Tab::Todos => {
                    let len = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
                    if let Some(i) = row_at(r.content, pos.y, len) {
                        self.todo_idx = i;
                        self.subtask_idx = 0;
                        self.todo_note_scroll = 0;
                        self.sub_note_scroll = 0;
                    }
                }
                Tab::Notes => {
                    let len = self.current_project().map(|p| p.notes.len()).unwrap_or(0);
                    if let Some(i) = row_at(r.content, pos.y, len) {
                        self.note_idx = i;
                        self.note_scroll = 0;
                    }
                }
                Tab::Schedule => {
                    let len = self.timeline().len();
                    if let Some(i) = row_at(r.content, pos.y, len) {
                        self.timeline_idx = i;
                    }
                }
                Tab::Overview => {}
            }
            return true;
        }

        false
    }

    fn select_project_to(&mut self, i: usize) {
        if i != self.project_idx {
            self.project_idx = i;
            self.reset_content_idx();
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => self.should_quit = true,
                KeyCode::Char('g') => self.link_repo_prompt(),
                KeyCode::Char('y') => self.sync_action(),
                KeyCode::Char('e') => self.open_settings = true,
                KeyCode::Char('t') => self.open_theme(),
                // page scroll in the note body
                KeyCode::Char('d') if self.focus == Focus::Detail && self.tab == Tab::Notes => {
                    self.note_scroll = self.note_scroll.saturating_add(10);
                }
                KeyCode::Char('u') if self.focus == Focus::Detail && self.tab == Tab::Notes => {
                    self.note_scroll = self.note_scroll.saturating_sub(10);
                }
                // scroll the note(s) shown under the subtask list
                KeyCode::Char('d')
                    if self.showing_todo_note() || self.showing_sub_note() =>
                {
                    self.scroll_detail_note(6);
                }
                KeyCode::Char('u')
                    if self.showing_todo_note() || self.showing_sub_note() =>
                {
                    self.scroll_detail_note(-6);
                }
                _ => {}
            }
            return;
        }

        let g_pending = std::mem::replace(&mut self.pending_g, false);

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('/') => self.open_search(),
            KeyCode::Char('m') => {
                self.minimal = !self.minimal;
                self.status = if self.minimal {
                    "minimal view — m to restore".into()
                } else {
                    "full view".into()
                };
            }
            // Tab switches the content view (Overview / Todos / Notes / Schedule).
            KeyCode::Tab => self.cycle_tab(true),
            KeyCode::BackTab => self.cycle_tab(false),
            KeyCode::Char('1') => self.goto_tab(Tab::Overview),
            KeyCode::Char('2') => self.goto_tab(Tab::Todos),
            KeyCode::Char('3') => self.goto_tab(Tab::Notes),
            KeyCode::Char('4') => self.goto_tab(Tab::Schedule),
            // Switch project from anywhere, without leaving the current view.
            KeyCode::Char('w') => self.select_project(-1),
            KeyCode::Char('s') => self.select_project(1),
            KeyCode::Esc => {
                if self.tab == Tab::Notes && self.focus == Focus::Detail && self.note_expanded {
                    self.note_expanded = false;
                } else {
                    self.focus = match self.focus {
                        Focus::Detail => Focus::Content,
                        _ => Focus::Projects,
                    };
                }
            }
            KeyCode::Char('g') => {
                if g_pending {
                    self.move_sel(-1_000_000);
                } else {
                    self.pending_g = true;
                }
            }
            KeyCode::Char('G') => self.move_sel(1_000_000),
            _ => match self.focus {
                Focus::Projects => self.handle_projects_key(key),
                Focus::Content => match self.tab {
                    Tab::Overview => self.handle_overview_key(key),
                    Tab::Todos => self.handle_todos_key(key),
                    Tab::Notes => self.handle_notes_key(key),
                    Tab::Schedule => self.handle_timeline_key(key),
                },
                Focus::Detail => match self.tab {
                    Tab::Notes => self.handle_note_body_key(key),
                    _ => self.handle_subtasks_key(key),
                },
            },
        }
    }

    // ---- projects panel ----------------------------------------------

    fn handle_projects_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('a') => self.begin_input(
                "New project   (#tag for tags)",
                String::new(),
                InputAction::AddProject,
            ),
            KeyCode::Char('r') => {
                if let Some(p) = self.current_project() {
                    let pre = project_edit_string(p);
                    self.begin_input("Rename project   (#tag)", pre, InputAction::RenameProject);
                }
            }
            KeyCode::Char('d') => {
                if let Some(p) = self.current_project() {
                    let prompt = format!("Delete project \"{}\" and all its items?", p.name);
                    let i = self.project_idx;
                    self.mode = Mode::Confirm(ConfirmState {
                        prompt,
                        action: ConfirmAction::DeleteProject(i),
                    });
                }
            }
            KeyCode::Char('o') => self.show_github(),
            KeyCode::Char('t') => self.open_tags(),
            KeyCode::Char('i') => self.project_info = !self.project_info,
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                if !self.store.projects.is_empty() {
                    self.focus = Focus::Content;
                }
            }
            _ => {}
        }
    }

    // ---- todos tab --------------------------------------------------

    fn handle_todos_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Projects,
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
                if self.current_todo().is_some() {
                    self.subtask_idx = 0;
                    self.sub_note_scroll = 0;
                    self.focus = Focus::Detail;
                }
            }
            KeyCode::Char('n') => self.toggle_todo_note(),
            KeyCode::Char('a') => {
                if self.current_project().is_some() {
                    self.begin_input(
                        "New todo   (!1..!3 · @YYYY-MM-DD · #tag)",
                        String::new(),
                        InputAction::AddTodo,
                    );
                } else {
                    self.status = "create a project first (press 1, then a)".into();
                }
            }
            KeyCode::Char('e') => {
                if let Some(t) = self
                    .current_project()
                    .and_then(|p| p.todos.get(self.todo_idx))
                {
                    let pre = todo_edit_string(t);
                    let i = self.todo_idx;
                    self.begin_input("Edit todo", pre, InputAction::EditTodo(i));
                }
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                let i = self.todo_idx;
                if let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i)) {
                    t.done = !t.done;
                    for s in &mut t.subtasks {
                        s.done = t.done;
                    }
                    self.dirty = true;
                }
            }
            KeyCode::Char('p') => {
                let i = self.todo_idx;
                if let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i)) {
                    t.priority = t.priority.next();
                    self.dirty = true;
                }
            }
            KeyCode::Char('d') => {
                if let Some(t) = self
                    .current_project()
                    .and_then(|p| p.todos.get(self.todo_idx))
                {
                    let prompt = format!("Delete todo \"{}\"?", t.title);
                    let i = self.todo_idx;
                    self.mode = Mode::Confirm(ConfirmState {
                        prompt,
                        action: ConfirmAction::DeleteTodo(i),
                    });
                }
            }
            KeyCode::Char('J') => self.reorder_todo(1),
            KeyCode::Char('K') => self.reorder_todo(-1),
            KeyCode::Char('o') => self.sort_todos_by_priority(),
            KeyCode::Char('N') => self.begin_edit_todo_note(),
            KeyCode::Char('A') => self.open_attachments(),
            KeyCode::Char('t') => self.open_tags(),
            KeyCode::Char('i') => self.todo_info = !self.todo_info,
            _ => {}
        }
    }

    /// Stable-sort the current project's todos, highest priority first. Keeps the
    /// selection on the same todo.
    fn sort_todos_by_priority(&mut self) {
        let selected_title = self.current_todo().map(|t| t.title.clone());
        if let Some(p) = self.current_project_mut() {
            if p.todos.len() < 2 {
                return;
            }
            p.todos.sort_by_key(|t| t.priority.rank());
            self.dirty = true;
        }
        if let (Some(title), Some(p)) = (selected_title, self.current_project())
            && let Some(i) = p.todos.iter().position(|t| t.title == title)
        {
            self.todo_idx = i;
        }
        self.subtask_idx = 0;
        self.status = "sorted by priority".into();
    }

    fn reorder_todo(&mut self, delta: i32) {
        let i = self.todo_idx;
        let len = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
        if len < 2 {
            return;
        }
        let j = (i as i32 + delta).clamp(0, len as i32 - 1) as usize;
        if i == j {
            return;
        }
        if let Some(p) = self.current_project_mut() {
            p.todos.swap(i, j);
        }
        self.todo_idx = j;
        self.dirty = true;
    }

    // ---- subtasks pane ------------------------------------------

    fn handle_subtasks_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Content,
            KeyCode::Char('n') => self.toggle_sub_note(),
            KeyCode::Char('a') => {
                if self.current_todo().is_some() {
                    self.begin_input(
                        "New subtask   (!1..!3 · #tag)",
                        String::new(),
                        InputAction::AddSubtask,
                    );
                }
            }
            KeyCode::Char('e') => {
                if let Some(s) = self
                    .current_todo()
                    .and_then(|t| t.subtasks.get(self.subtask_idx))
                {
                    let pre = subtask_edit_string(s);
                    let i = self.subtask_idx;
                    self.begin_input("Edit subtask", pre, InputAction::EditSubtask(i));
                }
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                let i = self.subtask_idx;
                if let Some(s) = self.current_todo_mut().and_then(|t| t.subtasks.get_mut(i)) {
                    s.done = !s.done;
                    self.dirty = true;
                }
                self.sync_parent_done();
            }
            KeyCode::Char('p') => {
                let i = self.subtask_idx;
                if let Some(s) = self.current_todo_mut().and_then(|t| t.subtasks.get_mut(i)) {
                    s.priority = s.priority.next();
                    self.dirty = true;
                }
            }
            KeyCode::Char('d') => {
                if let Some(s) = self
                    .current_todo()
                    .and_then(|t| t.subtasks.get(self.subtask_idx))
                {
                    let prompt = format!("Delete subtask \"{}\"?", s.title);
                    let i = self.subtask_idx;
                    self.mode = Mode::Confirm(ConfirmState {
                        prompt,
                        action: ConfirmAction::DeleteSubtask(i),
                    });
                }
            }
            KeyCode::Char('J') => self.reorder_subtask(1),
            KeyCode::Char('K') => self.reorder_subtask(-1),
            KeyCode::Char('N') => self.begin_edit_subtask_note(),
            KeyCode::Char('A') => self.open_subtask_attachments(),
            KeyCode::Char('t') => self.open_tags(),
            KeyCode::Char('i') => self.subtask_info = !self.subtask_info,
            _ => {}
        }
    }

    fn reorder_subtask(&mut self, delta: i32) {
        let i = self.subtask_idx;
        let len = self.current_todo().map(|t| t.subtasks.len()).unwrap_or(0);
        if len < 2 {
            return;
        }
        let j = (i as i32 + delta).clamp(0, len as i32 - 1) as usize;
        if i == j {
            return;
        }
        if let Some(t) = self.current_todo_mut() {
            t.subtasks.swap(i, j);
        }
        self.subtask_idx = j;
        self.dirty = true;
    }

    // ---- overview tab -------------------------------------------

    fn handle_overview_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Projects,
            KeyCode::Char('e') | KeyCode::Char('a') => {
                if let Some(p) = self.current_project() {
                    let pre = p.description.clone();
                    self.begin_input("Project description", pre, InputAction::EditDescription);
                }
            }
            KeyCode::Char('r') => {
                if let Some(p) = self.current_project() {
                    let pre = project_edit_string(p);
                    self.begin_input("Rename project   (#tag)", pre, InputAction::RenameProject);
                }
            }
            KeyCode::Char('t') => self.open_tags(),
            _ => {}
        }
    }

    // ---- notes tab ---------------------------------------------

    fn handle_notes_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Projects,
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
                if self.current_note().is_some() {
                    self.note_scroll = 0;
                    self.focus = Focus::Detail;
                }
            }
            KeyCode::Char('a') => {
                if self.current_project().is_some() {
                    self.begin_input("New note", String::new(), InputAction::AddNote);
                } else {
                    self.status = "create a project first".into();
                }
            }
            KeyCode::Char('e') => {
                if let Some(n) = self
                    .current_project()
                    .and_then(|p| p.notes.get(self.note_idx))
                {
                    let pre = n.text.clone();
                    let i = self.note_idx;
                    self.begin_input("Edit note", pre, InputAction::EditNote(i));
                }
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                let i = self.note_idx;
                if let Some(n) = self.current_project_mut().and_then(|p| p.notes.get_mut(i)) {
                    n.pinned = !n.pinned;
                    self.dirty = true;
                }
            }
            KeyCode::Char('d') => {
                if let Some(n) = self
                    .current_project()
                    .and_then(|p| p.notes.get(self.note_idx))
                {
                    let prompt = format!("Delete note \"{}\"?", truncate(&n.text, 40));
                    let i = self.note_idx;
                    self.mode = Mode::Confirm(ConfirmState {
                        prompt,
                        action: ConfirmAction::DeleteNote(i),
                    });
                }
            }
            KeyCode::Char('J') => self.reorder_note(1),
            KeyCode::Char('K') => self.reorder_note(-1),
            _ => {}
        }
    }

    fn reorder_note(&mut self, delta: i32) {
        let i = self.note_idx;
        let len = self.current_project().map(|p| p.notes.len()).unwrap_or(0);
        if len < 2 {
            return;
        }
        let j = (i as i32 + delta).clamp(0, len as i32 - 1) as usize;
        if i == j {
            return;
        }
        if let Some(p) = self.current_project_mut() {
            p.notes.swap(i, j);
        }
        self.note_idx = j;
        self.dirty = true;
    }

    // ---- note body pane (rendered markdown, beside a note) ------

    fn handle_note_body_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.note_scroll = self.note_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.note_scroll = self.note_scroll.saturating_sub(1)
            }
            KeyCode::PageDown => self.note_scroll = self.note_scroll.saturating_add(10),
            KeyCode::PageUp => self.note_scroll = self.note_scroll.saturating_sub(10),
            KeyCode::Char('h') | KeyCode::Left => {
                if self.note_expanded {
                    self.note_expanded = false;
                } else {
                    self.focus = Focus::Content;
                }
            }
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => {
                self.note_expanded = !self.note_expanded;
            }
            KeyCode::Char('e') | KeyCode::Char('i') => self.begin_edit_body(),
            _ => {}
        }
    }

    fn begin_edit_body(&mut self) {
        let Some(note) = self.current_note() else {
            return;
        };
        let body = note.body.clone();
        self.begin_edit(EditTarget::NoteBody(self.note_idx), &body);
        self.status = "editing note — esc / ^s to save".into();
    }

    /// Open the Markdown editor on the current todo's attached note.
    fn begin_edit_todo_note(&mut self) {
        let Some(todo) = self.current_todo() else {
            self.status = "pick a todo first".into();
            return;
        };
        let note = todo.note.clone();
        self.begin_edit(EditTarget::TodoNote(self.todo_idx), &note);
        self.status = "editing todo note — esc / ^s to save".into();
    }

    /// Open the Markdown editor on the selected subtask's attached note.
    fn begin_edit_subtask_note(&mut self) {
        let Some(sub) = self
            .current_todo()
            .and_then(|t| t.subtasks.get(self.subtask_idx))
        else {
            self.status = "pick a subtask first".into();
            return;
        };
        let note = sub.note.clone();
        self.begin_edit(EditTarget::SubtaskNote(self.subtask_idx), &note);
        self.status = "editing subtask note — esc / ^s to save".into();
    }

    fn begin_edit(&mut self, target: EditTarget, initial: &str) {
        let lines: Vec<String> = if initial.is_empty() {
            vec![String::new()]
        } else {
            initial.lines().map(str::to_string).collect()
        };
        let mut textarea = TextArea::new(lines);
        textarea.move_cursor(CursorMove::Bottom);
        textarea.move_cursor(CursorMove::End);
        textarea.set_placeholder_text("Write in Markdown…  # heading, - bullet, **bold**, `code`");
        self.mode = Mode::EditBody(Box::new(EditState { target, textarea }));
    }

    fn handle_edit_body(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.commit_edit_body(),
            KeyCode::Char('s') if ctrl => self.commit_edit_body(),
            _ => {
                if let Mode::EditBody(state) = &mut self.mode {
                    state.textarea.input(key);
                }
            }
        }
    }

    fn commit_edit_body(&mut self) {
        let Mode::EditBody(state) = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        let body = state.textarea.lines().join("\n").trim_end().to_string();
        match state.target {
            EditTarget::NoteBody(i) => {
                if let Some(n) = self.current_project_mut().and_then(|p| p.notes.get_mut(i)) {
                    n.body = body;
                    self.dirty = true;
                    self.status = "note saved".into();
                }
                self.note_scroll = 0;
            }
            EditTarget::TodoNote(i) => {
                if let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i)) {
                    t.note = body;
                    self.dirty = true;
                    self.status = "todo note saved".into();
                }
            }
            EditTarget::SubtaskNote(i) => {
                let ti = self.todo_idx;
                if let Some(s) = self
                    .current_project_mut()
                    .and_then(|p| p.todos.get_mut(ti))
                    .and_then(|t| t.subtasks.get_mut(i))
                {
                    s.note = body;
                    self.dirty = true;
                    self.status = "subtask note saved".into();
                }
            }
        }
    }

    // ---- attachments -------------------------------------------

    /// Open the manager for the todo selected in the Todos tab.
    fn open_attachments(&mut self) {
        if self.current_todo().is_none() {
            self.status = "pick a todo first".into();
            return;
        }
        self.open_attachments_for(AttachTarget {
            todo_idx: self.todo_idx,
            sub_idx: None,
        });
    }

    /// Open the manager for the subtask selected in the Subtasks pane.
    fn open_subtask_attachments(&mut self) {
        let Some(t) = self.current_todo() else {
            self.status = "pick a todo first".into();
            return;
        };
        if self.subtask_idx >= t.subtasks.len() {
            self.status = "pick a subtask first".into();
            return;
        }
        self.open_attachments_for(AttachTarget {
            todo_idx: self.todo_idx,
            sub_idx: Some(self.subtask_idx),
        });
    }

    fn open_attachments_for(&mut self, target: AttachTarget) {
        self.mode = Mode::Attach(AttachState { target, sel: 0 });
    }

    /// The attachment list a target points at (read-only).
    pub fn attachments_at(&self, t: AttachTarget) -> Option<&Vec<Attachment>> {
        let todo = self.current_project()?.todos.get(t.todo_idx)?;
        match t.sub_idx {
            None => Some(&todo.attachments),
            Some(i) => todo.subtasks.get(i).map(|s| &s.attachments),
        }
    }

    fn attachments_at_mut(&mut self, t: AttachTarget) -> Option<&mut Vec<Attachment>> {
        let todo = self.current_project_mut()?.todos.get_mut(t.todo_idx)?;
        match t.sub_idx {
            None => Some(&mut todo.attachments),
            Some(i) => todo.subtasks.get_mut(i).map(|s| &mut s.attachments),
        }
    }

    fn handle_attach(&mut self, key: KeyEvent) {
        let Mode::Attach(state) = &self.mode else {
            return;
        };
        let (target, sel) = (state.target, state.sel);
        let len = self.attachments_at(target).map(Vec::len).unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => {
                if let Mode::Attach(s) = &mut self.mode {
                    s.sel = step(sel, 1, len);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Mode::Attach(s) = &mut self.mode {
                    s.sel = step(sel, -1, len);
                }
            }
            KeyCode::Char('a') => self.begin_input(
                "Attachment — URL or path   (append  | label  to name it)",
                String::new(),
                InputAction::AddAttachment(target),
            ),
            KeyCode::Char('d') => {
                if let Some(list) = self.attachments_at_mut(target)
                    && sel < list.len()
                {
                    list.remove(sel);
                    self.dirty = true;
                    self.status = "attachment removed".into();
                }
                if let Mode::Attach(s) = &mut self.mode {
                    s.sel = s.sel.min(len.saturating_sub(2));
                }
            }
            KeyCode::Char('o') | KeyCode::Char('l') | KeyCode::Enter => {
                if let Some(a) = self
                    .attachments_at(target)
                    .and_then(|list| list.get(sel))
                    .map(|a| a.value.clone())
                {
                    self.open_external(&a);
                }
            }
            _ => {}
        }
    }

    /// Hand a URL or path to the OS opener (`open` / `xdg-open` / `explorer`).
    fn open_external(&mut self, target: &str) {
        let target = target.trim();
        if target.is_empty() {
            return;
        }
        let program = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(target_os = "windows") {
            "explorer"
        } else {
            "xdg-open"
        };
        match std::process::Command::new(program)
            .arg(target)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => self.status = format!("opening {}", truncate(target, 40)),
            Err(e) => self.status = format!("could not open: {e}"),
        }
    }

    // ---- tags -------------------------------------------------

    /// `t` opens the tag manager for whatever's under the cursor.
    fn open_tags(&mut self) {
        let target = match self.focus {
            Focus::Projects => Some(TagTarget::Project),
            Focus::Content if self.tab == Tab::Overview => Some(TagTarget::Project),
            Focus::Content if self.tab == Tab::Todos && self.current_todo().is_some() => {
                Some(TagTarget::Todo(self.todo_idx))
            }
            Focus::Detail
                if self.tab == Tab::Todos
                    && self
                        .current_todo()
                        .is_some_and(|t| self.subtask_idx < t.subtasks.len()) =>
            {
                Some(TagTarget::Subtask {
                    todo: self.todo_idx,
                    sub: self.subtask_idx,
                })
            }
            _ => None,
        };
        match target {
            Some(t) => self.mode = Mode::Tags(TagState { target: t, sel: 0 }),
            None => self.status = "nothing here to tag".into(),
        }
    }

    pub fn tags_at(&self, t: TagTarget) -> Option<&Vec<String>> {
        let p = self.current_project()?;
        match t {
            TagTarget::Project => Some(&p.tags),
            TagTarget::Todo(i) => p.todos.get(i).map(|t| &t.tags),
            TagTarget::Subtask { todo, sub } => {
                p.todos.get(todo).and_then(|t| t.subtasks.get(sub)).map(|s| &s.tags)
            }
        }
    }

    fn tags_at_mut(&mut self, t: TagTarget) -> Option<&mut Vec<String>> {
        let p = self.current_project_mut()?;
        match t {
            TagTarget::Project => Some(&mut p.tags),
            TagTarget::Todo(i) => p.todos.get_mut(i).map(|t| &mut t.tags),
            TagTarget::Subtask { todo, sub } => p
                .todos
                .get_mut(todo)
                .and_then(|t| t.subtasks.get_mut(sub))
                .map(|s| &mut s.tags),
        }
    }

    fn handle_tags(&mut self, key: KeyEvent) {
        let Mode::Tags(state) = &self.mode else {
            return;
        };
        let (target, sel) = (state.target, state.sel);
        let len = self.tags_at(target).map(Vec::len).unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => {
                if let Mode::Tags(s) = &mut self.mode {
                    s.sel = step(sel, 1, len);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Mode::Tags(s) = &mut self.mode {
                    s.sel = step(sel, -1, len);
                }
            }
            KeyCode::Char('a') => self.begin_input(
                "Add tags   (space-separated · # optional)",
                String::new(),
                InputAction::AddTag(target),
            ),
            KeyCode::Char('d') => {
                if let Some(list) = self.tags_at_mut(target)
                    && sel < list.len()
                {
                    let removed = list.remove(sel);
                    self.dirty = true;
                    self.status = format!("removed #{removed}");
                }
                if let Mode::Tags(s) = &mut self.mode {
                    s.sel = s.sel.min(len.saturating_sub(2));
                }
            }
            _ => {}
        }
    }

    // ---- fuzzy finder -----------------------------------------

    fn open_search(&mut self) {
        let mut editor = TextArea::new(vec![String::new()]);
        editor.set_placeholder_text("Fuzzy-find a project, todo or note…");
        self.mode = Mode::Search(Box::new(SearchState { editor, sel: 0 }));
    }

    /// Score every project / todo / note against the current query. Empty query
    /// lists everything (projects first, then todos, then notes, per project).
    pub fn search_results(&self) -> Vec<SearchHit> {
        let q = match &self.mode {
            Mode::Search(s) => s.query().trim().to_lowercase(),
            _ => return Vec::new(),
        };
        let mut scored: Vec<(i32, usize, SearchHit)> = Vec::new();
        for (pi, p) in self.store.projects.iter().enumerate() {
            // `hay` is matched against; `label` is what the result shows; `crumbs`
            // are the ancestor names. Tags fold into the haystack.
            let mut consider = |label: &str, hay: &str, target: SearchTarget, crumbs: Vec<String>| {
                let score = if q.is_empty() {
                    Some(0)
                } else {
                    fuzzy_score(&q, &hay.to_lowercase())
                };
                if let Some(score) = score {
                    let order = scored.len();
                    scored.push((
                        score,
                        order,
                        SearchHit {
                            project_idx: pi,
                            target,
                            label: label.to_string(),
                            crumbs,
                        },
                    ));
                }
            };
            let with_tags = |base: &str, tags: &[String]| {
                if tags.is_empty() {
                    base.to_string()
                } else {
                    format!("{base} {}", tags.join(" "))
                }
            };
            consider(
                &p.name,
                &with_tags(&p.name, &p.tags),
                SearchTarget::Project,
                vec![],
            );
            for (ti, t) in p.todos.iter().enumerate() {
                consider(
                    &t.title,
                    &with_tags(&t.title, &t.tags),
                    SearchTarget::Todo(ti),
                    vec![p.name.clone()],
                );
                if !q.is_empty() {
                    for (si, s) in t.subtasks.iter().enumerate() {
                        consider(
                            &s.title,
                            &with_tags(&s.title, &s.tags),
                            SearchTarget::Subtask { todo: ti, sub: si },
                            vec![p.name.clone(), t.title.clone()],
                        );
                    }
                }
            }
            for (ni, n) in p.notes.iter().enumerate() {
                consider(&n.text, &n.text, SearchTarget::Note(ni), vec![p.name.clone()]);
            }
        }
        // Highest score first; stable on insertion order for ties / empty query.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.truncate(40);
        scored.into_iter().map(|(_, _, h)| h).collect()
    }

    fn move_search_sel(&mut self, delta: i32) {
        let len = self.search_results().len();
        if let Mode::Search(s) = &mut self.mode {
            s.sel = step(s.sel, delta, len);
        }
    }

    fn commit_search(&mut self) {
        let sel = match &self.mode {
            Mode::Search(s) => s.sel,
            _ => return,
        };
        let Some(hit) = self.search_results().get(sel).cloned() else {
            self.mode = Mode::Normal;
            return;
        };
        self.project_idx = hit.project_idx;
        self.reset_content_idx();
        match hit.target {
            SearchTarget::Project => self.focus = Focus::Projects,
            SearchTarget::Todo(i) => {
                self.tab = Tab::Todos;
                self.todo_idx = i;
                self.subtask_idx = 0;
                self.focus = Focus::Content;
            }
            SearchTarget::Subtask { todo, sub } => {
                self.tab = Tab::Todos;
                self.todo_idx = todo;
                self.subtask_idx = sub;
                self.focus = Focus::Detail;
            }
            SearchTarget::Note(i) => {
                self.tab = Tab::Notes;
                self.note_idx = i;
                self.note_scroll = 0;
                self.focus = Focus::Content;
            }
        }
        self.mode = Mode::Normal;
        self.status = format!("jumped to {}", truncate(&hit.label, 40));
    }

    fn handle_search(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => self.commit_search(),
            KeyCode::Down | KeyCode::Tab => self.move_search_sel(1),
            KeyCode::Up | KeyCode::BackTab => self.move_search_sel(-1),
            KeyCode::Char('n') if ctrl => self.move_search_sel(1),
            KeyCode::Char('p') if ctrl => self.move_search_sel(-1),
            _ => {
                if let Mode::Search(s) = &mut self.mode {
                    s.editor.input(key);
                    s.sel = 0;
                }
            }
        }
    }

    // ---- theme picker -------------------------------------------

    /// Drop the memoised note-body render so it re-runs with the new palette.
    fn clear_render_cache(&self) {
        *self.md_cache.borrow_mut() = None;
    }

    fn open_theme(&mut self) {
        let idx = crate::theme::current_index();
        self.mode = Mode::Theme(ThemeState { idx, saved: idx });
    }

    fn preview_theme(&mut self, idx: usize) {
        crate::theme::set_index(idx);
        self.clear_render_cache();
    }

    fn handle_theme(&mut self, key: KeyEvent) {
        let Mode::Theme(state) = &self.mode else {
            return;
        };
        let (idx, saved, n) = (state.idx, state.saved, crate::theme::registry().len());
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let next = (idx + 1) % n;
                if let Mode::Theme(s) = &mut self.mode {
                    s.idx = next;
                }
                self.preview_theme(next);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let prev = (idx + n - 1) % n;
                if let Mode::Theme(s) = &mut self.mode {
                    s.idx = prev;
                }
                self.preview_theme(prev);
            }
            KeyCode::Enter | KeyCode::Char('l') => {
                self.preview_theme(idx);
                self.config.theme = Some(crate::theme::current().slug.clone());
                let _ = self.config.save();
                self.mode = Mode::Normal;
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.preview_theme(saved);
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    // ---- deadlines tab ---------------------------------------------

    fn handle_timeline_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Projects,
            KeyCode::Char('f') => {
                self.deadline_filter = self.deadline_filter.next();
                self.timeline_idx = 0;
                self.status = format!("filter: {}", self.deadline_filter.label());
            }
            KeyCode::Char('a') => {
                if self.current_project().is_some() {
                    self.begin_input(
                        "New milestone   (add @YYYY-MM-DD for the date)",
                        String::new(),
                        InputAction::AddMilestone,
                    );
                } else {
                    self.status = "create a project first".into();
                }
            }
            KeyCode::Char('e') => match self.selected_milestone_idx() {
                Some(mi) => {
                    if let Some(m) = self.current_project().and_then(|p| p.milestones.get(mi)) {
                        let pre = milestone_edit_string(m);
                        self.begin_input("Edit milestone", pre, InputAction::EditMilestone(mi));
                    }
                }
                None => {
                    self.status = "pick a milestone (◆) — todos are edited on the Todos tab".into()
                }
            },
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                if let Some(entry) = self.current_deadline() {
                    if let Some(ti) = entry.todo_idx {
                        let done = self
                            .current_project_mut()
                            .and_then(|p| p.todos.get_mut(ti))
                            .map(|t| {
                                t.done = !t.done;
                                for s in &mut t.subtasks {
                                    s.done = t.done;
                                }
                                t.done
                            });
                        if let Some(done) = done {
                            self.dirty = true;
                            self.status = if done { "todo done" } else { "todo reopened" }.into();
                        }
                    } else if let Some(mi) = entry.milestone_idx
                        && let Some(m) = self
                            .current_project_mut()
                            .and_then(|p| p.milestones.get_mut(mi))
                    {
                        m.done = !m.done;
                        self.dirty = true;
                    }
                }
            }
            KeyCode::Char('r') => {
                if let Some(entry) = self.current_deadline() {
                    if let Some(ti) = entry.todo_idx {
                        let pre = format!("@{}", entry.date.format("%Y-%m-%d"));
                        self.begin_input(
                            "Reschedule todo   (@YYYY-MM-DD)",
                            pre,
                            InputAction::RescheduleTodo(ti),
                        );
                    } else if let Some(mi) = entry.milestone_idx {
                        let pre = format!("@{}", entry.date.format("%Y-%m-%d"));
                        self.begin_input(
                            "Reschedule milestone   (@YYYY-MM-DD)",
                            pre,
                            InputAction::RescheduleMilestone(mi),
                        );
                    }
                }
            }
            KeyCode::Enter | KeyCode::Char('l') => {
                if let Some(entry) = self.current_deadline() {
                    if entry.kind == TlKind::Todo {
                        if let Some(ti) = entry.todo_idx {
                            self.todo_idx = ti;
                            self.tab = Tab::Todos;
                            self.focus = Focus::Content;
                            self.status = "jumped to todo".into();
                        }
                    } else if let Some(mi) = entry.milestone_idx
                        && let Some(m) = self.current_project().and_then(|p| p.milestones.get(mi))
                    {
                        let pre = milestone_edit_string(m);
                        self.begin_input("Edit milestone", pre, InputAction::EditMilestone(mi));
                    }
                }
            }
            KeyCode::Char('d') => {
                if let Some(entry) = self.current_deadline() {
                    if let Some(mi) = entry.milestone_idx {
                        if let Some(m) = self.current_project().and_then(|p| p.milestones.get(mi)) {
                            let prompt = format!("Delete milestone \"{}\"?", m.title);
                            self.mode = Mode::Confirm(ConfirmState {
                                prompt,
                                action: ConfirmAction::DeleteMilestone(mi),
                            });
                        }
                    } else if let Some(ti) = entry.todo_idx
                        && let Some(t) = self.current_project().and_then(|p| p.todos.get(ti))
                    {
                        let prompt = format!("Delete todo \"{}\"?", t.title);
                        self.mode = Mode::Confirm(ConfirmState {
                            prompt,
                            action: ConfirmAction::DeleteTodo(ti),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    // ---- input / confirm modes ----------------------------------

    fn handle_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status = "cancelled".into();
            }
            KeyCode::Enter => {
                if let Mode::Input(input) = std::mem::replace(&mut self.mode, Mode::Normal) {
                    self.commit_input(*input);
                }
            }
            _ => {
                if let Mode::Input(input) = &mut self.mode {
                    // Single-line field: swallow newlines, accept everything else
                    // (cursor movement, word-delete, paste) via tui-textarea.
                    input.editor.input(key);
                }
            }
        }
    }

    fn handle_confirm(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if let Mode::Confirm(c) = std::mem::replace(&mut self.mode, Mode::Normal) {
                    self.perform_confirm(c.action);
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status = "cancelled".into();
            }
            _ => {}
        }
    }

    fn commit_input(&mut self, input: InputState) {
        let value = input.value();
        match input.action {
            InputAction::AddProject => {
                let (name, tags) = parse_tagged_name(&value);
                if name.is_empty() {
                    self.status = "nothing added".into();
                    return;
                }
                let mut project = Project::new(&name);
                project.tags = tags;
                self.store.projects.push(project);
                self.project_idx = self.store.projects.len() - 1;
                self.focus = Focus::Content;
                self.tab = Tab::Todos;
                self.reset_content_idx();
                self.dirty = true;
                self.status = format!("added project: {name}");
            }
            InputAction::RenameProject => {
                let (name, tags) = parse_tagged_name(&value);
                if name.is_empty() {
                    return;
                }
                if let Some(p) = self.current_project_mut() {
                    p.name = name.clone();
                    p.tags = tags;
                }
                self.dirty = true;
                self.status = format!("renamed to: {name}");
            }
            InputAction::EditDescription => {
                let text = value.trim().to_string();
                if let Some(p) = self.current_project_mut() {
                    p.description = text;
                    self.dirty = true;
                    self.status = "description updated".into();
                }
            }
            InputAction::AddNote => {
                let text = value.trim().to_string();
                if text.is_empty() {
                    self.status = "nothing added".into();
                    return;
                }
                let mut len = 0;
                if let Some(p) = self.current_project_mut() {
                    p.notes.push(Note::new(text, false));
                    len = p.notes.len();
                }
                self.note_idx = len.saturating_sub(1);
                self.note_scroll = 0;
                self.focus = Focus::Detail;
                self.dirty = true;
                self.begin_edit_body();
            }
            InputAction::EditNote(i) => {
                let text = value.trim().to_string();
                if text.is_empty() {
                    return;
                }
                if let Some(n) = self.current_project_mut().and_then(|p| p.notes.get_mut(i)) {
                    n.text = text;
                    self.dirty = true;
                    self.status = "note updated".into();
                }
            }
            InputAction::AddAttachment(target) => {
                let raw = value.trim();
                if raw.is_empty() {
                    self.status = "nothing added".into();
                } else {
                    let (val, label) = match raw.split_once('|') {
                        Some((v, l)) => (v.trim().to_string(), l.trim().to_string()),
                        None => (raw.to_string(), String::new()),
                    };
                    if let Some(list) = self.attachments_at_mut(target) {
                        list.push(Attachment::new(val, label));
                        self.dirty = true;
                        self.status = "attachment added".into();
                    }
                }
                let sel = self
                    .attachments_at(target)
                    .map(|list| list.len().saturating_sub(1))
                    .unwrap_or(0);
                self.mode = Mode::Attach(AttachState { target, sel });
            }
            InputAction::AddTag(target) => {
                let new = crate::model::normalize_tags(value.split_whitespace());
                if new.is_empty() {
                    self.status = "no tags added".into();
                } else if let Some(list) = self.tags_at_mut(target) {
                    let before = list.len();
                    for t in new {
                        if !list.contains(&t) {
                            list.push(t);
                        }
                    }
                    let added = list.len() - before;
                    self.dirty = true;
                    self.status = match added {
                        0 => "already tagged".into(),
                        1 => "tag added".into(),
                        n => format!("{n} tags added"),
                    };
                }
                let sel = self
                    .tags_at(target)
                    .map(|list| list.len().saturating_sub(1))
                    .unwrap_or(0);
                self.mode = Mode::Tags(TagState { target, sel });
            }
            InputAction::AddTodo => {
                let (title, priority, due, tags) = parse_todo_input(&value);
                if title.is_empty() {
                    self.status = "nothing added".into();
                    return;
                }
                if let Some(p) = self.current_project_mut() {
                    let mut todo = Todo::new(title);
                    todo.priority = priority;
                    todo.due = due;
                    todo.tags = tags;
                    p.todos.push(todo);
                }
                let len = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
                self.todo_idx = len.saturating_sub(1);
                self.subtask_idx = 0;
                self.dirty = true;
                self.status = "todo added".into();
            }
            InputAction::EditTodo(i) => {
                let (title, priority, due, tags) = parse_todo_input(&value);
                if title.is_empty() {
                    return;
                }
                if let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i)) {
                    t.title = title;
                    t.priority = priority;
                    t.due = due;
                    t.tags = tags;
                    self.dirty = true;
                    self.status = "todo updated".into();
                }
            }
            InputAction::AddSubtask => {
                let (title, priority, tags) = parse_priority_input(&value);
                if title.is_empty() {
                    self.status = "nothing added".into();
                    return;
                }
                let mut len = 0;
                if let Some(t) = self.current_todo_mut() {
                    let mut sub = Subtask::new(title, false);
                    sub.priority = priority;
                    sub.tags = tags;
                    t.subtasks.push(sub);
                    len = t.subtasks.len();
                }
                self.subtask_idx = len.saturating_sub(1);
                self.dirty = true;
                self.status = "subtask added".into();
                self.sync_parent_done();
            }
            InputAction::EditSubtask(i) => {
                let (title, priority, tags) = parse_priority_input(&value);
                if title.is_empty() {
                    return;
                }
                if let Some(s) = self.current_todo_mut().and_then(|t| t.subtasks.get_mut(i)) {
                    s.title = title;
                    s.priority = priority;
                    s.tags = tags;
                    self.dirty = true;
                    self.status = "subtask updated".into();
                }
            }
            InputAction::AddMilestone => {
                let today = Local::now().date_naive();
                match parse_milestone_input(&value, today) {
                    Some((title, date)) => {
                        if let Some(p) = self.current_project_mut() {
                            p.milestones.push(Milestone {
                                title,
                                date,
                                done: false,
                            });
                        }
                        self.dirty = true;
                        self.status = "milestone added".into();
                    }
                    None => self.status = "nothing added".into(),
                }
            }
            InputAction::EditMilestone(i) => {
                let today = Local::now().date_naive();
                if let Some((title, date)) = parse_milestone_input(&value, today)
                    && let Some(m) = self
                        .current_project_mut()
                        .and_then(|p| p.milestones.get_mut(i))
                {
                    m.title = title;
                    m.date = date;
                    self.dirty = true;
                    self.status = "milestone updated".into();
                }
            }
            InputAction::RescheduleTodo(i) => {
                let today = Local::now().date_naive();
                if let Some((_, date)) = parse_milestone_input(&value, today)
                    && let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i))
                {
                    t.due = Some(date);
                    self.dirty = true;
                    self.status = format!("rescheduled to {}", date.format("%Y-%m-%d"));
                }
            }
            InputAction::RescheduleMilestone(i) => {
                let today = Local::now().date_naive();
                if let Some((_, date)) = parse_milestone_input(&value, today)
                    && let Some(m) = self
                        .current_project_mut()
                        .and_then(|p| p.milestones.get_mut(i))
                {
                    m.date = date;
                    self.dirty = true;
                    self.status = format!("rescheduled to {}", date.format("%Y-%m-%d"));
                }
            }
            InputAction::LinkRepo => {
                let value = value.trim().to_string();
                if value.is_empty() {
                    if let Some(p) = self.current_project_mut() {
                        p.repo = None;
                        self.dirty = true;
                        self.gh_cache = None;
                        self.status = "repo unlinked".into();
                    }
                } else if let Some((owner, repo)) = crate::github::parse_repo_string(&value) {
                    if let Some(p) = self.current_project_mut() {
                        p.repo = Some(format!("{owner}/{repo}"));
                        self.dirty = true;
                        self.status = format!("linked to {owner}/{repo}");
                        self.fetch_github(&owner, &repo);
                    }
                } else {
                    self.status = "invalid repo format — use owner/repo".into();
                }
            }
            InputAction::SyncRepo => {
                let v = value.trim().trim_end_matches('/');
                let v = if v.is_empty() { "voido-data" } else { v };
                // Accept "owner/repo" or a bare name (owner resolved on push).
                let looks_ok = crate::github::parse_repo_string(v).is_some()
                    || !v.contains('/') && !v.contains(char::is_whitespace);
                if !looks_ok {
                    self.status = "invalid repo — use a name or owner/repo".into();
                    return;
                }
                self.pending_sync_repo = Some(v.to_string());
                if self.sync_token.is_some() {
                    self.finish_sync_setup();
                } else {
                    self.begin_input(
                        "GitHub sync — token  (classic: repo scope)",
                        self.config.github_token.clone().unwrap_or_default(),
                        InputAction::SyncToken,
                    );
                }
            }
            InputAction::SyncToken => {
                let token = value.trim().to_string();
                if token.is_empty() {
                    self.pending_sync_repo = None;
                    self.status = "sync setup cancelled — no token given".into();
                    return;
                }
                self.config.github_token = Some(token.clone());
                self.sync_token = Some(token.clone());
                self.gh_client = GitHubClient::new(Some(token));
                self.sync_sha = None;
                self.finish_sync_setup();
            }
        }
    }

    fn perform_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::DeleteProject(i) => {
                if i < self.store.projects.len() {
                    let name = self.store.projects.remove(i).name;
                    self.project_idx = self
                        .project_idx
                        .min(self.store.projects.len().saturating_sub(1));
                    self.reset_content_idx();
                    self.focus = Focus::Projects;
                    self.dirty = true;
                    self.status = format!("deleted project: {name}");
                }
            }
            ConfirmAction::DeleteTodo(i) => {
                if let Some(p) = self.current_project_mut()
                    && i < p.todos.len()
                {
                    p.todos.remove(i);
                }
                let len = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
                self.todo_idx = self.todo_idx.min(len.saturating_sub(1));
                self.subtask_idx = 0;
                self.dirty = true;
                self.status = "todo deleted".into();
            }
            ConfirmAction::DeleteSubtask(i) => {
                if let Some(t) = self.current_todo_mut()
                    && i < t.subtasks.len()
                {
                    t.subtasks.remove(i);
                }
                let len = self.current_todo().map(|t| t.subtasks.len()).unwrap_or(0);
                self.subtask_idx = self.subtask_idx.min(len.saturating_sub(1));
                self.dirty = true;
                self.status = "subtask deleted".into();
                // Removing the last open subtask can complete the parent.
                self.sync_parent_done();
            }
            ConfirmAction::DeleteNote(i) => {
                if let Some(p) = self.current_project_mut()
                    && i < p.notes.len()
                {
                    p.notes.remove(i);
                }
                let len = self.current_project().map(|p| p.notes.len()).unwrap_or(0);
                self.note_idx = self.note_idx.min(len.saturating_sub(1));
                self.dirty = true;
                self.status = "note deleted".into();
            }
            ConfirmAction::DeleteMilestone(i) => {
                if let Some(p) = self.current_project_mut()
                    && i < p.milestones.len()
                {
                    p.milestones.remove(i);
                }
                self.timeline_idx = self.timeline_idx.saturating_sub(1);
                self.dirty = true;
                self.status = "milestone deleted".into();
            }
        }
    }

    // ---- helpers --------------------------------------------------

    fn begin_input(&mut self, title: &str, value: String, action: InputAction) {
        let mut editor = TextArea::new(vec![value]);
        editor.move_cursor(CursorMove::End);
        self.mode = Mode::Input(Box::new(InputState {
            title: title.to_string(),
            editor,
            action,
        }));
    }

    fn reset_content_idx(&mut self) {
        self.todo_idx = 0;
        self.subtask_idx = 0;
        self.note_idx = 0;
        self.note_scroll = 0;
        self.todo_note_open = false;
        self.sub_note_open = false;
        self.todo_note_scroll = 0;
        self.sub_note_scroll = 0;
        self.timeline_idx = 0;
    }

    fn cycle_tab(&mut self, forward: bool) {
        let cur = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let next = if forward {
            (cur + 1) % Tab::ALL.len()
        } else {
            (cur + Tab::ALL.len() - 1) % Tab::ALL.len()
        };
        self.goto_tab(Tab::ALL[next]);
    }

    /// Switch the content view and move focus into it.
    fn goto_tab(&mut self, tab: Tab) {
        self.tab = tab;
        if !self.store.projects.is_empty() {
            self.focus = Focus::Content;
        }
    }

    /// Move the project selection without changing which pane or view is focused.
    fn select_project(&mut self, delta: i32) {
        let new = step(self.project_idx, delta, self.store.projects.len());
        if new != self.project_idx {
            self.project_idx = new;
            self.reset_content_idx();
        }
    }

    /// Prompt to link (or, with an empty answer, unlink) a GitHub repo for the
    /// selected project.
    fn link_repo_prompt(&mut self) {
        if let Some(p) = self.current_project() {
            let pre = p.repo.clone().unwrap_or_default();
            self.begin_input("Link GitHub repo (owner/repo)", pre, InputAction::LinkRepo);
        }
    }

    /// Open the GitHub activity view for the selected project's linked repo.
    fn show_github(&mut self) {
        match self.current_project().and_then(|p| p.repo.clone()) {
            Some(repo) => match crate::github::parse_repo_string(&repo) {
                Some((owner, name)) => self.fetch_github(&owner, &name),
                None => self.status = "linked repo is malformed — re-link with ^g".into(),
            },
            None => self.status = "no repo linked — press ^g to link one".into(),
        }
    }

    fn fetch_github(&mut self, owner: &str, repo: &str) {
        if self.gh_loading {
            return;
        }
        self.gh_loading = true;
        self.status = format!("fetching {owner}/{repo}…");

        let (tx, rx) = mpsc::channel();
        let client = self.gh_client.clone();
        let owner = owner.to_string();
        let repo = repo.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(client.fetch_repo_info(&owner, &repo));
        });
        self.gh_rx = Some(rx);
    }

    fn move_sel(&mut self, delta: i32) {
        match self.focus {
            Focus::Projects => {
                let len = self.store.projects.len();
                self.project_idx = step(self.project_idx, delta, len);
                self.reset_content_idx();
            }
            Focus::Content => match self.tab {
                Tab::Overview => {}
                Tab::Todos => {
                    let len = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
                    self.todo_idx = step(self.todo_idx, delta, len);
                    self.subtask_idx = 0;
                    self.todo_note_scroll = 0;
                    self.sub_note_scroll = 0;
                }
                Tab::Notes => {
                    let len = self.current_project().map(|p| p.notes.len()).unwrap_or(0);
                    self.note_idx = step(self.note_idx, delta, len);
                    self.note_scroll = 0;
                }
                Tab::Schedule => {
                    let len = self.timeline().len();
                    self.timeline_idx = step(self.timeline_idx, delta, len);
                }
            },
            Focus::Detail => match self.tab {
                Tab::Notes => {
                    self.note_scroll = if delta < 0 {
                        self.note_scroll
                            .saturating_sub((-delta).min(i32::from(u16::MAX)) as u16)
                    } else {
                        self.note_scroll
                            .saturating_add(delta.min(i32::from(u16::MAX)) as u16)
                    };
                }
                _ => {
                    let len = self.current_todo().map(|t| t.subtasks.len()).unwrap_or(0);
                    self.subtask_idx = step(self.subtask_idx, delta, len);
                    self.sub_note_scroll = 0;
                }
            },
        }
    }
}

/// Which tab (if any) sits under `x` on the content pane's top border. Mirrors
/// the strip drawn in `ui::content_block`: one ` {title} ` cell per tab, starting
/// one column in from the rounded corner.
fn tab_at_x(content: Rect, x: u16) -> Option<Tab> {
    let mut cx = content.x + 1;
    for t in Tab::ALL {
        let w = t.title().chars().count() as u16 + 2;
        if x >= cx && x < cx + w {
            return Some(t);
        }
        cx += w;
    }
    None
}

/// Map a click's y-coordinate to a list row inside `pane`. Row 0 sits just below
/// the pane's top border. Returns `None` for clicks on the border or past the
/// last item (so the click still focuses the pane without moving the selection).
fn row_at(pane: Rect, y: u16, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let first = pane.y.saturating_add(1);
    if y < first {
        return None;
    }
    let idx = (y - first) as usize;
    (idx < len).then_some(idx)
}

/// Subsequence fuzzy match. `needle` and `haystack` must already be lowercased.
/// `None` when `needle` isn't a subsequence of `haystack`; otherwise a score
/// where higher is better (contiguous runs and early matches score more).
fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.chars().collect();
    let mut hi = 0usize;
    let mut score = 0i32;
    let mut prev_match = false;
    let mut first: Option<usize> = None;

    for nc in needle.chars() {
        let mut found = false;
        while hi < hay.len() {
            let hc = hay[hi];
            hi += 1;
            if hc == nc {
                found = true;
                first.get_or_insert(hi - 1);
                score += 1;
                if prev_match {
                    score += 3;
                }
                prev_match = true;
                break;
            }
            prev_match = false;
        }
        if !found {
            return None;
        }
    }

    if let Some(f) = first {
        score += 10 - f.min(10) as i32;
    }
    score -= haystack.chars().count() as i32 / 20;
    Some(score)
}

fn step(idx: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let max = len as i32 - 1;
    (idx as i32 + delta).clamp(0, max) as usize
}

/// A `#tag` token: `#` followed by a letter (so issue refs like `#42` stay in
/// the title). The tag body is cleaned later by `model::normalize_tag`.
fn is_tag_token(tok: &str) -> bool {
    let mut c = tok.chars();
    c.next() == Some('#') && c.next().is_some_and(|c| c.is_ascii_alphabetic())
}

/// Parse `buy milk !3 @2026-09-01 #chores` into (title, priority, due, tags).
fn parse_todo_input(raw: &str) -> (String, Priority, Option<NaiveDate>, Vec<String>) {
    let mut priority = Priority::Medium;
    let mut due = None;
    let mut tags: Vec<&str> = Vec::new();
    let mut words = Vec::new();

    for tok in raw.split_whitespace() {
        if let Some(rest) = tok.strip_prefix('@')
            && let Ok(d) = NaiveDate::parse_from_str(rest, "%Y-%m-%d")
        {
            due = Some(d);
            continue;
        }
        if is_tag_token(tok) {
            tags.push(tok);
            continue;
        }
        match tok {
            "!1" | "!low" => {
                priority = Priority::Low;
                continue;
            }
            "!2" | "!med" | "!medium" => {
                priority = Priority::Medium;
                continue;
            }
            "!3" | "!!" | "!high" => {
                priority = Priority::High;
                continue;
            }
            _ => {}
        }
        words.push(tok);
    }

    (
        words.join(" ").trim().to_string(),
        priority,
        due,
        crate::model::normalize_tags(tags),
    )
}

/// Parse `Launch v1 @2026-09-15` into (title, date), defaulting to `fallback`.
fn parse_milestone_input(raw: &str, fallback: NaiveDate) -> Option<(String, NaiveDate)> {
    let mut date = fallback;
    let mut words = Vec::new();

    for tok in raw.split_whitespace() {
        if tok == "|" {
            continue;
        }
        if let Some(rest) = tok.strip_prefix('@')
            && let Ok(d) = NaiveDate::parse_from_str(rest, "%Y-%m-%d")
        {
            date = d;
            continue;
        }
        words.push(tok);
    }

    let title = words.join(" ").trim().to_string();
    if title.is_empty() {
        None
    } else {
        Some((title, date))
    }
}

/// Parse `refactor auth !3 #api` into (title, priority, tags). `!1`/`!2`/`!3`
/// (also `!low`/`!med`/`!high`, `!!`) set the priority; `#tag` tokens are tags;
/// both drop out of the title.
fn parse_priority_input(raw: &str) -> (String, Priority, Vec<String>) {
    let mut priority = Priority::Medium;
    let mut tags: Vec<&str> = Vec::new();
    let mut words = Vec::new();
    for tok in raw.split_whitespace() {
        if is_tag_token(tok) {
            tags.push(tok);
            continue;
        }
        match tok {
            "!1" | "!low" => priority = Priority::Low,
            "!2" | "!med" | "!medium" => priority = Priority::Medium,
            "!3" | "!!" | "!high" => priority = Priority::High,
            _ => words.push(tok),
        }
    }
    (
        words.join(" ").trim().to_string(),
        priority,
        crate::model::normalize_tags(tags),
    )
}

/// Parse a project name line, pulling out `#tag` tokens: `Website #web #api`.
fn parse_tagged_name(raw: &str) -> (String, Vec<String>) {
    let mut tags: Vec<&str> = Vec::new();
    let mut words = Vec::new();
    for tok in raw.split_whitespace() {
        if is_tag_token(tok) {
            tags.push(tok);
        } else {
            words.push(tok);
        }
    }
    (
        words.join(" ").trim().to_string(),
        crate::model::normalize_tags(tags),
    )
}

/// `!1` for low, `!3` for high — the suffix `parse_priority_input` understands.
fn priority_suffix(p: Priority) -> &'static str {
    match p {
        Priority::Low => " !1",
        Priority::High => " !3",
        Priority::Medium => "",
    }
}

/// ` #tag #tag2` — the suffix the input parsers round-trip.
fn tags_suffix(tags: &[String]) -> String {
    tags.iter().map(|t| format!(" #{t}")).collect()
}

fn subtask_edit_string(s: &Subtask) -> String {
    format!(
        "{}{}{}",
        s.title,
        priority_suffix(s.priority),
        tags_suffix(&s.tags)
    )
}

fn todo_edit_string(t: &Todo) -> String {
    let mut s = t.title.clone();
    s.push_str(priority_suffix(t.priority));
    if let Some(d) = t.due {
        s.push_str(&format!(" @{}", d.format("%Y-%m-%d")));
    }
    s.push_str(&tags_suffix(&t.tags));
    s
}

fn project_edit_string(p: &Project) -> String {
    format!("{}{}", p.name, tags_suffix(&p.tags))
}

fn milestone_edit_string(m: &Milestone) -> String {
    format!("{} @{}", m.title, m.date.format("%Y-%m-%d"))
}

/// Worker-thread body for a sync push. Resolves a bare repo name against the
/// token's account, optionally creates the repo, then writes the data file
/// (retrying once with a fresh SHA on a conflict).
fn run_sync(
    token: &str,
    repo: &str,
    file: &str,
    json: &str,
    sha: Option<&str>,
    setup: bool,
) -> Result<SyncOk, String> {
    let (owner, name) = match crate::github::parse_repo_string(repo) {
        Some(pair) => pair,
        None => {
            let name = repo.trim().trim_matches('/').to_string();
            if name.is_empty() || name.contains('/') {
                return Err("invalid repo name".into());
            }
            (crate::github::authed_login(token)?, name)
        }
    };

    let client = SyncClient::new(token, &owner, &name, file)?;
    if setup {
        client.ensure_repo()?;
    }

    let new_sha = client.push(json, sha).or_else(|e| {
        if sha.is_none() || e.contains("409") || e.contains("422") || e.contains("conflict") {
            let latest = client.pull()?.map(|r| r.sha);
            client.push(json, latest.as_deref())
        } else {
            Err(e)
        }
    })?;

    Ok(SyncOk {
        repo: format!("{owner}/{name}"),
        sha: new_sha,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_clamps_within_bounds() {
        assert_eq!(step(0, -1, 5), 0);
        assert_eq!(step(4, 1, 5), 4);
        assert_eq!(step(2, 1, 5), 3);
        assert_eq!(step(2, -1_000_000, 5), 0);
        assert_eq!(step(2, 1_000_000, 5), 4);
        assert_eq!(step(3, 1, 0), 0); // empty list
    }

    #[test]
    fn parse_todo_extracts_priority_and_due() {
        let (title, prio, due, tags) = parse_todo_input("ship the release !3 @2026-09-15 #rel");
        assert_eq!(title, "ship the release");
        assert_eq!(prio, Priority::High);
        assert_eq!(due, NaiveDate::from_ymd_opt(2026, 9, 15));
        assert_eq!(tags, vec!["rel"]);
    }

    #[test]
    fn parse_todo_defaults_and_aliases() {
        let (title, prio, due, tags) = parse_todo_input("plain task");
        assert_eq!(title, "plain task");
        assert_eq!(prio, Priority::Medium);
        assert_eq!(due, None);
        assert!(tags.is_empty());

        let (.., prio, _, _) = parse_todo_input("do it !low");
        assert_eq!(prio, Priority::Low);
        let (.., prio, _, _) = parse_todo_input("do it !!");
        assert_eq!(prio, Priority::High);
    }

    #[test]
    fn parse_todo_keeps_invalid_date_token() {
        let (title, _, due, _) = parse_todo_input("mail @someone about it");
        assert_eq!(title, "mail @someone about it");
        assert_eq!(due, None);
    }

    #[test]
    fn parse_todo_tags_vs_issue_refs() {
        let (title, _, _, tags) = parse_todo_input("fix login #42 #auth-bug");
        assert_eq!(title, "fix login #42", "#42 is not a tag");
        assert_eq!(tags, vec!["auth-bug"]);
    }

    #[test]
    fn subtask_priority_parses_and_roundtrips() {
        let (title, prio, _) = parse_priority_input("wire up the API !3");
        assert_eq!(title, "wire up the API");
        assert_eq!(prio, Priority::High);

        let (title, prio, tags) = parse_priority_input("just a note #x #y #x");
        assert_eq!(title, "just a note");
        assert_eq!(prio, Priority::Medium);
        assert_eq!(tags, vec!["x", "y"], "cleaned + de-duped");

        let mut s = Subtask::new("polish copy", false);
        s.priority = Priority::Low;
        s.tags = vec!["ui".into()];
        let (title, prio, tags) = parse_priority_input(&subtask_edit_string(&s));
        assert_eq!(title, "polish copy");
        assert_eq!(prio, Priority::Low);
        assert_eq!(tags, vec!["ui"]);
    }

    #[test]
    fn parse_milestone_uses_fallback_when_no_date() {
        let fallback = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let (title, date) = parse_milestone_input("Launch v1", fallback).unwrap();
        assert_eq!(title, "Launch v1");
        assert_eq!(date, fallback);
    }

    #[test]
    fn parse_milestone_reads_explicit_date_and_rejects_empty() {
        let fallback = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let (title, date) = parse_milestone_input("Beta @2026-06-30", fallback).unwrap();
        assert_eq!(title, "Beta");
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 6, 30).unwrap());
        assert!(parse_milestone_input("@2026-06-30", fallback).is_none());
    }

    #[test]
    fn fuzzy_score_matches_subsequence_and_ranks() {
        assert!(fuzzy_score("abc", "xyz").is_none());
        assert!(fuzzy_score("abc", "aXbXc").is_some());
        // Contiguous run outranks a scattered match.
        let contiguous = fuzzy_score("side", "sidebar").unwrap();
        let scattered = fuzzy_score("side", "s i d e where").unwrap();
        assert!(contiguous > scattered);
        // Empty needle always matches.
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn todo_edit_string_roundtrips_through_parser() {
        let mut t = Todo::new("write docs");
        t.priority = Priority::High;
        t.due = NaiveDate::from_ymd_opt(2026, 3, 4);
        t.tags = vec!["docs".into(), "q3".into()];
        let (title, prio, due, tags) = parse_todo_input(&todo_edit_string(&t));
        assert_eq!(title, "write docs");
        assert_eq!(prio, Priority::High);
        assert_eq!(due, t.due);
        assert_eq!(tags, t.tags);
    }

    #[test]
    fn search_target_depth() {
        assert_eq!(SearchTarget::Project.depth(), 0);
        assert_eq!(SearchTarget::Todo(0).depth(), 1);
        assert_eq!(SearchTarget::Note(0).depth(), 1);
        assert_eq!(SearchTarget::Subtask { todo: 0, sub: 0 }.depth(), 2);
    }

    #[test]
    fn parse_tagged_name_splits_project_tags() {
        let (name, tags) = parse_tagged_name("Website Redesign #web #Frontend");
        assert_eq!(name, "Website Redesign");
        assert_eq!(tags, vec!["web", "frontend"]);
    }
}
