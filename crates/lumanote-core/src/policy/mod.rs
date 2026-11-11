//! Grant evaluation (handbook section 8 and 8.1): allow / needs approval / deny.
//!
//! Rules, in order:
//! 1. A task that already reached a terminal state may not act at all (`Deny`). A path
//!    resource is validated **lexically, without touching the file system**
//!    ([`paths::lexical_check`]): UNC and device paths, drive-relative paths, alternate data
//!    streams, NUL, reserved device names and `..` are `Deny`.
//! 2. A grant matches when it is active (not revoked, not expired, uses left), it
//!    *applies to the requesting task*, it is bound to this tool (if it names one), its
//!    scope contains the resource and its `arguments_hash` is unset or equal. A grant
//!    applies to a task when it is [`GrantScope::UserStanding`], or [`GrantScope::Task`]
//!    with `source_task` equal to the requesting task. Path scopes are compared on the
//!    lexical path; **only a path that lies under a granted root is then resolved**
//!    (canonicalize, symlink/junction escape check; an escape is `Deny`). A path that no
//!    grant covers is `NeedsApproval` and the file system is never consulted for it.
//! 3. Every action has an [`ActionClass`]. Outbound send, publish, payment and delete never
//!    accept a standing or task-implied grant: each use needs a one-shot approval
//!    (`max_uses = 1`, task-scoped, bound to the tool, the target and the arguments hash).
//!    A tool whose action has no explicit class counts as an external write
//!    ([`Action::class`]). If the task or its conversation holds content the user did not
//!    type (the untrusted flag), external writes and writes outside the task's
//!    `output_dirs` need such a one-shot approval too.
//! 4. On allow, one use of the chosen grant is consumed atomically ([`evaluate`]).
//!    The orchestrator uses [`authorize_and_begin_step`] instead, which consumes the use
//!    and writes the write-ahead step record in the same transaction.
//!
//! # Non-path resource scopes
//!
//! See [`other_scope_matches`] for the exact rule.

pub mod paths;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::ports::Clock;
use crate::schema::{LumaError, GrantId, Result, StepId, TaskId, Timestamp};
use crate::storage::conversation_tainted_in;
use crate::storage::grants::{consume, grants_for_action};
use crate::storage::tasks::{begin_step_in, reopen_step_in, set_step_consumed_grant};
use crate::storage::{tasks::load_task, Grant, GrantScope, Store, Task, TaskStep};
use crate::task::SideEffect;
use paths::{classify_with, lexical_check, resolve_target_with, FsPort, OsFs, PathMatch};

/// What is being attempted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Read data.
    Read,
    /// Write or modify data.
    Write,
    /// Delete data.
    Delete,
    /// Send data to an outside party (mail, publish, upload, ...).
    Send,
    /// Any other tool-specific action name.
    Other(String),
}

/// What kind of effect an action has, which decides how strictly policy treats it. Every
/// tool declares its class explicitly ([`crate::tools::Tool::action_class`]); an action
/// that was never classified counts as [`ActionClass::ExternalWrite`] (fail closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionClass {
    /// Reads data; changes nothing.
    Read,
    /// Writes only the application's own local records (for example todos). Never leaves
    /// the machine and is not a file write, so the untrusted flag does not restrict it.
    LocalRecord,
    /// Writes local files. With the untrusted flag set, a write outside the task's
    /// output directories needs a one-shot approval.
    LocalWrite,
    /// Writes to an external system without being one of the four classes below. With the
    /// untrusted flag set it needs a one-shot approval. This is also the class of any
    /// action nobody classified.
    ExternalWrite,
    /// Sends data to an outside party (mail, message, invitation, upload).
    Send,
    /// Publishes data.
    Publish,
    /// Moves money or commits to a payment.
    Payment,
    /// Deletes data.
    Delete,
}

impl ActionClass {
    /// Send, publish, payment and delete accept no standing or task-implied grant in this
    /// version, whatever the untrusted flag says: every use needs a fresh one-shot approval.
    pub fn always_one_shot(self) -> bool {
        matches!(
            self,
            ActionClass::Send | ActionClass::Publish | ActionClass::Payment | ActionClass::Delete
        )
    }

    /// Strictness order used to check that a tool does not declare a class weaker than the
    /// built-in action it uses.
    pub fn rank(self) -> u8 {
        match self {
            ActionClass::Read => 0,
            ActionClass::LocalRecord => 1,
            ActionClass::LocalWrite => 2,
            ActionClass::ExternalWrite => 3,
            ActionClass::Send
            | ActionClass::Publish
            | ActionClass::Payment
            | ActionClass::Delete => 4,
        }
    }

