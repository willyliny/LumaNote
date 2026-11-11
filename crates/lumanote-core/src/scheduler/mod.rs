//! Persistent scheduler for user-requested reminders (handbook sections 9 and 11).
//!
//! Inferred suggestions are a different system (section 6) and are not handled here.
//!
//! # Storage mapping
//!
//! The existing `schedules`/`deliveries` columns are used as
//! `next_run_at` (UTC), `timezone` (IANA), `recurrence` (RRULE subset, see [`recurrence`]),
//! `misfire_policy` ([`MisfirePolicy`]), `enabled`, and `task_template` (JSON holding title,
//! body, the user's original rule, action kind, usefulness window, payload). A delivery's
//! `occurrence_key` is the due instant as RFC 3339 UTC text, so it is unique per schedule
//! and stable across restarts, timezone edits and DST changes.
//!
//! # Delivery guarantees
//!
//! For each due occurrence the engine, in ONE transaction, inserts the `deliveries` row and
//! advances `schedules.next_run_at`; only afterwards does it call the `Notifier`, then marks
//! the row `delivered`. The unique `(schedule_id, occurrence_key)` constraint means an
//! occurrence that already has a row is never notified again by the due-time path.
//!
//! The window between "row written" and "notified" is **at-least-once**: the row stays
//! `pending` until the notifier succeeds, and every engine start (and every retry after a
//! notifier error) re-sends pending rows. A crash after the toast but before the row is
//! marked `delivered` can therefore show one reminder twice. That is the deliberate trade:
//! losing a reminder the user asked for is worse than a rare duplicate toast, and the
//! reminder notification is idempotent for the user. Actions that must never repeat or run
//! late (external writes, payments) are never executed by the scheduler at all; the
//! scheduler only notifies, so repeating that notice is harmless.
//!
//! # Time
//!
//! All time comes from [`Clock`](crate::ports::Clock). The loop sleeps at most
//! `max_sleep` per step and recomputes everything from `now_utc` after each wake, so a
//! system clock correction (forward or backward) or a resume from suspend is handled by
//! re-evaluation rather than by trusting elapsed monotonic time. Schedules carry their own
//! IANA timezone and never read the machine's timezone, so changing the machine timezone
//! does not move any schedule.
//!
//! # Deletion
//!
//! Deleting a schedule is a soft delete (`schedules.deleted_at`, migration 0003): handbook
//! section 9 requires trigger records to be kept. A deleted schedule is excluded from
//! listings (unless asked for), from every engine query and from update/enable/snooze; its
//! `pending` notifications are cancelled in the same transaction; the rest of its
//! `deliveries` stay queryable and the `(schedule_id, occurrence_key)` constraint still
//! applies.

mod engine;
mod model;
pub mod recurrence;
mod vague;

#[cfg(test)]
mod tests;

pub use engine::{share_store, Scheduler, SharedStore};
pub use model::{
    ActionKind, Delivery, DeliveryFilter, DeliveryId, DeliveryStatus, MisfirePolicy, NewSchedule,
    Schedule, ScheduleId, ScheduleUpdate, SchedulerConfig, SnoozeUntil, StartTime, TickReport,
};
pub use recurrence::{Freq, LocalTimeKind, Recurrence, RepeatSpec};
pub use vague::{
    resolve_vague, ClarificationReason, DayPart, DayRef, TimeDefaults, TimeResolution, VagueTime,
};
