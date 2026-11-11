//! Which writes the model attempted in a task did not happen and were not made up for
//! (handbook 8.1, "task completion"). Pure functions over the recorded call outcomes.

use super::events::UnresolvedItem;
use crate::storage::{CallOutcome, CallOutcomeKind};

/// Attempts to write that were denied, expired, rejected, failed or ended uncertain and
/// that no *later successful call of the same tool for the same target* superseded.
///
/// "Same target" is the recorded resource; a call whose target was never known (it was
/// rejected before a target existed, e.g. malformed arguments) is superseded by any later
/// success of the same tool, and an attempt is superseded by a success that has no target
/// either. Read-only calls never count. Order follows the order the calls were answered.
pub(crate) fn unresolved_writes(outcomes: &[CallOutcome]) -> Vec<UnresolvedItem> {
    outcomes
        .iter()
        .enumerate()
        .filter(|(_, o)| {
            o.is_write
                && !matches!(
                    o.outcome,
                    CallOutcomeKind::Succeeded | CallOutcomeKind::NotExecuted
                )
        })
        .filter(|(i, o)| {
            !outcomes[i + 1..].iter().any(|later| {
                later.is_write
                    && later.outcome == CallOutcomeKind::Succeeded
                    && later.tool == o.tool
                    && match (&o.resource, &later.resource) {
                        (Some(a), Some(b)) => a == b,
                        _ => true,
                    }
            })
        })
        .map(|(_, o)| UnresolvedItem {
            tool: o.tool.clone(),
            call_id: o.tool_call_id.clone(),
            resource: o.resource.clone(),
            outcome: o.outcome.as_str().to_owned(),
            detail: o.detail.clone(),
        })
        .collect()
}

/// One line per item for the task's failure reason.
pub(crate) fn describe(items: &[UnresolvedItem]) -> String {
    items
        .iter()
        .map(|i| match &i.resource {
            Some(r) => format!("{} on {} ({})", i.tool, r, i.outcome),
            None => format!("{} ({})", i.tool, i.outcome),
        })
        .collect::<Vec<_>>()
        .join("; ")
}