    /// Stable spelling for logs and tests.
    pub fn as_str(self) -> &'static str {
        match self {
            ActionClass::Read => "read",
            ActionClass::LocalRecord => "local_record",
            ActionClass::LocalWrite => "local_write",
            ActionClass::ExternalWrite => "external_write",
            ActionClass::Send => "send",
            ActionClass::Publish => "publish",
            ActionClass::Payment => "payment",
            ActionClass::Delete => "delete",
        }
    }
}

impl Action {
    /// Stable name stored in `grants.action`.
    pub fn as_str(&self) -> &str {
        match self {
            Action::Read => "read",
            Action::Write => "write",
            Action::Delete => "delete",
            Action::Send => "send",
            Action::Other(s) => s,
        }
    }
    /// Parses a stored name.
    pub fn parse(s: &str) -> Self {
        match s {
            "read" => Action::Read,
            "write" => Action::Write,
            "delete" => Action::Delete,
            "send" => Action::Send,
            other => Action::Other(other.to_owned()),
        }
    }
    /// The class of this action when nothing more specific was declared. The four built-in
    /// actions map to their class; **any other action is an external write** (fail
    /// closed): a tool that is not an external write must say so through
    /// [`crate::tools::Tool::action_class`].
    pub fn class(&self) -> ActionClass {
        match self {
            Action::Read => ActionClass::Read,
            Action::Write => ActionClass::LocalWrite,
            Action::Delete => ActionClass::Delete,
            Action::Send => ActionClass::Send,
            Action::Other(_) => ActionClass::ExternalWrite,
        }
    }
}

/// Target of an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    /// A file or directory.
    Path(PathBuf),
    /// Anything else (recipient, URL, calendar ID); matched by exact string or `prefix*`.
    Other(String),
}

impl Resource {
    fn describe(&self) -> String {
        match self {
            Resource::Path(p) => p.display().to_string(),
            Resource::Other(s) => s.clone(),
        }
    }
}

/// A request to be checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRequest {
    /// Requesting task.
    pub task_id: TaskId,
    /// Name of the tool making the call; one-shot approvals are bound to it.
    pub tool: String,
    /// Attempted action.
    pub action: Action,
    /// Declared class of the action (see [`ActionClass`]).
    pub class: ActionClass,
    /// Target.
    pub resource: Resource,
    /// Hash of the normalised arguments (see [`crate::schema::hash_arguments`]).
    pub arguments_hash: String,
}

impl ActionRequest {
    /// A request for `action` with the action's default class and the action name as tool
    /// name. The orchestrator sets `tool` and `class` from the tool's declaration instead.
    pub fn new(
        task_id: TaskId,
        action: Action,
        resource: Resource,
        arguments_hash: impl Into<String>,
    ) -> Self {
        Self {
            task_id,
            tool: action.as_str().to_owned(),
            class: action.class(),
            action,
            resource,
            arguments_hash: arguments_hash.into(),
        }
    }
}

/// Why approval is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalReason {
    /// No active grant covers this exact action.
    NoMatchingGrant,
    /// The task (or its conversation) holds content the user did not type; each such
    /// action needs its own confirmation.
    UntrustedContent,
    /// The action never accepts a standing grant (send, publish, payment, delete): every
    /// use needs its own confirmation.
    ConfirmEveryUse,
}

/// Outcome of a policy check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Allowed; one use of the grant was consumed.
    Allow {
        /// The grant that covered the action.
        grant_id: GrantId,
    },
    /// The user must decide first.
    NeedsApproval {
        /// Why.
        reason: ApprovalReason,
    },
    /// Refused outright (invalid path, traversal, symlink escape).
    Deny {
        /// Explanation.
        reason: String,
    },
}

impl Decision {
    /// Maps to the typed error: `Allow` is `Ok(grant)`, the rest become
    /// `ApprovalRequired` / `NoPermission`.
    pub fn into_result(self, req: &ActionRequest) -> Result<GrantId> {
        match self {
            Decision::Allow { grant_id } => Ok(grant_id),
            Decision::NeedsApproval { .. } => Err(LumaError::ApprovalRequired {
                action: req.action.as_str().to_owned(),
                resource: req.resource.describe(),
            }),
            Decision::Deny { .. } => Err(LumaError::NoPermission {
                action: req.action.as_str().to_owned(),
                resource: req.resource.describe(),
            }),
        }
    }
}

/// The task's declared output directories, lexically normalised.
fn output_dirs(task: &Task, windows: bool) -> Vec<PathBuf> {
    task.scope
        .get("output_dirs")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .filter_map(|d| lexical_check(Path::new(d), windows).ok())
        .collect()
}

