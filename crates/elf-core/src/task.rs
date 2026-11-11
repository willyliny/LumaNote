//! Task state machine (handbook 7.5 transition table) and step enums.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::schema::{str_enum, ElfError, Result};

str_enum!(
    /// Lifecycle state of a task.
    TaskStatus {
        /// Created, not started.
        Queued => "queued",
        /// Model loop or tool execution in progress.
        Running => "running",
        /// Waiting for approval or a user decision.
        AwaitingUser => "awaiting_user",
        /// Limit reached or waiting for connectivity; resumable.
        Paused => "paused",
        /// Checking evidence of the result.
        Verifying => "verifying",
        /// An external write may or may not have happened.
        Uncertain => "uncertain",
        /// Finished with evidence (terminal).
        Completed => "completed",
        /// Failed (terminal).
        Failed => "failed",
        /// Cancelled (terminal).
        Cancelled => "cancelled",
    }
);

str_enum!(
    /// Side-effect class of a task step (handbook section 9).
    SideEffect {
        /// Read-only.
        None => "none",
        /// Writes inside the local sandbox of granted paths.
        LocalWrite => "local_write",
        /// Writes to an external system; never re-run automatically.
        ExternalWrite => "external_write",
    }
);

str_enum!(
    /// Status of a task step under the write-ahead rule.
    StepStatus {
        /// Recorded before execution.
        Started => "started",
        /// Finished successfully.
        Succeeded => "succeeded",
        /// Finished with a failure.
        Failed => "failed",
        /// External write whose result is unknown.
        Uncertain => "uncertain",
        /// Non-external step cut off by a restart; safe to re-run.
        Interrupted => "interrupted",
    }
);

/// Per-task execution limits (handbook 7.5: 20 tool calls and 10 minutes by default),
/// read from `tasks.limits_json`. Exceeding one pauses the task; it never fails it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLimits {
    /// Tool calls allowed per budget window.
    pub max_tool_calls: u32,
    /// Seconds allowed per budget window (measured on the injected clock).
    pub max_duration_secs: u64,
}

impl Default for TaskLimits {
    fn default() -> Self {
        Self {
            max_tool_calls: 20,
            max_duration_secs: 600,
        }
    }
}

impl TaskLimits {
    /// Parses `limits_json`; missing keys keep their default, wrongly typed or zero values
    /// are rejected so a typo cannot silently disable a limit.
    pub fn from_json(v: &serde_json::Value) -> Result<Self> {
        let mut out = Self::default();
        let Some(obj) = v.as_object() else {
            return if v.is_null() {
                Ok(out)
            } else {
                Err(ElfError::invalid_input("limits must be a JSON object"))
            };
        };
        if let Some(n) = obj.get("max_tool_calls") {
            out.max_tool_calls = n
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    ElfError::invalid_input("max_tool_calls must be a positive integer")
                })?;
        }
        if let Some(n) = obj.get("max_duration_secs") {
            out.max_duration_secs = n.as_u64().filter(|n| *n > 0).ok_or_else(|| {
                ElfError::invalid_input("max_duration_secs must be a positive integer")
            })?;
        }
        Ok(out)
    }
}

use TaskStatus::*;

impl TaskStatus {
    /// States reachable from `self` according to the handbook table.
    pub fn allowed_targets(self) -> &'static [TaskStatus] {
        match self {
            Queued => &[Running, Cancelled],
            Running => &[
                AwaitingUser,
                Paused,
                Verifying,
                Uncertain,
                Failed,
                Cancelled,
            ],
            AwaitingUser => &[Running, Cancelled],
            Paused => &[Running, Cancelled],
            Verifying => &[Completed, Running, Paused, Failed, Cancelled],
            Uncertain => &[Completed, Failed, Cancelled],
            Completed | Failed | Cancelled => &[],
        }
    }

    /// True for completed, failed and cancelled.
    pub fn is_terminal(self) -> bool {
        self.allowed_targets().is_empty()
    }

    /// Whether the table allows `self -> to`.
    pub fn can_transition_to(self, to: TaskStatus) -> bool {
        self.allowed_targets().contains(&to)
    }
}

/// Checks a transition against the table (evidence is checked by the storage layer).
pub fn check_transition(from: TaskStatus, to: TaskStatus) -> Result<()> {
    if from.can_transition_to(to) {
        Ok(())
    } else {
        Err(ElfError::InvalidTransition {
            from: from.to_string(),
            to: to.to_string(),
            reason: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The handbook table, written out independently of `allowed_targets`.
    const LEGAL: &[(TaskStatus, TaskStatus)] = &[
        (Queued, Running),
        (Queued, Cancelled),
        (Running, AwaitingUser),
        (Running, Paused),
        (Running, Verifying),
        (Running, Uncertain),
        (Running, Failed),
        (Running, Cancelled),
        (AwaitingUser, Running),
        (AwaitingUser, Cancelled),
        (Paused, Running),
        (Paused, Cancelled),
        (Verifying, Completed),
        (Verifying, Running),
        (Verifying, Paused),
        (Verifying, Failed),
        (Verifying, Cancelled),
        (Uncertain, Completed),
        (Uncertain, Failed),
        (Uncertain, Cancelled),
    ];

    #[test]
    fn every_pair_matches_the_table() {
        let mut legal = 0;
        let mut illegal = 0;
        for &from in TaskStatus::ALL {
            for &to in TaskStatus::ALL {
                let expected = LEGAL.contains(&(from, to));
                assert_eq!(from.can_transition_to(to), expected, "{from} -> {to}");
                match check_transition(from, to) {
                    Ok(()) => legal += 1,
                    Err(ElfError::InvalidTransition { from: f, to: t, .. }) => {
                        assert!(!expected);
                        assert_eq!((f.as_str(), t.as_str()), (from.as_str(), to.as_str()));
                        illegal += 1;
                    }
                    Err(e) => panic!("unexpected error {e}"),
                }
            }
        }
        assert_eq!((legal, illegal), (20, 61));
    }

    #[test]
    fn limits_default_and_validation() {
        use serde_json::json;
        assert_eq!(
            TaskLimits::from_json(&json!({})).unwrap(),
            TaskLimits::default()
        );
        assert_eq!(TaskLimits::default().max_tool_calls, 20);
        assert_eq!(TaskLimits::default().max_duration_secs, 600);
        let l = TaskLimits::from_json(&json!({"max_tool_calls": 3})).unwrap();
        assert_eq!((l.max_tool_calls, l.max_duration_secs), (3, 600));
        for bad in [
            json!({"max_tool_calls": 0}),
            json!({"max_tool_calls": "3"}),
            json!({"max_duration_secs": -1}),
            json!([1]),
        ] {
            assert!(TaskLimits::from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn terminal_states_and_parsing() {
        for &s in TaskStatus::ALL {
            assert_eq!(s.is_terminal(), matches!(s, Completed | Failed | Cancelled));
            assert_eq!(TaskStatus::parse(s.as_str()).unwrap(), s);
            assert_eq!(
                serde_json::to_string(&s).unwrap(),
                format!("\"{}\"", s.as_str())
            );
        }
        assert!(TaskStatus::parse("done").is_err());
    }
}
