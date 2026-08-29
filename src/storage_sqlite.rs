//! SQLite storage backend for voido.

use crate::model::{Milestone, Note, Priority, Project, Store, Subtask, Todo};
use rusqlite::{Connection, params};

/// Bump when the schema changes and add a matching arm in `migrate`.
const SCHEMA_VERSION: i64 = 2;

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
                repo TEXT DEFAULT NULL
            );
            CREATE TABLE IF NOT EXISTS todos (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                done INTEGER DEFAULT 0,
                priority TEXT DEFAULT 'medium',
                due TEXT DEFAULT NULL,
                FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS subtasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                todo_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                done INTEGER DEFAULT 0,
                priority TEXT NOT NULL DEFAULT 'medium',
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
            .prepare("SELECT id, name, description, repo FROM projects ORDER BY id")
            .map_err(|e| e.to_string())?;
        let project_rows: Vec<(i64, String, String, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;

        for (pid, name, description, repo) in project_rows {
            let mut project = Project::new(&name);
            project.description = description;
            project.repo = repo;
            project.todos = self.load_todos(pid)?;
            project.notes = self.load_notes(pid)?;
            project.milestones = self.load_milestones(pid)?;
            store.projects.push(project);
        }

        Ok(store)
    }

    fn load_todos(&self, project_id: i64) -> Result<Vec<Todo>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, title, done, priority, due FROM todos WHERE project_id = ?1 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows: Vec<(i64, String, bool, String, Option<String>)> = stmt
            .query_map(params![project_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;

        let mut todos = Vec::with_capacity(rows.len());
        for (id, title, done, priority, due) in rows {
            let mut todo = Todo::new(&title);
            todo.done = done;
            todo.priority = Priority::from_label(&priority);
            todo.due = due.and_then(|d| chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok());
            todo.subtasks = self.load_subtasks(id)?;
            todos.push(todo);
        }
        Ok(todos)
    }

    fn load_subtasks(&self, todo_id: i64) -> Result<Vec<Subtask>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT title, done, priority FROM subtasks WHERE todo_id = ?1 ORDER BY id")
            .map_err(|e| e.to_string())?;
        stmt.query_map(params![todo_id], |row| {
            let mut sub = Subtask::new(row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0);
            sub.priority = Priority::from_label(&row.get::<_, String>(2)?);
            Ok(sub)
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())
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
    pub fn save(&self, store: &Store) -> Result<(), String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;

        tx.execute_batch(
            "DELETE FROM subtasks;
             DELETE FROM todos;
             DELETE FROM notes;
             DELETE FROM milestones;
             DELETE FROM projects;",
        )
        .map_err(|e| e.to_string())?;

        for project in &store.projects {
            tx.execute(
                "INSERT INTO projects (name, description, repo) VALUES (?1, ?2, ?3)",
                params![project.name, project.description, project.repo],
            )
            .map_err(|e| e.to_string())?;
            let pid = tx.last_insert_rowid();

            for todo in &project.todos {
                tx.execute(
                    "INSERT INTO todos (project_id, title, done, priority, due) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        pid,
                        todo.title,
                        todo.done as i64,
                        todo.priority.label(),
                        todo.due.map(|d| d.format("%Y-%m-%d").to_string()),
                    ],
                )
                .map_err(|e| e.to_string())?;
                let tid = tx.last_insert_rowid();

                for sub in &todo.subtasks {
                    tx.execute(
                        "INSERT INTO subtasks (todo_id, title, done, priority) VALUES (?1, ?2, ?3, ?4)",
                        params![tid, sub.title, sub.done as i64, sub.priority.label()],
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
