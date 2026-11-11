//! `create_todos`: stores todos locally. Dates the model is unsure about are stored as
//! uncertain, never guessed.

use std::time::Duration;

use async_trait::async_trait;
use chrono::DateTime;
use serde_json::{json, Value};

use super::{Tool, ToolContext, ToolOutput};
use crate::policy::{Action, ActionClass, Resource};
use crate::schema::{LumaError, Result, Trust};
use crate::storage::NewTodo;
use crate::task::SideEffect;

/// Name of the policy action and resource used for local todos.
pub const TODOS_ACTION: &str = "create_todos";
/// Resource string of the local todo list.
pub const TODOS_RESOURCE: &str = "local:todos";

/// Most todos accepted in one call.
const MAX_TODOS: usize = 50;

/// The `create_todos` tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct CreateTodosTool;

fn parse_todo(item: &Value) -> Result<NewTodo> {
    let title = item
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| LumaError::invalid_input("todo title is empty"))?
        .to_owned();
    let text = |k: &str| {
        item.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let due_text = text("due_text");
    let due_uncertain = item
        .get("due_uncertain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let due_at = match item.get("due_at").and_then(Value::as_str) {
        None => None,
        Some(raw) => Some(
            DateTime::parse_from_rfc3339(raw)
                .map_err(|e| {
                    LumaError::invalid_input(format!(
                        "due_at {raw:?} for {title:?} is not an RFC 3339 date-time with a time zone ({e}); \
                         if the date is not certain, set due_uncertain=true and put the wording in due_text"
                    ))
                })?
                .with_timezone(&chrono::Utc),
        ),
    };
    if due_uncertain && due_at.is_none() && due_text.is_none() {
        return Err(LumaError::invalid_input(format!(
            "todo {title:?} is marked due_uncertain but gives neither a tentative due_at nor due_text"
        )));
    }
    Ok(NewTodo {
        title,
        due_at,
        due_uncertain,
        due_text,
        source_ref: text("source_ref"),
    })
}

#[async_trait]
impl Tool for CreateTodosTool {
    fn name(&self) -> &str {
        "create_todos"
    }

    fn description(&self) -> &str {
        "Save todo items to the user's local todo list. Give due_at (RFC 3339 with time zone) \
         only when the date is certain. If the date is vague or you are unsure, set \
         due_uncertain=true and copy the original wording into due_text instead of guessing; \
         the user will confirm it. Use source_ref to point at where the item came from."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["todos"],
            "properties": {
                "todos": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_TODOS,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["title"],
                        "properties": {
                            "title": {"type": "string", "minLength": 1, "maxLength": 200},
                            "due_at": {"type": "string", "format": "date-time"},
                            "due_uncertain": {"type": "boolean"},
                            "due_text": {"type": "string", "maxLength": 200},
                            "source_ref": {"type": "string", "maxLength": 200}
                        }
                    }
                }
            }
        })
    }

    fn required_action(&self) -> Action {
        Action::Other(TODOS_ACTION.to_owned())
    }

    fn action_class(&self) -> ActionClass {
        ActionClass::LocalRecord
    }

    fn side_effect(&self) -> SideEffect {
        SideEffect::LocalWrite
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(10)
    }

    fn max_output_bytes(&self) -> usize {
        16 * 1024
    }

    fn resource(&self, _args: &Value, _ctx: &ToolContext) -> Result<Resource> {
        Ok(Resource::Other(TODOS_RESOURCE.to_owned()))
    }

    fn preview(&self, args: &Value, _resource: &Resource) -> String {
        let titles: Vec<&str> = args
            .get("todos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|t| t.get("title").and_then(Value::as_str))
            .collect();
        format!(
            "Add {} item(s) to the local todo list: {}",
            titles.len(),
            titles.join("; ")
        )
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let items = args
            .get("todos")
            .and_then(Value::as_array)
            .ok_or_else(|| LumaError::invalid_input("todos must be an array"))?;
        let parsed = items.iter().map(parse_todo).collect::<Result<Vec<_>>>()?;
        let now = ctx.now();
        // Keyed by this call's idempotency key: if the rows were inserted but the step was
        // never finished (crash), the re-run finds them instead of inserting them again.
        let stored = ctx.store.with(|s| {
            s.insert_todos_keyed(&ctx.task_id, Some(&ctx.idempotency_key), &parsed, now)
        })?;
        // read back: the result only reports what the database actually holds
        let verified = ctx.store.with(|s| {
            stored
                .iter()
                .map(|t| s.get_todo(&t.id))
                .collect::<Result<Vec<_>>>()
        })?;
        if verified != stored {
            return Err(LumaError::ExternalFailure {
                message: "stored todos differ from what was written".into(),
                retryable: false,
            });
        }
        let needs_confirmation = verified.iter().filter(|t| t.due_uncertain).count();
        let content = json!({
            "created": verified.iter().map(|t| json!({
                "id": t.id,
                "title": t.title,
                "due_at": t.due_at,
                "due_uncertain": t.due_uncertain,
                "due_text": t.due_text,
            })).collect::<Vec<_>>(),
            "count": verified.len(),
            "dates_needing_confirmation": needs_confirmation,
        });
        Ok(ToolOutput {
            content: content.to_string(),
            trust: Trust::UserProvided, // app-generated confirmation, no third-party content
            evidence: None,
        })
    }
}
