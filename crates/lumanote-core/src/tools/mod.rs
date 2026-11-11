//! Tool registry (handbook sections 3, 7.5, 8): schemas, permissions, timeouts and the
//! tools themselves. The registry only describes and runs tools; whether a call may run
//! is decided by [`crate::policy`] and driven by [`crate::agent`].

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::model::ToolSpec;
use crate::policy::{Action, ActionClass, Resource};
use crate::ports::{Clock, CredentialStore, HttpTransport, Notifier};
use crate::schema::{LumaError, EvidenceRef, Result, TaskId, Timestamp, Trust};
use crate::storage::SharedStore;
use crate::task::SideEffect;

pub mod create_todos;
pub mod fsutil;
pub mod read_file;
mod scope;
pub mod validate;
pub mod write_markdown;

#[cfg(test)]
mod tests;

pub use create_todos::CreateTodosTool;
pub use read_file::ReadFileTool;
pub use scope::TaskScope;
use validate::ArgsValidator;
pub use write_markdown::WriteMarkdownTool;

/// External capabilities a tool may use (all injected, none required).
#[derive(Clone)]
pub struct Ports {
    /// Time source; tools must never read the OS clock.
    pub clock: Arc<dyn Clock>,
    /// User notifications.
    pub notifier: Option<Arc<dyn Notifier>>,
    /// Network access.
    pub http: Option<Arc<dyn HttpTransport>>,
    /// Secrets.
    pub credentials: Option<Arc<dyn CredentialStore>>,
}

impl Ports {
    /// Ports with only a clock.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            notifier: None,
            http: None,
            credentials: None,
        }
    }
}

/// Everything a tool can see while it runs.
#[derive(Clone)]
pub struct ToolContext {
    /// The task the call belongs to.
    pub task_id: TaskId,
    /// Database access.
    pub store: SharedStore,
    /// Injected external capabilities.
    pub ports: Ports,
    /// Cancelled when the task is cancelled; long tools must watch it.
    pub cancel: CancellationToken,
    /// The task's declared scope (output and read directories).
    pub scope: TaskScope,
    /// Idempotency key of this call; the same key is never executed twice.
    pub idempotency_key: String,
}

impl ToolContext {
    /// Current time from the injected clock.
    pub fn now(&self) -> Timestamp {
        self.ports.clock.now_utc()
    }
}

/// What a finished tool returns to the orchestrator.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// Text handed to the model (usually JSON).
    pub content: String,
    /// How far the content can be trusted; `External` flags the task as untrusted.
    pub trust: Trust,
    /// Proof of what the tool did (e.g. the written artifact), if it produced any.
    pub evidence: Option<EvidenceRef>,
}

/// A capability the model can call. Implementations hold no per-task state.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique name shown to the model (`[A-Za-z_][A-Za-z0-9_-]*`, at most 64 characters).
    fn name(&self) -> &str;
    /// What the tool does, written for the model.
    fn description(&self) -> &str;
    /// JSON Schema of the argument object (`"type": "object"`).
    fn parameters_schema(&self) -> Value;
    /// The permission action the policy engine must grant.
    fn required_action(&self) -> Action;
    /// How strictly policy must treat this tool (read, local record, local write,
    /// external write, send, publish, payment, delete). There is deliberately **no
    /// default**: a tool cannot be written, or registered, without declaring its class. An
    /// unclassified action would count as an external write
    /// ([`crate::policy::Action::class`]). [`Registry::register`] checks the declaration
    /// against [`Tool::required_action`] and [`Tool::side_effect`].
    fn action_class(&self) -> ActionClass;
    /// Side-effect class, recorded in the write-ahead step.
    fn side_effect(&self) -> SideEffect;
    /// Trust of the text of an error this tool returns. Errors of a tool that talks to the
    /// outside may carry third-party text (an HTTP body, a server message), so such a tool
    /// returns [`Trust::External`] here. The default is [`Trust::UserProvided`]: the
    /// message was written by this application.
    fn error_trust(&self) -> Trust {
        Trust::UserProvided
    }
    /// Longest the tool may run before the orchestrator gives up on it.
    fn timeout(&self) -> Duration;
    /// Largest result handed to the model; longer output is cut and flagged.
    fn max_output_bytes(&self) -> usize;
    /// False for placeholders that are declared but not implemented; such tools are
    /// never offered to the model and never executed.
    fn implemented(&self) -> bool {
        true
    }
    /// Handbook 7.5 step 1: resolves the arguments to the normalised target the policy
    /// engine will check. An error means the target is unusable (it is returned to the
    /// model; nothing runs).
    fn resource(&self, args: &Value, ctx: &ToolContext) -> Result<Resource>;
    /// Short description of what will happen, shown when the user is asked to approve.
    /// `resource` is the real location when it could be determined.
    fn preview(&self, args: &Value, resource: &Resource) -> String;
    /// The full payload the user is approving (for example the whole document). `None`
    /// means: show the complete arguments as pretty-printed JSON. The orchestrator caps the
    /// size and marks a cut visibly.
    fn preview_detail(&self, _args: &Value, _resource: &Resource) -> Option<String> {
        None
    }
    /// Arguments as stored in `task_steps.args_redacted`. The default shortens long strings.
    fn redact_args(&self, args: &Value) -> Value {
        shorten_strings(args, 200)
    }
    /// Runs the tool. `args` already passed schema validation and the policy check.
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput>;
}

