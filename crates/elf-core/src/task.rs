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
            Verifying => &[Completed, Running, Failed],
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
        (Verifying, Failed),
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
        assert_eq!((legal, illegal), (18, 63));
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