/// Whether the target still resolves to what the user was shown when they approved. The
/// approval request recorded the real location (`resolved_resource`) when it could be
/// determined; the target must resolve to exactly that. When nothing was recorded, the
/// target must not resolve anywhere but below its own spelling.
fn approved_location_holds(
    tx: &Connection,
    grant: &Grant,
    spelled: &Path,
    real: &Path,
    windows: bool,
) -> Result<bool> {
    let shown: Option<String> = tx
        .query_row(
            "SELECT resolved_resource FROM approvals WHERE grant_id = ?1",
            [grant.id.as_str()],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(match shown {
        Some(s) => {
            let s = Path::new(&s);
            paths::path_within_with(real, s, windows) && paths::path_within_with(s, real, windows)
        }
        None => paths::path_within_with(real, spelled, windows),
    })
}

/// A one-shot approval for exactly this request: task-scoped, single use, bound to the
/// tool and to the arguments hash, created for this task.
fn is_one_shot_for(grant: &Grant, req: &ActionRequest) -> bool {
    grant.scope == GrantScope::Task
        && grant.max_uses == Some(1)
        && grant.arguments_hash.as_deref() == Some(req.arguments_hash.as_str())
        && grant.tool.as_deref() == Some(req.tool.as_str())
        && grant.source_task.as_ref() == Some(&req.task_id)
}

/// Whether a non-path `resource` is covered by a grant's `scope` string.
///
/// Rule (byte-exact, case-sensitive, no normalisation of either side):
/// * A scope that does not end in `*` matches only an identical resource. A `*` anywhere
///   but the last position is an ordinary character.
/// * A scope ending in `*` is a *prefix* pattern: the text before the `*` (the prefix)
///   must be non-empty, and the resource must start with it. A bare `*` and a prefix
///   containing another `*` match nothing, so a wildcard can never mean "everything".
/// * URL-like resources (containing `://`) are stricter, so that a prefix cannot spill
///   over a host or scheme boundary: the prefix must itself contain `://` followed by a
///   `/`, i.e. it must pin the scheme and the complete host (`https://docs.rs/*` is fine,
///   `https://docs.rs*` would also cover `https://docs.rs.evil.example/` and `https*`
///   would cover `https://anything`, so both match nothing).
/// * A wildcard never matches a resource that has a `..` path segment (segments split on
///   `/` and `\`), contains a backslash, or contains a percent-encoded dot, slash or
///   backslash (`%2e`, `%2f`, `%5c`, any case): those can climb out of the prefix once a
///   server or file system normalises them. Exact scopes are not affected.
///
/// * A prefix that looks like a bare host name (letters, digits, `-` and at least one `.`,
///   no `/`, no `:`) matches nothing: `example.test*` would also cover
///   `example.test.evil.com`. Write `example.test/*` (or the full URL prefix); a host is
///   always compared whole. There are no wildcard sub-domain rules: list each host.
///
/// Non-wildcard prefixes are otherwise plain string prefixes: `mail:*` covers
/// `mail:a@x.com`, and `cal-*` covers `cal-1` and `cal-10`. Tools must therefore give
/// each resource as one canonical target (never a list of recipients).
pub fn other_scope_matches(scope: &str, resource: &str) -> bool {
    let Some(prefix) = scope.strip_suffix('*') else {
        return scope == resource;
    };
    if prefix.is_empty() || prefix.contains('*') || !resource.starts_with(prefix) {
        return false;
    }
    let host_like = prefix.contains('.')
        && prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if host_like {
        return false;
    }
    if resource.contains("://") {
        match prefix.split_once("://") {
            Some((_, after_scheme)) if after_scheme.contains('/') => {}
            _ => return false,
        }
    }
    if resource.contains('\\') || resource.split('/').any(|seg| seg == "..") {
        return false;
    }
    let lower = resource.to_ascii_lowercase();
    !["%2e", "%2f", "%5c"].iter().any(|enc| lower.contains(enc))
}

/// Evaluates `req` against stored grants and, on allow, consumes one use.
/// Everything happens in one transaction. Errors are only for bad input
/// (unknown task) or storage failures; refusals are [`Decision`] values.
///
/// This consumes a use *without* recording a step. Code that is about to execute a tool
/// must call [`authorize_and_begin_step`] instead, so that a crash cannot spend a
/// one-shot approval without a durable step record.
pub fn evaluate(store: &mut Store, req: &ActionRequest, clock: &dyn Clock) -> Result<Decision> {
    evaluate_with(store, req, clock, &OsFs)
}

/// [`evaluate`] with an explicit file-system port (tests inject a spy).
pub fn evaluate_with(
    store: &mut Store,
    req: &ActionRequest,
    clock: &dyn Clock,
    fs: &dyn FsPort,
) -> Result<Decision> {
    let now = clock.now_utc();
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let decision = evaluate_in(&tx, req, now, fs)?;
    if matches!(decision, Decision::Allow { .. }) {
        tx.commit()?;
    }
    Ok(decision)
}

/// How [`authorize_and_begin_step`] records the step.
#[derive(Debug, Clone)]
pub struct StepStart<'a> {
    /// Tool name.
    pub tool: &'a str,
    /// Arguments with secrets removed.
    pub args_redacted: Option<&'a str>,
    /// Side-effect class of the tool.
    pub side_effect: SideEffect,
    /// Idempotency key of the call.
    pub idempotency_key: Option<&'a str>,
    /// An existing failed or interrupted non-external step to re-open instead of
    /// inserting a new one.
    pub reopen: Option<&'a StepId>,
}

