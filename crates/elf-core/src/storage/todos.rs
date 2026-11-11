//! Local todos created by the `create_todos` tool (handbook section 2: dates the model
//! is unsure about are stored as uncertain, never guessed).

use std::fmt;

use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::{conv, get_opt_ts, get_ts, Store};
use crate::schema::{str_enum, ts_to_sql, ElfError, Result, TaskId, Timestamp, TodoId};

str_enum!(
    /// Lifecycle of a todo.
    TodoStatus {
        /// Still to do.
        Open => "open",
        /// Done.
        Done => "done",
        /// Dropped.
        Cancelled => "cancelled",
    }
);

/// A stored todo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    /// Identifier.
    pub id: TodoId,
    /// Task that created it.
    pub task_id: TaskId,
    /// What to do.
    pub title: String,
    /// Due instant; with `due_uncertain` it is only a tentative value (or absent).
    pub due_at: Option<Timestamp>,
    /// The due date is not certain and needs the user's confirmation.
    pub due_uncertain: bool,
    /// The original wording of the date (e.g. "next Friday?"), kept for the user.
    pub due_text: Option<String>,
    /// Current status.
    pub status: TodoStatus,
    /// Where it came from (e.g. a line of the meeting notes).
    pub source_ref: Option<String>,
    /// Creation time.
    pub created_at: Timestamp,
}

/// Parameters for a new todo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTodo {
    /// What to do (non-empty).
    pub title: String,
    /// Due instant.
    pub due_at: Option<Timestamp>,
    /// Whether the date is uncertain.
    pub due_uncertain: bool,
    /// Original wording of the date.
    pub due_text: Option<String>,
    /// Source reference.
    pub source_ref: Option<String>,
}

const TODO_COLS: &str =
    "id, task_id, title, due_at, due_uncertain, due_text, status, source_ref, created_at";

fn todo_from_row(r: &Row<'_>) -> rusqlite::Result<Todo> {
    let status: String = r.get(6)?;
    Ok(Todo {
        id: TodoId::from_string(r.get::<_, String>(0)?),
        task_id: TaskId::from_string(r.get::<_, String>(1)?),
        title: r.get(2)?,
        due_at: get_opt_ts(r, 3)?,
        due_uncertain: r.get::<_, i64>(4)? != 0,
        due_text: r.get(5)?,
        status: conv(6, TodoStatus::parse(&status))?,
        source_ref: r.get(7)?,
        created_at: get_ts(r, 8)?,
    })
}

impl Store {
    /// Inserts todos for a task in one transaction (all or none).
    pub fn insert_todos(
        &mut self,
        task_id: &TaskId,
        todos: &[NewTodo],
        now: Timestamp,
    ) -> Result<Vec<Todo>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut ids = Vec::with_capacity(todos.len());
        for t in todos {
            if t.title.trim().is_empty() {
                return Err(ElfError::invalid_input("todo title is empty"));
            }
            let id = TodoId::new();
            tx.execute(
                "INSERT INTO todos (id, task_id, title, due_at, due_uncertain, due_text, status, source_ref, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?8)",
                params![
                    id.as_str(),
                    task_id.as_str(),
                    t.title,
                    t.due_at.as_ref().map(ts_to_sql),
                    i64::from(t.due_uncertain),
                    t.due_text,
                    t.source_ref,
                    ts_to_sql(&now)
                ],
            )?;
            ids.push(id);
        }
        tx.commit()?;
        ids.iter().map(|id| self.get_todo(id)).collect()
    }

    /// Loads a todo.
    pub fn get_todo(&self, id: &TodoId) -> Result<Todo> {
        self.conn
            .query_row(
                &format!("SELECT {TODO_COLS} FROM todos WHERE id = ?1"),
                [id.as_str()],
                todo_from_row,
            )
            .optional()?
            .ok_or_else(|| ElfError::invalid_input(format!("unknown todo {id}")))
    }

    /// Todos in creation order, optionally only those of one task.
    pub fn list_todos(&self, task_id: Option<&TaskId>) -> Result<Vec<Todo>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TODO_COLS} FROM todos WHERE (?1 IS NULL OR task_id = ?1) ORDER BY rowid"
        ))?;
        let rows = stmt.query_map([task_id.map(TaskId::as_str)], todo_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Sets a todo's status (the user ticks it off in the UI).
    pub fn set_todo_status(&mut self, id: &TodoId, status: TodoStatus) -> Result<Todo> {
        let n = self.conn.execute(
            "UPDATE todos SET status = ?2 WHERE id = ?1",
            params![id.as_str(), status.as_str()],
        )?;
        if n == 0 {
            return Err(ElfError::invalid_input(format!("unknown todo {id}")));
        }
        self.get_todo(id)
    }
}
