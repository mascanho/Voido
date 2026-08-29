//! SQLite storage backend for shiki.

use rusqlite::{Connection, params};
use crate::model::{Milestone, Note, Priority, Project, Store, Subtask, Todo};

pub struct SqliteStorage {
    conn: Connection,
}

impl SqliteStorage {
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        let storage = Self { conn };
        storage.create_tables()?;
        Ok(storage)
    }

    fn create_tables(&self) -> Result<(), String> {
        self.conn.execute_batch(
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
            );"
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn load(&self) -> Store {
        let mut store = Store::default();

        let mut stmt = self.conn
            .prepare("SELECT id, name, description, repo FROM projects ORDER BY id")
            .unwrap();
        let project_rows: Vec<(i64, String, String, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        for (pid, name, description, repo) in project_rows {
            let mut project = Project::new(&name);
            project.description = description;
            project.repo = repo;

            // Load todos
            let mut todo_stmt = self.conn
                .prepare("SELECT id, title, done, priority, due FROM todos WHERE project_id = ?1 ORDER BY id")
                .unwrap();
            let todos: Vec<Todo> = todo_stmt
                .query_map(params![pid], |row| {
                    let id: i64 = row.get(0)?;
                    let title: String = row.get(1)?;
                    let done: bool = row.get::<_, i64>(2)? != 0;
                    let priority: String = row.get(3)?;
                    let due: Option<String> = row.get(4)?;
                    Ok((id, title, done, priority, due))
                })
                .unwrap()
                .filter_map(|r| r.ok())
                .map(|(id, title, done, priority, due)| {
                    let mut todo = Todo::new(&title);
                    todo.done = done;
                    todo.priority = match priority.as_str() {
                        "low" => Priority::Low,
                        "high" => Priority::High,
                        _ => Priority::Medium,
                    };
                    todo.due = due.and_then(|d| chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok());

                    // Load subtasks
                    let mut sub_stmt = self.conn
                        .prepare("SELECT title, done FROM subtasks WHERE todo_id = ?1 ORDER BY id")
                        .unwrap();
                    todo.subtasks = sub_stmt
                        .query_map(params![id], |row| {
                            Ok(Subtask::new(
                                row.get::<_, String>(0)?,
                                row.get::<_, i64>(1)? != 0,
                            ))
                        })
                        .unwrap()
                        .filter_map(|r| r.ok())
                        .collect();

                    todo
                })
                .collect();
            project.todos = todos;

            // Load notes
            let mut note_stmt = self.conn
                .prepare("SELECT id, text, pinned, body FROM notes WHERE project_id = ?1 ORDER BY id")
                .unwrap();
            project.notes = note_stmt
                .query_map(params![pid], |row| {
                    let text: String = row.get(1)?;
                    let pinned: bool = row.get::<_, i64>(2)? != 0;
                    let body: String = row.get(3)?;
                    Ok(Note::new(text, pinned).with_body(body))
                })
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();

            // Load milestones
            let mut ms_stmt = self.conn
                .prepare("SELECT title, date, done FROM milestones WHERE project_id = ?1 ORDER BY id")
                .unwrap();
            project.milestones = ms_stmt
                .query_map(params![pid], |row| {
                    Ok(Milestone {
                        title: row.get(0)?,
                        date: chrono::NaiveDate::parse_from_str(
                            &row.get::<_, String>(1)?,
                            "%Y-%m-%d",
                        )
                        .unwrap_or_default(),
                        done: row.get::<_, i64>(2)? != 0,
                    })
                })
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();

            store.projects.push(project);
        }

        store
    }

    pub fn save(&self, store: &Store) -> Result<(), String> {
        self.conn.execute("DELETE FROM subtasks", []).map_err(|e| e.to_string())?;
        self.conn.execute("DELETE FROM todos", []).map_err(|e| e.to_string())?;
        self.conn.execute("DELETE FROM notes", []).map_err(|e| e.to_string())?;
        self.conn.execute("DELETE FROM milestones", []).map_err(|e| e.to_string())?;
        self.conn.execute("DELETE FROM projects", []).map_err(|e| e.to_string())?;

        for project in &store.projects {
            self.conn.execute(
                "INSERT INTO projects (name, description, repo) VALUES (?1, ?2, ?3)",
                params![project.name, project.description, project.repo],
            ).map_err(|e| e.to_string())?;

            let pid = self.conn.last_insert_rowid();

            for todo in &project.todos {
                self.conn.execute(
                    "INSERT INTO todos (project_id, title, done, priority, due) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        pid,
                        todo.title,
                        todo.done as i64,
                        todo.priority.label(),
                        todo.due.map(|d| d.format("%Y-%m-%d").to_string()),
                    ],
                ).map_err(|e| e.to_string())?;

                let tid = self.conn.last_insert_rowid();
                for sub in &todo.subtasks {
                    self.conn.execute(
                        "INSERT INTO subtasks (todo_id, title, done) VALUES (?1, ?2, ?3)",
                        params![tid, sub.title, sub.done as i64],
                    ).map_err(|e| e.to_string())?;
                }
            }

            for note in &project.notes {
                self.conn.execute(
                    "INSERT INTO notes (project_id, text, pinned, body) VALUES (?1, ?2, ?3, ?4)",
                    params![pid, note.text, note.pinned as i64, note.body],
                ).map_err(|e| e.to_string())?;
            }

            for ms in &project.milestones {
                self.conn.execute(
                    "INSERT INTO milestones (project_id, title, date, done) VALUES (?1, ?2, ?3, ?4)",
                    params![pid, ms.title, ms.date.format("%Y-%m-%d").to_string(), ms.done as i64],
                ).map_err(|e| e.to_string())?;
            }
        }

        Ok(())
    }
}