/// Replaces strings longer than `max` characters by a prefix and a length note.
pub fn shorten_strings(v: &Value, max: usize) -> Value {
    match v {
        Value::String(s) if s.chars().count() > max => {
            let head: String = s.chars().take(max).collect();
            Value::String(format!("{head}...(+{} chars)", s.chars().count() - max))
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| shorten_strings(x, max)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), shorten_strings(x, max)))
                .collect(),
        ),
        other => other.clone(),
    }
}

struct Entry {
    tool: Arc<dyn Tool>,
    validator: ArgsValidator,
}

/// The set of tools the application can run.
#[derive(Default)]
pub struct Registry {
    entries: BTreeMap<String, Entry>,
    planned: BTreeSet<String>,
}

/// Checks that a tool's declared [`ActionClass`] is consistent with its action and side
/// effect: the class may not be weaker than the class of a built-in action it uses
/// (`write` is at least a local write, `send` is a send, ...); a read has no side effect,
/// local classes write locally, external classes write externally; delete may be either.
pub fn check_class(tool: &dyn Tool) -> std::result::Result<(), String> {
    let class = tool.action_class();
    let action = tool.required_action();
    if !matches!(action, Action::Other(_)) && class.rank() < action.class().rank() {
        return Err(format!(
            "declares class {} but its action {} is at least {}",
            class.as_str(),
            action.as_str(),
            action.class().as_str()
        ));
    }
    let side = tool.side_effect();
    let ok = match class {
        ActionClass::Read => side == SideEffect::None,
        ActionClass::LocalRecord | ActionClass::LocalWrite => side == SideEffect::LocalWrite,
        ActionClass::ExternalWrite
        | ActionClass::Send
        | ActionClass::Publish
        | ActionClass::Payment => side == SideEffect::ExternalWrite,
        ActionClass::Delete => side != SideEffect::None,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "declares class {} which does not fit side effect {}",
            class.as_str(),
            side.as_str()
        ))
    }
}

fn valid_tool_name(n: &str) -> bool {
    let mut chars = n.chars();
    n.len() <= 64
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry with the three built-in tools: `create_todos`, `read_file`, `write_markdown`.
    pub fn with_builtin_tools() -> Result<Self> {
        let mut r = Self::new();
        r.register(Arc::new(CreateTodosTool))?;
        r.register(Arc::new(ReadFileTool::default()))?;
        r.register(Arc::new(WriteMarkdownTool))?;
        Ok(r)
    }

    /// Adds a tool, compiling its argument schema. Duplicate names, bad names, invalid
    /// schemas and a declared class that contradicts the tool's action or side effect
    /// (see [`check_class`]) are errors. Unimplemented tools are refused: declare them with
    /// [`Registry::declare_planned`] instead.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        let name = tool.name().to_owned();
        if !valid_tool_name(&name) {
            return Err(LumaError::invalid_input(format!(
                "invalid tool name {name:?}"
            )));
        }
        if self.entries.contains_key(&name) || self.planned.contains(&name) {
            return Err(LumaError::invalid_input(format!(
                "tool {name} is already registered"
            )));
        }
        if !tool.implemented() {
            return Err(LumaError::invalid_input(format!(
                "tool {name} is not implemented; declare it as planned instead"
            )));
        }
        check_class(tool.as_ref())
            .map_err(|why| LumaError::invalid_input(format!("tool {name}: {why}")))?;
        let validator = ArgsValidator::compile(&tool.parameters_schema())
            .map_err(|e| LumaError::invalid_input(format!("tool {name}: {e}")))?;
        self.entries.insert(name, Entry { tool, validator });
        Ok(())
    }

    /// Records that a tool name exists on the roadmap but is not implemented. It is never
    /// offered to the model; a call to it gets a clear "not available" error.
    pub fn declare_planned(&mut self, name: &str) {
        if !self.entries.contains_key(name) {
            self.planned.insert(name.to_owned());
        }
    }

    /// Whether `name` is declared but not implemented.
    pub fn is_planned(&self, name: &str) -> bool {
        self.planned.contains(name)
    }

    /// Looks up an implemented tool.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.entries.get(name).map(|e| &e.tool)
    }

    /// Names of all implemented tools.
    pub fn names(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    /// Validates `args` against the named tool's schema (type, required, enum,
    /// additionalProperties, bounds, formats). The error lists the problems.
    pub fn validate_arguments(&self, name: &str, args: &Value) -> Result<()> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| LumaError::invalid_input(format!("unknown tool {name}")))?;
        entry.validator.check(args).map_err(|problems| {
            LumaError::invalid_input(format!(
                "invalid arguments for {name}: {}",
                problems.join("; ")
            ))
        })
    }

    /// The tools to show the model for a task: implemented, allowed by the task's scope,
    /// and either covered by at least one active grant *that applies to this task* (so the
    /// model is never offered something that policy would refuse outright) or of a class
    /// that is confirmed on every use anyway (send, publish, payment, delete: no grant can
    /// ever cover them, the user is asked each time). This only filters what is *offered*;
    /// every call is still checked by the policy engine.
    pub fn specs_for_task(
        &self,
        store: &SharedStore,
        task_id: &TaskId,
        scope: &TaskScope,
        now: Timestamp,
    ) -> Result<Vec<ToolSpec>> {
        let mut out = Vec::new();
        for (name, e) in &self.entries {
            if !e.tool.implemented() || !scope.permits_tool(name) {
                continue;
            }
            let action = e.tool.required_action();
            if !e.tool.action_class().always_one_shot()
                && !store.with(|s| s.has_active_grant_for_task(action.as_str(), task_id, now))?
            {
                continue;
            }
            out.push(ToolSpec {
                name: name.clone(),
                description: e.tool.description().to_owned(),
                parameters_schema: e.tool.parameters_schema(),
            });
        }
        Ok(out)
    }
}
