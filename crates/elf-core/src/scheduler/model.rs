//! Public data types of the scheduler: identifiers, enums, requests and read models.

use std::fmt;
use std::time::Duration;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::recurrence::{Recurrence, RepeatSpec};
use super::vague::TimeDefaults;
use crate::schema::{str_enum, Timestamp};

macro_rules! sched_id {
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

sched_id!(
    /// Identifier of a schedule.
    ScheduleId
);
sched_id!(
    /// Identifier of a delivery (one occurrence of a schedule).
    DeliveryId
);

str_enum!(
    /// What a schedule is for. Anything other than a reminder is a side-effecting action.
    ActionKind {
        /// A plain notification to the user.
        Reminder => "reminder",
        /// A task that writes to an external system (send, create, delete, ...).
        ExternalWrite => "external_write",
        /// A task that spends money.
        Payment => "payment",
    }
);

str_enum!(
    /// What to do with occurrences missed while the app was not running (column `misfire_policy`).
    MisfirePolicy {
        /// Late occurrences within the usefulness window are merged into one catch-up
        /// notification; older ones are marked missed. The default for reminders.
        Coalesce => "coalesce",
        /// Every late occurrence is marked missed; nothing is delivered late.
        Skip => "skip",
        /// Late occurrences are never delivered or executed; they are marked
        /// `needs_attention` for the user to decide. Mandatory for external writes and payments.
        NeedsAttention => "needs_attention",
    }
);

str_enum!(
    /// State of one occurrence in `deliveries.status`.
    DeliveryStatus {
        /// Row written, notification not yet confirmed (re-sent after a crash).
        Pending => "pending",
        /// The notification was shown.
        Delivered => "delivered",
        /// Occurrence was too old to be useful; not delivered.
        Missed => "missed",
        /// Late occurrence of an action that must not run late; waiting for the user.
        NeedsAttention => "needs_attention",
        /// The user marked it done.
        Done => "done",
        /// The user cancelled it.
        Cancelled => "cancelled",
        /// The user postponed it; a one-shot follow-up schedule exists.
        Snoozed => "snoozed",
    }
);

/// When a schedule starts / fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StartTime {
    /// An absolute instant (UTC).
    Instant {
        /// The instant.
        at: Timestamp,
    },
    /// A wall-clock time in the schedule's timezone.
    Local {
        /// The local date and time.
        at: NaiveDateTime,
    },
}

/// Request to create a schedule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewSchedule {
    /// Short title shown in the notification.
    pub title: String,
    /// Notification body.
    pub body: String,
    /// IANA timezone name, e.g. `Asia/Taipei`.
    pub timezone: String,
    /// First occurrence (one-shot) or anchor of the recurrence.
    pub start: StartTime,
    /// Recurrence; `None` makes a one-shot schedule.
    pub repeat: Option<RepeatSpec>,
    /// The user's own words / rule text, kept verbatim (handbook section 11). If absent,
    /// the RRULE text or the ISO start time is stored.
    pub original_rule: Option<String>,
    /// What the schedule does.
    pub action: ActionKind,
    /// Misfire policy; defaults to `coalesce` for reminders and is forced to
    /// `needs_attention` for external writes and payments.
    pub misfire_policy: Option<MisfirePolicy>,
    /// How long after the due time a late notification is still useful, in seconds.
    /// Defaults to [`SchedulerConfig::default_useful_window`].
    pub useful_window_secs: Option<u64>,
    /// Opaque data the shell may attach (for example a task template); returned verbatim.
    pub payload: Value,
}

impl NewSchedule {
    /// A reminder with sensible defaults.
    pub fn reminder(
        title: impl Into<String>,
        body: impl Into<String>,
        timezone: impl Into<String>,
        start: StartTime,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            timezone: timezone.into(),
            start,
            repeat: None,
            original_rule: None,
            action: ActionKind::Reminder,
            misfire_policy: None,
            useful_window_secs: None,
            payload: Value::Null,
        }
    }
}

