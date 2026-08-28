//! Data model for shiki: projects, todos and timeline milestones.

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
}

impl Subtask {
    pub fn new(title: impl Into<String>, done: bool) -> Self {
        Self { title: title.into(), done }
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
}

impl Todo {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            done: false,
            priority: Priority::Medium,
            due: None,
            subtasks: Vec::new(),
        }
    }

    /// (done, total) subtask counts.
    pub fn subtask_progress(&self) -> (usize, usize) {
        (
            self.subtasks.iter().filter(|s| s.done).count(),
            self.subtasks.len(),
        )
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
    pub todos: Vec<Todo>,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub milestones: Vec<Milestone>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            todos: Vec::new(),
            notes: Vec::new(),
            milestones: Vec::new(),
        }
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
            due(prio(Todo::new("Design system in Figma"), Priority::High), day(3)),
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
            Milestone { title: "Design review".into(), date: day(5), done: false },
            Milestone { title: "Public launch".into(), date: day(21), done: false },
        ];

        let mut cli = Project::new("Shiki CLI");
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
        cli.milestones = vec![Milestone { title: "v0.1 release".into(), date: day(7), done: false }];

        Store { projects: vec![website, cli] }
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
