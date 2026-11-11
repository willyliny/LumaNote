//! Shared schema types: IDs, timestamps, the typed error enum and small helpers.

use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

/// A UTC instant; serialises as RFC 3339.
pub type Timestamp = DateTime<Utc>;

/// Result alias using [`ElfError`].
pub type Result<T> = std::result::Result<T, ElfError>;

/// Formats a timestamp in the fixed-width form stored in SQLite (sortable as text).
pub fn ts_to_sql(t: &Timestamp) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Parses an RFC 3339 timestamp into UTC.
pub fn ts_from_sql(s: &str) -> Result<Timestamp> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| ElfError::Storage {
            message: format!("bad timestamp {s:?}: {e}"),
        })
}

/// Lower-case hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Stable hash of JSON arguments (object keys are sorted by `serde_json`).
pub fn hash_arguments(args: &serde_json::Value) -> String {
    sha256_hex(args.to_string().as_bytes())
}

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Generates a fresh random identifier.
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }
            /// Wraps an existing identifier string.
            pub fn from_string(s: impl Into<String>) -> Self {
                Self(s.into())
            }
            /// The identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(
    /// Identifier of a task.
    TaskId
);
id_type!(
    /// Identifier of a task step.
    StepId
);
id_type!(
    /// Identifier of a conversation.
    ConversationId
);
id_type!(
    /// Identifier of a stored message.
    MessageId
);
id_type!(
    /// Identifier of an artifact.
    ArtifactId
);
id_type!(
    /// Identifier of a grant.
    GrantId
);
id_type!(
    /// Identifier of a model request.
    RequestId
);
id_type!(
    /// Provider-assigned (or locally assigned) identifier of a tool call.
    ToolCallId
);

/// Defines a string-backed enum with `as_str`, `parse` and `Display`.
macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $var:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $( $(#[$vm])* #[serde(rename = $s)] $var, )+
        }
        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$var),+];
            /// The stable wire / database spelling.
            pub fn as_str(self) -> &'static str {
                match self { $($name::$var => $s),+ }
            }
            /// Parses the stable spelling.
            pub fn parse(s: &str) -> $crate::schema::Result<Self> {
                match s {
                    $($s => Ok($name::$var),)+
                    _ => Err($crate::schema::ElfError::InvalidInput {
                        message: format!("unknown {} value: {s}", stringify!($name)),
                    }),
                }
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}
pub(crate) use str_enum;

str_enum!(
    /// How much the origin of tool-result content can be trusted (handbook section 8).
    Trust {
        /// Provided directly by the user.
        UserProvided => "user_provided",
        /// Read from a location the user granted.
        LocalGranted => "local_granted",
        /// Fetched from outside (web, mail, third-party tool); untrusted.
        External => "external",
    }
);

/// Pointer to the evidence that justifies completing a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceRef {
    /// A stored artifact (file, document) belonging to the task.
    Artifact {
        /// The artifact.
        id: ArtifactId,
    },
    /// A task step that finished successfully (tool result).
    Step {
        /// The step.
        id: StepId,
    },
}

impl EvidenceRef {
    /// Compact `kind:id` form stored in `tasks.result_ref` / `task_steps.evidence_ref`.
    pub fn to_ref_string(&self) -> String {
        match self {
            EvidenceRef::Artifact { id } => format!("artifact:{id}"),
            EvidenceRef::Step { id } => format!("step:{id}"),
        }
    }
}

