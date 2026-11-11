//! Grant evaluation (handbook section 8): allow / needs approval / deny.
//!
//! Rules, in order:
//! 1. Path resources must be absolute, contain no `..`, and resolve (following
//!    symlinks) without error; otherwise the result is `Deny`. A task that already
//!    reached a terminal state may not act at all (`Deny`).
//! 2. A grant matches when it is active (not revoked, not expired, uses left), it
//!    *applies to the requesting task*, its scope contains the resource, and its
//!    `arguments_hash` is unset or equal. A grant applies to a task when it is
//!    [`GrantScope::UserStanding`], or [`GrantScope::Task`] with `source_task` equal to
//!    the requesting task: grants created for one task never authorise another. A path
//!    that looks inside a granted directory but resolves outside it (symlink escape) is
//!    `Deny`.
//! 3. If the task is flagged as having read untrusted content, outbound sends,
//!    deletes and writes outside the task's `output_dirs` accept only a one-shot
//!    approval (`max_uses = 1`, arguments hash set, task-scoped, `source_task` = this
//!    task); standing grants never apply.
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

use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::ports::Clock;
use crate::schema::{ElfError, GrantId, Result, StepId, TaskId, Timestamp};
use crate::storage::grants::{consume, grants_for_action};
use crate::storage::tasks::{begin_step_in, reopen_step_in, set_step_consumed_grant};
use crate::storage::{tasks::load_task, Grant, GrantScope, Store, Task, TaskStep};
use crate::task::SideEffect;
use paths::{classify, resolve_target, PathMatch};

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

/// Action classes that matter for the untrusted-content rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionClass {
    /// Read-only.
    Read,
    /// Local or external write.
    Write,
    /// Deletion.
    Delete,
    /// Outbound send.
    Send,
    /// Not covered by the untrusted rule.
    Other,
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
    /// Classification used by the untrusted-content rule.
    pub fn class(&self) -> ActionClass {
        match self {
            Action::Read => ActionClass::Read,
            Action::Write => ActionClass::Write,
            Action::Delete => ActionClass::Delete,
            Action::Send => ActionClass::Send,
            Action::Other(_) => ActionClass::Other,
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
    /// Attempted action.
    pub action: Action,
    /// Target.
    pub resource: Resource,
    /// Hash of the normalised arguments (see [`crate::schema::hash_arguments`]).
    pub arguments_hash: String,
}

/// Why approval is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalReason {
    /// No active grant covers this exact action.
    NoMatchingGrant,
    /// The task read untrusted content; each such action needs its own confirmation.
    UntrustedContent,
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
            Decision::NeedsApproval { .. } => Err(ElfError::ApprovalRequired {
                action: req.action.as_str().to_owned(),
                resource: req.resource.describe(),
            }),
            Decision::Deny { .. } => Err(ElfError::NoPermission {
                action: req.action.as_str().to_owned(),
                resource: req.resource.describe(),
            }),
        }
    }
}

fn write_in_task_scope(task: &Task, real: Option<&PathBuf>) -> bool {
    let Some(real) = real else { return false };
    task.scope
        .get("output_dirs")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .filter_map(|d| std::fs::canonicalize(d).ok())
        .any(|d| paths::path_within(real, &d))
}

fn scope_match(grant: &Grant, resource: &Resource, real: Option<&PathBuf>) -> PathMatch {
    match (resource, real) {
        (Resource::Path(raw), Some(real)) => {
            let scope = Path::new(&grant.resource_scope);
            if scope.is_absolute() {
                classify(scope, raw, real)
            } else {
                PathMatch::Outside
            }
        }
        (Resource::Other(s), _) => {
            if other_scope_matches(&grant.resource_scope, s) {
                PathMatch::Inside
            } else {
                PathMatch::Outside
            }
        }
        (Resource::Path(_), None) => PathMatch::Outside,
    }
}

fn is_one_shot_for(grant: &Grant, task: &TaskId) -> bool {
    grant.scope == GrantScope::Task
        && grant.max_uses == Some(1)
        && grant.arguments_hash.is_some()
        && grant.source_task.as_ref() == Some(task)
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
    let now = clock.now_utc();
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let decision = evaluate_in(&tx, req, now)?;
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
    let now = clock.now_utc();
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let decision = evaluate_in(&tx, req, now)?;
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

fn evaluate_in(tx: &Connection, req: &ActionRequest, now: Timestamp) -> Result<Decision> {
    let task = load_task(tx, &req.task_id)?;
    if task.status.is_terminal() {
        return Ok(Decision::Deny {
            reason: format!("task {} is already {}", task.id, task.status),
        });
    }

    let real: Option<PathBuf> = match &req.resource {
        Resource::Path(p) => match resolve_target(p) {
            Ok(r) => Some(r),
            Err(reason) => return Ok(Decision::Deny { reason }),
        },
        Resource::Other(_) => None,
    };

    let sensitive = task.untrusted_content
        && match req.action.class() {
            ActionClass::Send | ActionClass::Delete => true,
            ActionClass::Write => !write_in_task_scope(&task, real.as_ref()),
            ActionClass::Read | ActionClass::Other => false,
        };

    let mut matched: Vec<Grant> = Vec::new();
    for g in grants_for_action(tx, req.action.as_str())? {
        if !g.is_active(now) || !g.applies_to(&req.task_id) {
            continue;
        }
        match scope_match(&g, &req.resource, real.as_ref()) {
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
                let hash_ok = g
                    .arguments_hash
                    .as_deref()
                    .is_none_or(|h| h == req.arguments_hash);
                let sensitive_ok = !sensitive || is_one_shot_for(&g, &req.task_id);
                if hash_ok && sensitive_ok {
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
    Ok(Decision::NeedsApproval {
        reason: if sensitive {
            ApprovalReason::UntrustedContent
        } else {
            ApprovalReason::NoMatchingGrant
        },
    })
}
