//! Data model for voido: projects, todos and timeline milestones.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    #[default]
    Medium,
    High,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Priority::Low => "low",
            Priority::Medium => "med",
            Priority::High => "high",
        }
    }

    /// Parse a stored label back into a `Priority` (anything unknown -> Medium).
    pub fn from_label(s: &str) -> Self {
        match s {
            "low" => Priority::Low,
            "high" => Priority::High,
            _ => Priority::Medium,
        }
    }

    /// Sort weight, highest-priority first (High = 0).
    pub fn rank(self) -> u8 {
        match self {
            Priority::High => 0,
            Priority::Medium => 1,
            Priority::Low => 2,
        }
    }

    /// Cycle low -> med -> high -> low.
    pub fn next(self) -> Self {
        match self {
            Priority::Low => Priority::Medium,
            Priority::Medium => Priority::High,
            Priority::High => Priority::Low,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subtask {
    pub title: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub priority: Priority,
    /// Free-form Markdown note attached to this subtask.
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

impl Subtask {
    pub fn new(title: impl Into<String>, done: bool) -> Self {
        Self {
            title: title.into(),
            done,
            priority: Priority::Medium,
            note: String::new(),
            attachments: Vec::new(),
        }
    }
}

/// A link, file or image attached to a todo. The kind is inferred from `value`
/// (a `http(s)://` prefix -> link; an image file extension -> image; otherwise a
/// plain file path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// URL or filesystem path.
    pub value: String,
    /// Optional display label; falls back to `value` when empty.
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Link,
    Image,
    File,
}

impl Attachment {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }

    pub fn kind(&self) -> AttachmentKind {
        let v = self.value.trim();
        if v.starts_with("http://") || v.starts_with("https://") || v.starts_with("www.") {
            return AttachmentKind::Link;
        }
        let lower = v.to_ascii_lowercase();
        let is_image = [
            ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".bmp", ".tiff", ".heic",
        ]
        .iter()
        .any(|ext| lower.ends_with(ext));
        if is_image {
            AttachmentKind::Image
        } else {
            AttachmentKind::File
        }
    }

    /// What to show for this attachment: the label, or the value when unlabelled.
    pub fn display(&self) -> &str {
        if self.label.trim().is_empty() {
            &self.value
        } else {
            &self.label
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    pub title: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub due: Option<NaiveDate>,
    #[serde(default)]
    pub subtasks: Vec<Subtask>,
    /// Free-form Markdown note attached to this todo.
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

impl Todo {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            done: false,
            priority: Priority::Medium,
            due: None,
            subtasks: Vec::new(),
            note: String::new(),
            attachments: Vec::new(),
        }
    }

    /// (done, total) subtask counts.
    pub fn subtask_progress(&self) -> (usize, usize) {
        (
            self.subtasks.iter().filter(|s| s.done).count(),
            self.subtasks.len(),
        )
    }

    /// Keep `done` in lockstep with the subtasks: a todo that has subtasks is
    /// done exactly when every one of them is. A todo with no subtasks keeps
    /// whatever `done` state it was given. Returns `true` if `done` changed.
    pub fn recompute_done(&mut self) -> bool {
        if self.subtasks.is_empty() {
            return false;
        }
        let all_done = self.subtasks.iter().all(|s| s.done);
        let changed = self.done != all_done;
        self.done = all_done;
        changed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Milestone {
    pub title: String,
    pub date: NaiveDate,
    #[serde(default)]
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    /// One-line title shown in the notes list.
    pub text: String,
    #[serde(default)]
    pub pinned: bool,
    /// Markdown body, rendered in the pane beside the note.
    #[serde(default)]
    pub body: String,
}

impl Note {
    pub fn new(text: impl Into<String>, pinned: bool) -> Self {
        Self {
            text: text.into(),
            pinned,
            body: String::new(),
        }
    }

    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub todos: Vec<Todo>,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub milestones: Vec<Milestone>,
    #[serde(default = "default_date")]
    pub created: NaiveDate,
}

fn default_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            repo: None,
            todos: Vec::new(),
            notes: Vec::new(),
            milestones: Vec::new(),
            created: chrono::Local::now().date_naive(),
        }
    }

    /// A project reads as complete only once it has at least one tracked item
    /// and every todo, subtask and milestone in it is done.
    pub fn is_complete(&self) -> bool {
        let has_items = !self.todos.is_empty() || !self.milestones.is_empty();
        has_items
            && self
                .todos
                .iter()
                .all(|t| t.done && t.subtasks.iter().all(|s| s.done))
            && self.milestones.iter().all(|m| m.done)
    }

    pub fn open_todos(&self) -> usize {
        self.todos.iter().filter(|t| !t.done).count()
    }

    pub fn done_todos(&self) -> usize {
        self.todos.iter().filter(|t| t.done).count()
    }

    /// The soonest not-done milestone, if any.
    pub fn next_milestone(&self) -> Option<&Milestone> {
        self.milestones
            .iter()
            .filter(|m| !m.done)
            .min_by_key(|m| m.date)
    }

    /// (done, total) subtask counts across all todos.
    pub fn subtask_progress(&self) -> (usize, usize) {
        let total = self.todos.iter().map(|t| t.subtasks.len()).sum();
        let done = self
            .todos
            .iter()
            .flat_map(|t| &t.subtasks)
            .filter(|s| s.done)
            .count();
        (done, total)
    }

    pub fn note_count(&self) -> usize {
        self.notes.len()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Store {
    pub projects: Vec<Project>,
}

impl Store {
    /// A friendly first-run dataset so the app never opens empty.
    pub fn sample() -> Self {
        use chrono::{Duration, Local};

        let today = Local::now().date_naive();
        let day = |n: i64| today + Duration::days(n);

        let mut website = Project::new("Website Redesign");
        website.description = "Marketing site refresh".into();
        let mut home = Todo::new("Rebuild the home page");
        home.subtasks = vec![
            Subtask::new("Hero section", true),
            Subtask::new("Nav + footer", false),
            Subtask::new("Mobile breakpoints", false),
        ];
        website.todos = vec![
            done(Todo::new("Audit current pages")),
            due(
                prio(Todo::new("Design system in Figma"), Priority::High),
                day(3),
            ),
            home,
            due(Todo::new("Ship to staging"), day(9)),
        ];
        let hero_body = "\
# Hero direction

Stakeholder feedback from the **Q3 review** — the current hero reads as a
brochure, not a product.

## Must have

- Full-bleed image, *no carousel*
- Headline set in `Inter Display`, 64px
- Primary CTA above the fold on mobile

## Open questions

1. Do we keep the announcement bar?
2. Video background — worth the weight budget?

> \"Make it feel like something you'd want to use every day.\"

See the [thread](mailto:team@example.com) for the full notes.
";
        website.notes = vec![
            Note::new("Bolder hero — Q3 review notes", true).with_body(hero_body),
            Note::new("Reuse the icon set from the app", false),
        ];
        website.milestones = vec![
            Milestone {
                title: "Design review".into(),
                date: day(5),
                done: false,
            },
            Milestone {
                title: "Public launch".into(),
                date: day(21),
                done: false,
            },
        ];

        let mut cli = Project::new("Voido CLI");
        cli.description = "A keyboard-first TUI for todos, projects and timelines".into();
        cli.todos = vec![
            done(Todo::new("Vim-style navigation")),
            prio(Todo::new("Timeline view"), Priority::High),
            Todo::new("Package for Homebrew"),
        ];
        cli.notes = vec![
            Note::new("Keymap principles", true).with_body(
                "# Keymap\n\nKeep it **close to Vim** — no surprises.\n\n- `hjkl` everywhere\n- `gg` / `G` to jump\n- `d` always asks first\n",
            ),
        ];
        cli.milestones = vec![Milestone {
            title: "v0.1 release".into(),
            date: day(7),
            done: false,
        }];

        Store {
            projects: vec![website, cli],
        }
    }
}

fn done(mut t: Todo) -> Todo {
    t.done = true;
    t
}

fn prio(mut t: Todo, p: Priority) -> Todo {
    t.priority = p;
    t
}

fn due(mut t: Todo, d: NaiveDate) -> Todo {
    t.due = Some(d);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_kind_inference() {
        assert_eq!(
            Attachment::new("https://example.com", "").kind(),
            AttachmentKind::Link
        );
        assert_eq!(
            Attachment::new("/home/me/diagram.PNG", "").kind(),
            AttachmentKind::Image
        );
        assert_eq!(
            Attachment::new("./notes/spec.pdf", "").kind(),
            AttachmentKind::File
        );
    }

    #[test]
    fn attachment_display_prefers_label() {
        assert_eq!(Attachment::new("/x/y.pdf", "Spec").display(), "Spec");
        assert_eq!(Attachment::new("/x/y.pdf", "  ").display(), "/x/y.pdf");
    }

    #[test]
    fn priority_rank_orders_high_first() {
        let mut v = [Priority::Low, Priority::High, Priority::Medium];
        v.sort_by_key(|p| p.rank());
        assert_eq!(v, [Priority::High, Priority::Medium, Priority::Low]);
    }

    #[test]
    fn recompute_done_follows_subtasks() {
        let mut t = Todo::new("parent");
        assert!(!t.recompute_done(), "no subtasks -> untouched");
        assert!(!t.done);

        t.subtasks = vec![Subtask::new("a", true), Subtask::new("b", false)];
        assert!(!t.recompute_done());
        assert!(!t.done, "one subtask open -> parent open");

        t.subtasks[1].done = true;
        assert!(t.recompute_done(), "all done -> parent flips to done");
        assert!(t.done);

        t.subtasks.push(Subtask::new("c", false));
        assert!(t.recompute_done(), "new open subtask -> parent reopens");
        assert!(!t.done);
    }

    #[test]
    fn is_complete_needs_every_child_done() {
        let empty = Project::new("empty");
        assert!(!empty.is_complete(), "no items -> not complete");

        let mut p = Project::new("P");
        let mut t1 = Todo::new("t1");
        t1.subtasks = vec![Subtask::new("s1", true), Subtask::new("s2", false)];
        let mut t2 = Todo::new("t2");
        t2.done = true;
        p.todos = vec![t1, t2];
        p.milestones = vec![Milestone {
            title: "m".into(),
            date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            done: true,
        }];

        assert!(!p.is_complete(), "t1 not done, s2 not done");

        p.todos[0].done = true;
        assert!(!p.is_complete(), "s2 still not done");

        p.todos[0].subtasks[1].done = true;
        assert!(p.is_complete(), "everything done now");

        p.milestones[0].done = false;
        assert!(!p.is_complete(), "milestone reopened");
    }
}