/// Partial update of a schedule. `None` fields are left as they are.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ScheduleUpdate {
    /// New title.
    pub title: Option<String>,
    /// New body.
    pub body: Option<String>,
    /// New timezone. For a one-shot schedule `start` must be given too (the wall-clock
    /// time is re-interpreted in the new zone); a recurring schedule keeps its local time.
    pub timezone: Option<String>,
    /// New start / anchor; recomputes the next run.
    pub start: Option<StartTime>,
    /// `Some(Some(r))` sets a recurrence, `Some(None)` makes the schedule one-shot.
    pub repeat: Option<Option<RepeatSpec>>,
    /// New original rule text.
    pub original_rule: Option<String>,
    /// New misfire policy.
    pub misfire_policy: Option<MisfirePolicy>,
    /// New usefulness window in seconds.
    pub useful_window_secs: Option<u64>,
    /// New payload.
    pub payload: Option<Value>,
}

/// A stored schedule as seen by the shell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schedule {
    /// Identifier.
    pub id: ScheduleId,
    /// Title.
    pub title: String,
    /// Body.
    pub body: String,
    /// IANA timezone name.
    pub timezone: String,
    /// UTC instant of the next occurrence; `None` when a one-shot has been handled.
    pub next_run_at: Option<Timestamp>,
    /// Recurrence, `None` for one-shot schedules.
    pub recurrence: Option<Recurrence>,
    /// The user's original rule text.
    pub original_rule: String,
    /// What the schedule does.
    pub action: ActionKind,
    /// Effective misfire policy.
    pub misfire_policy: MisfirePolicy,
    /// Usefulness window in seconds.
    pub useful_window_secs: u64,
    /// Whether the schedule is active.
    pub enabled: bool,
    /// Opaque payload.
    pub payload: Value,
    /// Creation time.
    pub created_at: Timestamp,
    /// If this schedule is a snooze follow-up, the delivery it came from.
    pub snoozed_from: Option<DeliveryId>,
}

/// One occurrence of a schedule and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    /// Identifier.
    pub id: DeliveryId,
    /// Owning schedule.
    pub schedule_id: ScheduleId,
    /// Occurrence key (the due instant, RFC 3339 UTC).
    pub occurrence_key: String,
    /// The occurrence's scheduled instant.
    pub due_at: Timestamp,
    /// Current state.
    pub status: DeliveryStatus,
    /// When the notification was confirmed shown.
    pub delivered_at: Option<Timestamp>,
}

/// Filter for [`Scheduler::list_deliveries`](super::Scheduler::list_deliveries).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeliveryFilter {
    /// Only this schedule.
    pub schedule_id: Option<ScheduleId>,
    /// Only this status.
    pub status: Option<DeliveryStatus>,
    /// Maximum rows (default 100).
    pub limit: Option<u32>,
}

/// When to re-remind after a snooze.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SnoozeUntil {
    /// This many seconds from now.
    After {
        /// Delay in seconds (> 0).
        secs: u64,
    },
    /// At an absolute instant, which must be in the future.
    At {
        /// The instant.
        at: Timestamp,
    },
}

/// Engine tunables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerConfig {
    /// An occurrence this close to (or after) its due time still counts as on time.
    pub on_time_grace: Duration,
    /// Default usefulness window for late occurrences.
    pub default_useful_window: Duration,
    /// Longest single sleep. After every sleep the engine re-reads the wall clock, so this
    /// bounds how long a system time change goes unnoticed.
    pub max_sleep: Duration,
    /// Wall-clock/monotonic disagreement above which a clock jump is recorded.
    pub clock_jump_threshold: Duration,
    /// Delay before retrying a notification the notifier refused.
    pub retry_delay: Duration,
    /// Defaults for vague times such as "tomorrow morning"; none by default.
    pub time_defaults: TimeDefaults,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            on_time_grace: Duration::from_secs(120),
            default_useful_window: Duration::from_secs(12 * 3600),
            max_sleep: Duration::from_secs(30),
            clock_jump_threshold: Duration::from_secs(5),
            retry_delay: Duration::from_secs(30),
            time_defaults: TimeDefaults::default(),
        }
    }
}

/// What one evaluation pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TickReport {
    /// Occurrences recorded as pending notification.
    pub due: usize,
    /// Occurrences recorded as missed.
    pub missed: usize,
    /// Occurrences recorded as needing attention.
    pub needs_attention: usize,
    /// Notifications shown (a catch-up notification counts once).
    pub notifications_sent: usize,
}