/// Policy check and write-ahead step record in ONE transaction.
///
/// On `Allow`, the grant use is consumed, the step is inserted (or re-opened) as
/// `started`, and the grant is recorded on the step (`consumed_grant_id`), all committed
/// together; the step is returned. On any other decision nothing is written and no step is
/// returned. If recording the step fails (task not running, duplicate idempotency key,
/// storage error) the whole transaction rolls back and no use is consumed.
///
/// Crash analysis: before the commit nothing happened (the user is simply asked again, as
/// before); after it the step exists as `started` and restart recovery applies the normal
/// rules (external writes become `uncertain` and are never re-run; other steps become
/// `interrupted` and the use they consumed is given back, because a re-run is authorised
/// again).
pub fn authorize_and_begin_step(
    store: &mut Store,
    req: &ActionRequest,
    clock: &dyn Clock,
    start: &StepStart<'_>,
) -> Result<(Decision, Option<TaskStep>)> {
    authorize_and_begin_step_with(store, req, clock, start, &OsFs)
}

/// [`authorize_and_begin_step`] with an explicit file-system port.
pub fn authorize_and_begin_step_with(
    store: &mut Store,
    req: &ActionRequest,
    clock: &dyn Clock,
    start: &StepStart<'_>,
    fs: &dyn FsPort,
) -> Result<(Decision, Option<TaskStep>)> {
    let now = clock.now_utc();
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let decision = evaluate_in(&tx, req, now, fs)?;
    let Decision::Allow { grant_id } = &decision else {
        return Ok((decision, None));
    };
    let step_id = match start.reopen {
        Some(id) => {
            reopen_step_in(&tx, id, now)?;
            id.clone()
        }
        None => begin_step_in(
            &tx,
            &req.task_id,
            start.tool,
            start.args_redacted,
            start.side_effect,
            start.idempotency_key,
            now,
        )?,
    };
    set_step_consumed_grant(&tx, &step_id, grant_id)?;
    tx.commit()?;
    let step = store.get_step(&step_id)?;
    Ok((decision, Some(step)))
}

/// A grant that may cover the request, with the scope in lexical form.
struct Candidate {
    grant: Grant,
    scope: PathBuf,
    /// Bound to the exact tool and arguments (a one-shot approval of this very request).
    argument_bound: bool,
}