/// Typed error shared by every module (handbook section 12, last paragraph).
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ElfError {
    /// The action is not allowed at all.
    #[error("no permission: {action} on {resource}")]
    NoPermission {
        /// Attempted action.
        action: String,
        /// Target resource.
        resource: String,
    },
    /// The action needs a user decision first.
    #[error("approval required: {action} on {resource}")]
    ApprovalRequired {
        /// Attempted action.
        action: String,
        /// Target resource.
        resource: String,
    },
    /// An external system (provider, network, tool) failed.
    #[error("external failure: {message}")]
    ExternalFailure {
        /// Human-readable detail.
        message: String,
        /// Whether a retry may help.
        retryable: bool,
    },
    /// An operation exceeded its deadline.
    #[error("timeout: {operation}")]
    Timeout {
        /// The operation that timed out.
        operation: String,
    },
    /// The operation was cancelled.
    #[error("cancelled")]
    Cancelled,
    /// An external write may or may not have happened.
    #[error("outcome uncertain: {message}")]
    OutcomeUncertain {
        /// Human-readable detail.
        message: String,
    },
    /// A state-machine transition that the table forbids.
    #[error("invalid transition: {from} -> {to}{}", reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default())]
    InvalidTransition {
        /// Current state.
        from: String,
        /// Requested state.
        to: String,
        /// Extra explanation, e.g. missing evidence.
        reason: Option<String>,
    },
    /// The caller passed something invalid (including unknown IDs).
    #[error("invalid input: {message}")]
    InvalidInput {
        /// Human-readable detail.
        message: String,
    },
    /// Database or on-disk persistence failure.
    #[error("storage error: {message}")]
    Storage {
        /// Human-readable detail.
        message: String,
    },
}

impl ElfError {
    /// Stable machine-readable code (same as the serde `kind`).
    pub fn code(&self) -> &'static str {
        match self {
            ElfError::NoPermission { .. } => "no_permission",
            ElfError::ApprovalRequired { .. } => "approval_required",
            ElfError::ExternalFailure { .. } => "external_failure",
            ElfError::Timeout { .. } => "timeout",
            ElfError::Cancelled => "cancelled",
            ElfError::OutcomeUncertain { .. } => "outcome_uncertain",
            ElfError::InvalidTransition { .. } => "invalid_transition",
            ElfError::InvalidInput { .. } => "invalid_input",
            ElfError::Storage { .. } => "storage_error",
        }
    }

    /// Shorthand for [`ElfError::InvalidInput`].
    pub fn invalid_input(message: impl Into<String>) -> Self {
        ElfError::InvalidInput {
            message: message.into(),
        }
    }
}

impl From<rusqlite::Error> for ElfError {
    fn from(e: rusqlite::Error) -> Self {
        ElfError::Storage {
            message: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_serialises_with_kind_tag() {
        let e = ElfError::InvalidTransition {
            from: "queued".into(),
            to: "completed".into(),
            reason: None,
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "invalid_transition");
        assert_eq!(serde_json::from_value::<ElfError>(v).unwrap(), e);
        assert_eq!(
            serde_json::to_value(ElfError::Cancelled).unwrap()["kind"],
            "cancelled"
        );
        assert_eq!(e.code(), "invalid_transition");
        assert!(e.to_string().contains("queued -> completed"));
    }

    #[test]
    fn timestamps_round_trip_rfc3339() {
        let t = ts_from_sql("2026-11-12T01:02:03.5+08:00").unwrap();
        assert_eq!(ts_to_sql(&t), "2026-11-11T17:02:03.500000Z");
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.ends_with("Z\""));
        assert!(ts_from_sql("nope").is_err());
    }

    #[test]
    fn ids_are_unique_and_transparent() {
        let a = TaskId::new();
        assert_ne!(a, TaskId::new());
        assert_eq!(
            serde_json::to_string(&TaskId::from_string("x")).unwrap(),
            "\"x\""
        );
    }

    #[test]
    fn argument_hash_ignores_key_order() {
        let a = serde_json::json!({"a": 1, "b": [1, 2]});
        let b = serde_json::json!({"b": [1, 2], "a": 1});
        assert_eq!(hash_arguments(&a), hash_arguments(&b));
        assert_ne!(
            hash_arguments(&a),
            hash_arguments(&serde_json::json!({"a": 2}))
        );
    }
}
