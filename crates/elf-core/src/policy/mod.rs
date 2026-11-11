//! Grant evaluation (handbook section 8): allow / needs approval / deny.
//!
//! Rules, in order:
//! 1. Path resources must be absolute, contain no `..`, and resolve (following
//!    symlinks) without error; otherwise the result is `Deny`.
//! 2. A grant matches when it is active (not revoked, not expired, uses left),
//!    its scope contains the resource, and its `arguments_hash` is unset or equal.
//!    A path that looks inside a granted directory but resolves outside it
//!    (symlink escape) is `Deny`.
//! 3. If the task is flagged as having read untrusted content, outbound sends,
//!    deletes and writes outside the task's `output_dirs` accept only a one-shot
//!    approval (`max_uses = 1`, arguments hash set, `source_task` = this task);
//!    standing grants never apply.
//! 4. On allow, one use of the chosen grant is consumed atomically.

pub mod paths;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use rusqlite::TransactionBehavior;
use serde::{Deserialize, Serialize};

use crate::ports::Clock;
use crate::schema::{ElfError, GrantId, Result, TaskId};
use crate::storage::grants::{consume, grants_for_action};
use crate::storage::{tasks::load_task, Grant, Store, Task};
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
        .any(|d| real.starts_with(d))
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
            let scope = grant.resource_scope.as_str();
            let hit = match scope.strip_suffix('*') {
                Some(prefix) => s.starts_with(prefix),
                None => s == scope,
            };
            if hit {
                PathMatch::Inside
            } else {
                PathMatch::Outside
            }
        }
        (Resource::Path(_), None) => PathMatch::Outside,
    }
}

fn is_one_shot_for(grant: &Grant, task: &TaskId) -> bool {
    grant.max_uses == Some(1)
        && grant.arguments_hash.is_some()
        && grant.source_task.as_ref() == Some(task)
}

/// Evaluates `req` against stored grants and, on allow, consumes one use.
/// Everything happens in one transaction. Errors are only for bad input
/// (unknown task) or storage failures; refusals are [`Decision`] values.
pub fn evaluate(store: &mut Store, req: &ActionRequest, clock: &dyn Clock) -> Result<Decision> {
    let now = clock.now_utc();
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let task = load_task(&tx, &req.task_id)?;

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
    for g in grants_for_action(&tx, req.action.as_str())? {
        if !g.is_active(now) {
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
        if consume(&tx, &g.id, now)? {
            tx.commit()?;
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