fn evaluate_in(
    tx: &Connection,
    req: &ActionRequest,
    now: Timestamp,
    fs: &dyn FsPort,
) -> Result<Decision> {
    let task = load_task(tx, &req.task_id)?;
    if task.status.is_terminal() {
        return Ok(Decision::Deny {
            reason: format!("task {} is already {}", task.id, task.status),
        });
    }
    let windows = fs.windows_rules();

    // 1. purely lexical validation: nothing below touches the file system yet
    let lexical: Option<PathBuf> = match &req.resource {
        Resource::Path(p) => match lexical_check(p, windows) {
            Ok(n) => Some(n),
            Err(reason) => return Ok(Decision::Deny { reason }),
        },
        Resource::Other(_) => None,
    };

    // 2. which restrictions apply
    let tainted = task.untrusted_content
        || match &task.conversation_id {
            Some(c) => conversation_tainted_in(tx, c)?,
            None => false,
        };
    let mut real: Option<PathBuf> = None;
    let mut resolve_error: Option<String> = None;
    let mut resolve = |lex: &Path| -> Option<PathBuf> {
        if real.is_none() && resolve_error.is_none() {
            match resolve_target_with(fs, lex) {
                Ok(r) => real = Some(r),
                Err(reason) => resolve_error = Some(reason),
            }
        }
        real.clone()
    };
    let by_taint = tainted
        && match req.class {
            ActionClass::ExternalWrite
            | ActionClass::Send
            | ActionClass::Publish
            | ActionClass::Payment
            | ActionClass::Delete => true,
            ActionClass::LocalWrite => !match &lexical {
                // in scope only if lexically inside an output directory AND, once resolved
                // (it is under a root the task declared), still inside it
                Some(lex) => output_dirs(&task, windows)
                    .iter()
                    .filter(|d| paths::path_within_with(lex, d, windows))
                    .any(|d| {
                        resolve(lex).is_some_and(|r| {
                            resolve_target_with(fs, d)
                                .is_ok_and(|dr| paths::path_within_with(&r, &dr, windows))
                        })
                    }),
                None => false,
            },
            _ => false,
        };
    let one_shot_only = req.class.always_one_shot() || by_taint;
    let needs_approval = Decision::NeedsApproval {
        reason: if by_taint {
            ApprovalReason::UntrustedContent
        } else if one_shot_only {
            ApprovalReason::ConfirmEveryUse
        } else {
            ApprovalReason::NoMatchingGrant
        },
    };

    // 3. candidate grants, matched without the file system
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut others: Vec<Grant> = Vec::new();
    for g in grants_for_action(tx, req.action.as_str())? {
        if !g.is_active(now) || !g.applies_to(&req.task_id) {
            continue;
        }
        if g.tool.as_deref().is_some_and(|t| t != req.tool) {
            continue;
        }
        if g.arguments_hash
            .as_deref()
            .is_some_and(|h| h != req.arguments_hash)
        {
            continue;
        }
        match (&req.resource, &lexical) {
            (Resource::Other(s), _) => {
                if other_scope_matches(&g.resource_scope, s) {
                    others.push(g);
                }
            }
            (Resource::Path(_), Some(lex)) => {
                let Ok(scope) = lexical_check(Path::new(&g.resource_scope), windows) else {
                    continue;
                };
                let argument_bound = g.arguments_hash.is_some();
                // Coverage is decided on the spelling alone: nothing is looked up for a
                // path no grant covers, not even the grant's own root. (A path spelled
                // with the real location of a linked grant root is simply not covered and
                // asks for approval.)
                if paths::path_within_with(lex, &scope, windows) {
                    candidates.push(Candidate {
                        grant: g,
                        scope,
                        argument_bound,
                    });
                }
            }
            (Resource::Path(_), None) => {}
        }
    }

    // 4. decide
    let mut matched: Vec<Grant> = Vec::new();
    match (&req.resource, &lexical) {
        (Resource::Path(_), Some(lex)) => {
            if candidates.is_empty() {
                // nothing covers this path: it is not looked at
                return Ok(needs_approval);
            }
            let Some(real) = resolve(lex) else {
                return Ok(Decision::Deny {
                    reason: resolve_error.unwrap_or_else(|| "cannot resolve the path".into()),
                });
            };
            let standing_cover = candidates.iter().any(|c| !c.argument_bound)
                || output_dirs(&task, windows)
                    .iter()
                    .any(|d| paths::path_within_with(lex, d, windows));
            for c in candidates {
                match classify_with(fs, &c.scope, lex, &real) {
                    PathMatch::Escape => {
                        return Ok(Decision::Deny {
                            reason: format!(
                                "{} resolves outside the granted directory",
                                req.resource.describe()
                            ),
                        })
                    }
                    PathMatch::Outside => {}
                    PathMatch::Inside => {
                        if !standing_cover
                            && !approved_location_holds(tx, &c.grant, lex, &real, windows)?
                        {
                            // The approval covers what the user was shown: the target's
                            // real location at the time of the request. It resolves
                            // somewhere else now (a link was changed or redirects): not
                            // what was approved.
                            return Ok(Decision::Deny {
                                reason: format!(
                                    "{} resolves to {}, which is not what was approved",
                                    req.resource.describe(),
                                    real.display()
                                ),
                            });
                        }
                        if !one_shot_only || is_one_shot_for(&c.grant, req) {
                            matched.push(c.grant);
                        }
                    }
                }
            }
        }
        _ => {
            for g in others {
                if !one_shot_only || is_one_shot_for(&g, req) {
                    matched.push(g);
                }
            }
        }
    }
    // Prefer argument-bound (one-shot style) grants so approvals are spent first.
    matched.sort_by_key(|g| g.arguments_hash.is_none());

    for g in matched {
        if consume(tx, &g.id, now)? {
            return Ok(Decision::Allow { grant_id: g.id });
        }
    }
    Ok(needs_approval)
}
