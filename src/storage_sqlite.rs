//! SQLite storage backend for voido.

use crate::model::{Attachment, Meeting, Milestone, Note, Priority, Project, Store, Subtask, Todo};
use chrono::NaiveDate;
use rusqlite::{Connection, params};

/// Bump when the schema changes and add a matching arm in `migrate`.
const SCHEMA_VERSION: i64 = 8;

/// A raw `projects` row: (id, name, description, repo, tags, created).
type ProjectRow = (i64, String, String, Option<String>, String, String);

/// A raw `todos` row before subtasks/attachments are attached:
/// (id, title, done, priority, due, note, tags).
type TodoRow = (i64, String, bool, String, Option<String>, String, String);

/// Tags round-trip through a single comma-joined column (they contain no commas).
fn split_tags(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

pub struct SqliteStorage {
    conn: Connection,
}

impl SqliteStorage {
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| e.to_string())?;
        let storage = Self { conn };
        storage.create_tables()?;
        storage.migrate()?;
        Ok(storage)
    }

    fn create_tables(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS projects (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                description TEXT DEFAULT '',
                repo TEXT DEFAULT NULL,
                tags TEXT NOT NULL DEFAULT '',
                created TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE IF NOT EXISTS todos (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                done INTEGER DEFAULT 0,
                priority TEXT DEFAULT 'medium',
                due TEXT DEFAULT NULL,
                note TEXT NOT NULL DEFAULT '',
                tags TEXT NOT NULL DEFAULT '',
                FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS attachments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                todo_id INTEGER NOT NULL,
                subtask_id INTEGER DEFAULT NULL,
                value TEXT NOT NULL,
                label TEXT NOT NULL DEFAULT '',
                FOREIGN KEY (todo_id) REFERENCES todos(id) ON DELETE CASCADE,
                FOREIGN KEY (subtask_id) REFERENCES subtasks(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS subtasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                todo_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                done INTEGER DEFAULT 0,
                priority TEXT NOT NULL DEFAULT 'medium',
                note TEXT NOT NULL DEFAULT '',
                tags TEXT NOT NULL DEFAULT '',
                FOREIGN KEY (todo_id) REFERENCES todos(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS notes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id INTEGER NOT NULL,
                text TEXT NOT NULL,
                pinned INTEGER DEFAULT 0,
                body TEXT DEFAULT '',
                FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS milestones (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                date TEXT NOT NULL,
                done INTEGER DEFAULT 0,
                FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS meetings (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                date TEXT NOT NULL,
                time TEXT DEFAULT NULL,
                attendees TEXT NOT NULL DEFAULT '',
                note TEXT NOT NULL DEFAULT '',
                held INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
            );",
            )
            .map_err(|e| e.to_string())
    }

    /// Apply any pending schema migrations, tracked via `PRAGMA user_version`.
    fn migrate(&self) -> Result<(), String> {
        let current: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;

        // Fresh database (no projects yet) — just stamp the current version.
        if current == 0 {
            let has_rows: bool = self
                .conn
                .query_row("SELECT EXISTS(SELECT 1 FROM projects)", [], |row| {
                    row.get::<_, i64>(0).map(|n| n != 0)
                })
                .map_err(|e| e.to_string())?;
            if !has_rows {
                self.set_version(SCHEMA_VERSION)?;
                return Ok(());
            }
        }

        let mut version = current;
        while version < SCHEMA_VERSION {
            match version {
                // 0 -> 1: no structural change; existing DBs are already compatible.
                0 => {}
                // 1 -> 2: subtasks gained a `priority`.
                1 => self
                    .conn
                    .execute_batch(
                        "ALTER TABLE subtasks ADD COLUMN priority TEXT NOT NULL DEFAULT 'medium';",
                    )
                    .map_err(|e| e.to_string())?,
                // 2 -> 3: todos gained a `note`; attachments table added.
                2 => self
                    .conn
                    .execute_batch(
                        "ALTER TABLE todos ADD COLUMN note TEXT NOT NULL DEFAULT '';
                         CREATE TABLE IF NOT EXISTS attachments (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            todo_id INTEGER NOT NULL,
                            value TEXT NOT NULL,
                            label TEXT NOT NULL DEFAULT '',
                            FOREIGN KEY (todo_id) REFERENCES todos(id) ON DELETE CASCADE
                         );",
                    )
                    .map_err(|e| e.to_string())?,
                // 3 -> 4: subtasks gained a `note`.
                3 => self
                    .conn
                    .execute_batch("ALTER TABLE subtasks ADD COLUMN note TEXT NOT NULL DEFAULT '';")
                    .map_err(|e| e.to_string())?,
                // 4 -> 5: attachments can belong to a subtask (NULL = todo-level).
                4 => self
                    .conn
                    .execute_batch(
                        "ALTER TABLE attachments ADD COLUMN subtask_id INTEGER DEFAULT NULL;",
                    )
                    .map_err(|e| e.to_string())?,
                // 5 -> 6: projects, todos and subtasks gained `tags`.
                5 => self
                    .conn
                    .execute_batch(
                        "ALTER TABLE projects ADD COLUMN tags TEXT NOT NULL DEFAULT '';
                         ALTER TABLE todos ADD COLUMN tags TEXT NOT NULL DEFAULT '';
                         ALTER TABLE subtasks ADD COLUMN tags TEXT NOT NULL DEFAULT '';",
                    )
                    .map_err(|e| e.to_string())?,
                // 6 -> 7: projects remember the day they were created, so they
                // can be ordered by it.
                6 => self
                    .conn
                    .execute_batch(
                        "ALTER TABLE projects ADD COLUMN created TEXT NOT NULL DEFAULT '';",
                    )
                    .map_err(|e| e.to_string())?,
                // 7 -> 8: projects gained meetings.
                7 => self
                    .conn
                    .execute_batch(
                        "CREATE TABLE IF NOT EXISTS meetings (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            project_id INTEGER NOT NULL,
                            title TEXT NOT NULL,
                            date TEXT NOT NULL,
                            time TEXT DEFAULT NULL,
                            attendees TEXT NOT NULL DEFAULT '',
                            note TEXT NOT NULL DEFAULT '',
                            held INTEGER NOT NULL DEFAULT 0,
                            FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
                         );",
                    )
                    .map_err(|e| e.to_string())?,
                other => return Err(format!("no migration path from schema version {other}")),
            }
            version += 1;
        }
        self.set_version(SCHEMA_VERSION)
    }

    fn set_version(&self, v: i64) -> Result<(), String> {
        self.conn
            .pragma_update(None, "user_version", v)
            .map_err(|e| e.to_string())
    }

    pub fn load(&self) -> Result<Store, String> {
        let mut store = Store::default();

        let mut stmt = self
            .conn
            .prepare("SELECT id, name, description, repo, tags, created FROM projects ORDER BY id")
            .map_err(|e| e.to_string())?;
        let project_rows: Vec<ProjectRow> = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;

        for (pid, name, description, repo, tags, created) in project_rows {
            let mut project = Project::new(&name);
            project.description = description;
            project.repo = repo;
            project.tags = split_tags(&tags);
            // Blank on rows written before the column existed — those keep the
            // today's-date `Project::new` gave them.
            if let Ok(d) = NaiveDate::parse_from_str(&created, "%Y-%m-%d") {
                project.created = d;
            }
            project.todos = self.load_todos(pid)?;
            project.notes = self.load_notes(pid)?;
            project.milestones = self.load_milestones(pid)?;
            project.meetings = self.load_meetings(pid)?;
            store.projects.push(project);
        }

        Ok(store)
    }

    fn load_todos(&self, project_id: i64) -> Result<Vec<Todo>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, title, done, priority, due, note, tags FROM todos WHERE project_id = ?1 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows: Vec<TodoRow> = stmt
            .query_map(params![project_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;

        let mut todos = Vec::with_capacity(rows.len());
        for (id, title, done, priority, due, note, tags) in rows {
            let mut todo = Todo::new(&title);
            todo.done = done;
            todo.priority = Priority::from_label(&priority);
            todo.due = due.and_then(|d| chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok());
            todo.note = note;
            todo.tags = split_tags(&tags);
            todo.subtasks = self.load_subtasks(id)?;
            todo.attachments = self.load_attachments("todo_id = ?1 AND subtask_id IS NULL", id)?;
            todos.push(todo);
        }
        Ok(todos)
    }

    /// Attachment rows matching `where_clause` (one `?1` bind of `id`), in insert
    /// order.
    fn load_attachments(&self, where_clause: &str, id: i64) -> Result<Vec<Attachment>, String> {
        let sql = format!("SELECT value, label FROM attachments WHERE {where_clause} ORDER BY id");
        let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;
        stmt.query_map(params![id], |row| {
            Ok(Attachment::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())
    }

    fn load_subtasks(&self, todo_id: i64) -> Result<Vec<Subtask>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, title, done, priority, note, tags FROM subtasks WHERE todo_id = ?1 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows: Vec<(i64, String, bool, String, String, String)> = stmt
            .query_map(params![todo_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;

        let mut subs = Vec::with_capacity(rows.len());
        for (id, title, done, priority, note, tags) in rows {
            let mut sub = Subtask::new(title, done);
            sub.priority = Priority::from_label(&priority);
            sub.note = note;
            sub.tags = split_tags(&tags);
            sub.attachments = self.load_attachments("subtask_id = ?1", id)?;
            subs.push(sub);
        }
        Ok(subs)
    }

    fn load_notes(&self, project_id: i64) -> Result<Vec<Note>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT text, pinned, body FROM notes WHERE project_id = ?1 ORDER BY id")
            .map_err(|e| e.to_string())?;
        stmt.query_map(params![project_id], |row| {
            let text: String = row.get(0)?;
            let pinned: bool = row.get::<_, i64>(1)? != 0;
            let body: String = row.get(2)?;
            Ok(Note::new(text, pinned).with_body(body))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())
    }

    fn load_milestones(&self, project_id: i64) -> Result<Vec<Milestone>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT title, date, done FROM milestones WHERE project_id = ?1 ORDER BY id")
            .map_err(|e| e.to_string())?;
        stmt.query_map(params![project_id], |row| {
            Ok(Milestone {
                title: row.get(0)?,
                date: chrono::NaiveDate::parse_from_str(&row.get::<_, String>(1)?, "%Y-%m-%d")
                    .unwrap_or_default(),
                done: row.get::<_, i64>(2)? != 0,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())
    }

    /// Rewrite the whole store in a single transaction: either every row lands
    /// or the on-disk database is left untouched.
    fn load_meetings(&self, project_id: i64) -> Result<Vec<Meeting>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT title, date, time, attendees, note, held FROM meetings WHERE project_id = ?1 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([project_id], |row| {
                Ok(Meeting {
                    title: row.get(0)?,
                    date: NaiveDate::parse_from_str(&row.get::<_, String>(1)?, "%Y-%m-%d")
                        .unwrap_or_else(|_| chrono::Local::now().date_naive()),
                    time: row.get::<_, Option<String>>(2)?.filter(|t| !t.is_empty()),
                    attendees: split_tags(&row.get::<_, String>(3)?),
                    note: row.get(4)?,
                    held: row.get::<_, i64>(5)? != 0,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    pub fn save(&self, store: &Store) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;

        tx.execute_batch(
            "DELETE FROM attachments;
             DELETE FROM subtasks;
             DELETE FROM todos;
             DELETE FROM notes;
             DELETE FROM milestones;
             DELETE FROM meetings;
             DELETE FROM projects;",
        )
        .map_err(|e| e.to_string())?;

        for project in &store.projects {
            tx.execute(
                "INSERT INTO projects (name, description, repo, tags, created) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    project.name,
                    project.description,
                    project.repo,
                    project.tags.join(","),
                    project.created.format("%Y-%m-%d").to_string(),
                ],
            )
            .map_err(|e| e.to_string())?;
            let pid = tx.last_insert_rowid();

            for todo in &project.todos {
                tx.execute(
                    "INSERT INTO todos (project_id, title, done, priority, due, note, tags) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        pid,
                        todo.title,
                        todo.done as i64,
                        todo.priority.label(),
                        todo.due.map(|d| d.format("%Y-%m-%d").to_string()),
                        todo.note,
                        todo.tags.join(","),
                    ],
                )
                .map_err(|e| e.to_string())?;
                let tid = tx.last_insert_rowid();

                for sub in &todo.subtasks {
                    tx.execute(
                        "INSERT INTO subtasks (todo_id, title, done, priority, note, tags) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            tid,
                            sub.title,
                            sub.done as i64,
                            sub.priority.label(),
                            sub.note,
                            sub.tags.join(","),
                        ],
                    )
                    .map_err(|e| e.to_string())?;
                    let sid = tx.last_insert_rowid();

                    for att in &sub.attachments {
                        tx.execute(
                            "INSERT INTO attachments (todo_id, subtask_id, value, label) VALUES (?1, ?2, ?3, ?4)",
                            params![tid, sid, att.value, att.label],
                        )
                        .map_err(|e| e.to_string())?;
                    }
                }

                for att in &todo.attachments {
                    tx.execute(
                        "INSERT INTO attachments (todo_id, value, label) VALUES (?1, ?2, ?3)",
                        params![tid, att.value, att.label],
                    )
                    .map_err(|e| e.to_string())?;
                }
            }

            for note in &project.notes {
                tx.execute(
                    "INSERT INTO notes (project_id, text, pinned, body) VALUES (?1, ?2, ?3, ?4)",
                    params![pid, note.text, note.pinned as i64, note.body],
                )
                .map_err(|e| e.to_string())?;
            }

            for meeting in &project.meetings {
                tx.execute(
                    "INSERT INTO meetings (project_id, title, date, time, attendees, note, held) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        pid,
                        meeting.title,
                        meeting.date.format("%Y-%m-%d").to_string(),
                        meeting.time,
                        meeting.attendees.join(","),
                        meeting.note,
                        meeting.held as i64,
                    ],
                )
                .map_err(|e| e.to_string())?;
            }

            for ms in &project.milestones {
                tx.execute(
                    "INSERT INTO milestones (project_id, title, date, done) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        pid,
                        ms.title,
                        ms.date.format("%Y-%m-%d").to_string(),
                        ms.done as i64
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
        }

        tx.commit().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Attachment, Meeting, Project, Subtask, Todo};

    #[test]
    fn round_trips_todo_note_and_attachments() {
        let dir = std::env::temp_dir().join(format!("voido-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        let db = SqliteStorage::open(&dir).unwrap();

        let mut store = Store::default();
        let mut p = Project::new("P");
        p.tags = vec!["work".into(), "q3".into()];
        let created = NaiveDate::from_ymd_opt(2025, 4, 9).unwrap();
        p.created = created;
        let mut held = Meeting::new("kickoff", NaiveDate::from_ymd_opt(2025, 4, 10).unwrap());
        held.time = Some("09:30".into());
        held.attendees = vec!["Ana".into(), "Sam".into()];
        held.note = "## Minutes\n\nShipped the plan.".to_string();
        held.held = true;
        p.meetings = vec![
            held,
            Meeting::new("retro", NaiveDate::from_ymd_opt(2025, 5, 2).unwrap()),
        ];
        let mut t = Todo::new("write docs");
        t.note = "# heading\n\nsome **body**".into();
        t.tags = vec!["docs".into()];
        t.attachments = vec![
            Attachment::new("https://example.com", "site"),
            Attachment::new("/tmp/pic.png", ""),
        ];
        let mut sub = Subtask::new("outline", false);
        sub.note = "cover the migration path".into();
        sub.tags = vec!["draft".into()];
        sub.attachments = vec![Attachment::new("https://rfc.example/1", "RFC")];
        t.subtasks.push(sub);
        p.todos.push(t);
        store.projects.push(p);

        db.save(&store).unwrap();
        let loaded = db.load().unwrap();
        let lt = &loaded.projects[0].todos[0];
        assert_eq!(lt.note, "# heading\n\nsome **body**");
        assert_eq!(lt.attachments.len(), 2);
        assert_eq!(lt.attachments[0].label, "site");
        assert_eq!(lt.attachments[1].value, "/tmp/pic.png");
        assert_eq!(lt.subtasks[0].note, "cover the migration path");
        assert_eq!(lt.subtasks[0].attachments.len(), 1);
        assert_eq!(lt.subtasks[0].attachments[0].label, "RFC");
        // Todo-level attachments must not leak into the subtask and vice versa.
        assert_eq!(lt.attachments.len(), 2);
        assert_eq!(loaded.projects[0].tags, vec!["work", "q3"]);
        assert_eq!(lt.tags, vec!["docs"]);
        assert_eq!(lt.subtasks[0].tags, vec!["draft"]);
        assert_eq!(loaded.projects[0].created, created);
        let lm = &loaded.projects[0].meetings;
        assert_eq!(lm.len(), 2);
        assert_eq!(lm[0].title, "kickoff");
        assert_eq!(lm[0].time.as_deref(), Some("09:30"));
        assert_eq!(lm[0].attendees, vec!["Ana", "Sam"]);
        assert_eq!(lm[0].note, "## Minutes\n\nShipped the plan.");
        assert!(lm[0].held);
        // A meeting with no time or attendees comes back blank, not empty-string.
        assert_eq!(lm[1].title, "retro");
        assert!(lm[1].time.is_none());
        assert!(lm[1].attendees.is_empty());
        assert!(!lm[1].held);

        let _ = std::fs::remove_file(&dir);
    }

    /// A database written before `projects.created` existed must gain the column
    /// on open, with the rows already in it left readable.
    #[test]
    fn migrates_a_pre_created_column_database() {
        let path = std::env::temp_dir().join(format!("voido-migrate-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE projects (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    description TEXT DEFAULT '',
                    repo TEXT DEFAULT NULL,
                    tags TEXT NOT NULL DEFAULT ''
                 );
                 INSERT INTO projects (name, tags) VALUES ('Old', 'work');",
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 6).unwrap();
        }

        let db = SqliteStorage::open(&path).unwrap();
        let loaded = db.load().unwrap();
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].name, "Old");
        assert_eq!(loaded.projects[0].tags, vec!["work"]);
        // No stored date — the project keeps the one `Project::new` gave it.
        assert_eq!(
            loaded.projects[0].created,
            chrono::Local::now().date_naive()
        );
        // And it round-trips once saved through the new column.
        db.save(&loaded).unwrap();
        assert_eq!(
            db.load().unwrap().projects[0].created,
            chrono::Local::now().date_naive()
        );

        let _ = std::fs::remove_file(&path);
    }
}
