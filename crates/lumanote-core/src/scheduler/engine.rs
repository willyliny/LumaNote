//! The scheduler service: shell-facing API plus the background loop.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, NaiveDateTime};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::model::{
    ActionKind, Delivery, DeliveryFilter, DeliveryId, DeliveryStatus, MisfirePolicy, NewSchedule,
    Schedule, ScheduleId, ScheduleUpdate, SchedulerConfig, SnoozeUntil, StartTime, TickReport,
};
use super::recurrence::{resolve_local, Recurrence};
use super::vague::{resolve_vague, TimeResolution, VagueTime};
use crate::ports::{Clock, Notification, Notifier};
use crate::schema::{ts_from_sql, ts_to_sql, LumaError, Result, Timestamp};
use crate::storage::schedules::{DeliveryRow, NewDeliveryRow, ScheduleRow};
use crate::storage::Store;

/// The database handle shared between the shell and the scheduler. The lock is only ever
/// held for synchronous database work, never across an `.await`.
pub type SharedStore = Arc<Mutex<Store>>;

/// Wraps a [`Store`] for sharing.
pub fn share_store(store: Store) -> SharedStore {
    Arc::new(Mutex::new(store))
}

/// Upper bound on occurrences enumerated for one schedule in one pass.
const MAX_ENUMERATED: usize = 20_000;
/// At most this many *missed* occurrences are recorded per pass and schedule; older ones
/// are skipped silently (a daily reminder off for years would otherwise add thousands of rows).
const MAX_MISSED_RECORDS: usize = 100;
/// At most this many occurrence times are listed in a catch-up notification.
const MAX_LISTED: usize = 10;

/// JSON stored in `schedules.task_template`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Template {
    v: u32,
    title: String,
    body: String,
    original_rule: String,
    action: ActionKind,
    useful_window_secs: u64,
    payload: Value,
    created_at: Timestamp,
    snoozed_from: Option<String>,
}

fn parse_tz(name: &str) -> Result<Tz> {
    Tz::from_str(name)
        .map_err(|_| LumaError::invalid_input(format!("unknown IANA timezone {name:?}")))
}

fn secs_i64(secs: u64) -> i64 {
    i64::try_from(secs).unwrap_or(i64::MAX / 4)
}

fn micro() -> ChronoDuration {
    ChronoDuration::microseconds(1)
}

fn row_to_schedule(row: &ScheduleRow) -> Result<Schedule> {
    let t: Template = serde_json::from_str(&row.task_template).map_err(|e| LumaError::Storage {
        message: format!("bad schedule template for {}: {e}", row.id),
    })?;
    Ok(Schedule {
        id: ScheduleId::from_string(row.id.clone()),
        title: t.title,
        body: t.body,
        timezone: row.timezone.clone(),
        next_run_at: row.next_run_at,
        recurrence: row
            .recurrence
            .as_deref()
            .map(Recurrence::parse)
            .transpose()?,
        original_rule: t.original_rule,
        action: t.action,
        misfire_policy: MisfirePolicy::parse(&row.misfire_policy)?,
        useful_window_secs: t.useful_window_secs,
        enabled: row.enabled,
        payload: t.payload,
        created_at: t.created_at,
        snoozed_from: t.snoozed_from.map(DeliveryId::from_string),
        deleted_at: row.deleted_at,
    })
}

fn schedule_to_row(s: &Schedule) -> Result<ScheduleRow> {
    let template = Template {
        v: 1,
        title: s.title.clone(),
        body: s.body.clone(),
        original_rule: s.original_rule.clone(),
        action: s.action,
        useful_window_secs: s.useful_window_secs,
        payload: s.payload.clone(),
        created_at: s.created_at,
        snoozed_from: s.snoozed_from.as_ref().map(|d| d.as_str().to_owned()),
    };
    Ok(ScheduleRow {
        id: s.id.as_str().to_owned(),
        task_template: serde_json::to_string(&template).map_err(|e| LumaError::Storage {
            message: format!("cannot serialise schedule template: {e}"),
        })?,
        timezone: s.timezone.clone(),
        next_run_at: s.next_run_at,
        recurrence: s.recurrence.as_ref().map(Recurrence::to_rrule),
        misfire_policy: s.misfire_policy.as_str().to_owned(),
        enabled: s.enabled,
        deleted_at: s.deleted_at,
    })
}

/// The live (not soft-deleted) schedule row, or an "unknown schedule" error.
fn live_row(store: &Store, id: &ScheduleId) -> Result<ScheduleRow> {
    store
        .get_schedule_row(id.as_str())?
        .filter(|r| r.deleted_at.is_none())
        .ok_or_else(|| LumaError::invalid_input(format!("unknown schedule {id}")))
}

fn row_to_delivery(row: &DeliveryRow) -> Result<Delivery> {
    Ok(Delivery {
        id: DeliveryId::from_string(row.id.clone()),
        schedule_id: ScheduleId::from_string(row.schedule_id.clone()),
        occurrence_key: row.occurrence_key.clone(),
        due_at: ts_from_sql(&row.occurrence_key)?,
        status: DeliveryStatus::parse(&row.status)?,
        delivered_at: row.delivered_at,
    })
}

/// Local start of a request, resolved in a timezone.
fn resolve_start(start: &StartTime, tz: Tz) -> Result<(NaiveDateTime, Timestamp)> {
    match start {
        StartTime::Instant { at } => Ok((at.with_timezone(&tz).naive_local(), *at)),
        StartTime::Local { at } => Ok((*at, resolve_local(tz, *at)?.instant)),
    }
}

/// Computes the first `next_run_at` for a start / recurrence pair.
fn first_run(
    rec: Option<&Recurrence>,
    tz: Tz,
    start_instant: Timestamp,
    now: Timestamp,
) -> Result<Timestamp> {
    match rec {
        None => {
            if start_instant < now {
                return Err(LumaError::invalid_input(
                    "the requested time is already in the past",
                ));
            }
            Ok(start_instant)
        }
        Some(rec) => {
            let after = if start_instant > now {
                start_instant - micro()
            } else {
                now
            };
            rec.next_after(tz, after)
        }
    }
}

/// Persistent scheduler for user-requested reminders. Cheap to clone; clones share state,
/// so the shell can keep one handle for the API and run another in a background task.
#[derive(Clone)]
pub struct Scheduler {
    store: SharedStore,
    clock: Arc<dyn Clock>,
    notifier: Arc<dyn Notifier>,
    config: SchedulerConfig,
    shared: Arc<Shared>,
}

struct Shared {
    wake: Notify,
    /// True when `deliveries` may hold `pending` rows that need a notification attempt.
    /// Starts true so a freshly started engine recovers rows left by a crash.
    dirty: AtomicBool,
    clock_jumps: AtomicU64,
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Scheduler {
    /// Creates a scheduler over an already migrated database.
    pub fn new(
        store: SharedStore,
        clock: Arc<dyn Clock>,
        notifier: Arc<dyn Notifier>,
        config: SchedulerConfig,
    ) -> Self {
        Self {
            store,
            clock,
            notifier,
            config,
            shared: Arc::new(Shared {
                wake: Notify::new(),
                dirty: AtomicBool::new(true),
                clock_jumps: AtomicU64::new(0),
            }),
        }
    }

    pub(super) fn db(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wake(&self) {
        self.shared.wake.notify_one();
    }

    /// Number of wall-clock jumps (system time change, resume from sleep) the loop has noticed.
    pub fn clock_jumps_detected(&self) -> u64 {
        self.shared.clock_jumps.load(Ordering::Relaxed)
    }

    // ------------------------------------------------------------------ API

    /// Creates a schedule and wakes the loop.
    pub fn create_schedule(&self, new: NewSchedule) -> Result<Schedule> {
        let now = self.clock.now_utc();
        if new.title.trim().is_empty() {
            return Err(LumaError::invalid_input("title must not be empty"));
        }
        let tz = parse_tz(&new.timezone)?;
        let (start_local, start_instant) = resolve_start(&new.start, tz)?;
        let policy = match (new.action, new.misfire_policy) {
            (ActionKind::Reminder, p) => p.unwrap_or(MisfirePolicy::Coalesce),
            (_, None | Some(MisfirePolicy::NeedsAttention)) => MisfirePolicy::NeedsAttention,
            (action, Some(p)) => {
                return Err(LumaError::invalid_input(format!(
                    "{action} schedules must use misfire policy needs_attention, not {p}"
                )))
            }
        };
        let rec = new
            .repeat
            .as_ref()
            .map(|spec| Recurrence::new(spec, start_local))
            .transpose()?;
        let next = first_run(rec.as_ref(), tz, start_instant, now)?;
        let original_rule = new
            .original_rule
            .clone()
            .or_else(|| rec.as_ref().map(Recurrence::to_rrule))
            .unwrap_or_else(|| ts_to_sql(&start_instant));
        let schedule = Schedule {
            id: ScheduleId::new(),
            title: new.title,
            body: new.body,
            timezone: new.timezone,
            next_run_at: Some(next),
            recurrence: rec,
            original_rule,
            action: new.action,
            misfire_policy: policy,
            useful_window_secs: new
                .useful_window_secs
                .unwrap_or(self.config.default_useful_window.as_secs()),
            enabled: true,
            payload: new.payload,
            created_at: now,
            snoozed_from: None,
            deleted_at: None,
        };
        self.db()
            .insert_schedule_row(&schedule_to_row(&schedule)?)?;
        self.wake();
        Ok(schedule)
    }

    /// Applies a partial update; time-related changes recompute the next run.
    pub fn update_schedule(&self, id: &ScheduleId, update: ScheduleUpdate) -> Result<Schedule> {
        let now = self.clock.now_utc();
        let mut store = self.db();
        let row = live_row(&store, id)?;
        let mut s = row_to_schedule(&row)?;

        if let Some(t) = update.title {
            if t.trim().is_empty() {
                return Err(LumaError::invalid_input("title must not be empty"));
            }
            s.title = t;
        }
        if let Some(b) = update.body {
            s.body = b;
        }
        if let Some(r) = update.original_rule {
            s.original_rule = r;
        }
        if let Some(w) = update.useful_window_secs {
            s.useful_window_secs = w;
        }
        if let Some(p) = update.payload {
            s.payload = p;
        }
        if let Some(p) = update.misfire_policy {
            if s.action != ActionKind::Reminder && p != MisfirePolicy::NeedsAttention {
                return Err(LumaError::invalid_input(format!(
                    "{} schedules must use misfire policy needs_attention",
                    s.action
                )));
            }
            s.misfire_policy = p;
        }

        let time_changed =
            update.timezone.is_some() || update.start.is_some() || update.repeat.is_some();
        if time_changed {
            let old_tz = parse_tz(&s.timezone)?;
            let tz_changed = update.timezone.is_some();
            let tz_name = update.timezone.unwrap_or_else(|| s.timezone.clone());
            let tz = parse_tz(&tz_name)?;
            let spec = match &update.repeat {
                Some(r) => r.clone(),
                None => s.recurrence.as_ref().map(Recurrence::to_spec),
            };
            let (start_local, start_instant) =
                match (&update.start, &s.recurrence, s.next_run_at) {
                    (Some(start), _, _) => resolve_start(start, tz)?,
                    (None, Some(old), _) => {
                        let local = old.anchor.and_time(old.time);
                        (local, resolve_local(tz, local)?.instant)
                    }
                    (None, None, Some(next)) if !tz_changed => {
                        (next.with_timezone(&old_tz).naive_local(), next)
                    }
                    (None, None, _) => return Err(LumaError::invalid_input(
                        "changing the timezone of a one-shot schedule requires a new start time",
                    )),
                };
            let rec = spec
                .as_ref()
                .map(|sp| Recurrence::new(sp, start_local))
                .transpose()?;
            s.next_run_at = Some(first_run(rec.as_ref(), tz, start_instant, now)?);
            s.recurrence = rec;
            s.timezone = tz_name;
        }
        store.replace_schedule_row(&schedule_to_row(&s)?)?;
        drop(store);
        self.wake();
        Ok(s)
    }

    /// Enables or disables a schedule. Re-enabling never back-fills the disabled period: a
    /// recurring schedule resumes at its next future occurrence; a one-shot whose time has
    /// already passed is rejected (update its time first).
    pub fn set_enabled(&self, id: &ScheduleId, enabled: bool) -> Result<Schedule> {
        let now = self.clock.now_utc();
        let mut store = self.db();
        let row = live_row(&store, id)?;
        let mut s = row_to_schedule(&row)?;
        if enabled && !s.enabled {
            if let Some(next) = s.next_run_at {
                if next <= now {
                    match &s.recurrence {
                        Some(rec) => {
                            s.next_run_at = Some(rec.next_after(parse_tz(&s.timezone)?, now)?)
                        }
                        None => {
                            return Err(LumaError::invalid_input(
                                "the one-shot time has passed; update the schedule's time first",
                            ))
                        }
                    }
                }
            }
        }
        s.enabled = enabled;
        store.replace_schedule_row(&schedule_to_row(&s)?)?;
        drop(store);
        self.wake();
        Ok(s)
    }

    /// Deletes a schedule (soft delete). It stops firing at once and disappears from
    /// listings; its delivery history is kept (handbook section 9) and stays queryable
    /// through [`Scheduler::list_deliveries`]. Notifications still `pending` for it are
    /// cancelled. Deleting an unknown or already deleted schedule is an error.
    pub fn delete_schedule(&self, id: &ScheduleId) -> Result<()> {
        let now = self.clock.now_utc();
        let deleted = self.db().soft_delete_schedule_row(id.as_str(), now)?;
        if !deleted {
            return Err(LumaError::invalid_input(format!("unknown schedule {id}")));
        }
        self.wake();
        Ok(())
    }

    /// Loads one schedule; a deleted schedule is reported as unknown.
    pub fn get_schedule(&self, id: &ScheduleId) -> Result<Schedule> {
        row_to_schedule(&live_row(&self.db(), id)?)
    }

    /// Loads one schedule even if it was deleted (for showing the history of a deleted
    /// reminder); `deleted_at` tells which.
    pub fn get_schedule_including_deleted(&self, id: &ScheduleId) -> Result<Schedule> {
        let row = self
            .db()
            .get_schedule_row(id.as_str())?
            .ok_or_else(|| LumaError::invalid_input(format!("unknown schedule {id}")))?;
        row_to_schedule(&row)
    }

    /// All live schedules in creation order (deleted ones excluded).
    pub fn list_schedules(&self) -> Result<Vec<Schedule>> {
        self.list_schedules_with(false)
    }

    /// Schedules in creation order; with `include_deleted` the soft-deleted ones are
    /// listed too (marked by `deleted_at`).
    pub fn list_schedules_with(&self, include_deleted: bool) -> Result<Vec<Schedule>> {
        self.db()
            .list_schedule_rows(include_deleted)?
            .iter()
            .map(row_to_schedule)
            .collect()
    }

    /// Delivery history, newest occurrence first.
    pub fn list_deliveries(&self, filter: &DeliveryFilter) -> Result<Vec<Delivery>> {
        self.db()
            .list_delivery_rows(
                filter.schedule_id.as_ref().map(ScheduleId::as_str),
                filter.status.map(DeliveryStatus::as_str),
                filter.limit.unwrap_or(100),
            )?
            .iter()
            .map(row_to_delivery)
            .collect()
    }

    /// "稍後提醒": postpones a delivered reminder by creating a one-shot follow-up schedule
    /// (same title, body, timezone and action) and marking the delivery `snoozed`, atomically.
    pub fn snooze(&self, delivery: &DeliveryId, until: SnoozeUntil) -> Result<Schedule> {
        let now = self.clock.now_utc();
        let at = match until {
            SnoozeUntil::After { secs } => now + ChronoDuration::seconds(secs_i64(secs)),
            SnoozeUntil::At { at } => at,
        };
        if at <= now {
            return Err(LumaError::invalid_input("snooze time must be in the future"));
        }
        let mut store = self.db();
        let d = store
            .get_delivery_row(delivery.as_str())?
            .ok_or_else(|| LumaError::invalid_input(format!("unknown delivery {delivery}")))?;
        let origin_row = store
            .get_schedule_row(&d.schedule_id)?
            .filter(|r| r.deleted_at.is_none())
            .ok_or_else(|| LumaError::invalid_input("the delivery's schedule no longer exists"))?;
        let origin = row_to_schedule(&origin_row)?;
        let follow_up = Schedule {
            id: ScheduleId::new(),
            title: origin.title,
            body: origin.body,
            timezone: origin.timezone,
            next_run_at: Some(at),
            recurrence: None,
            original_rule: format!("snooze:{}", ts_to_sql(&at)),
            action: origin.action,
            misfire_policy: origin.misfire_policy,
            useful_window_secs: origin.useful_window_secs,
            enabled: true,
            payload: origin.payload,
            created_at: now,
            snoozed_from: Some(delivery.clone()),
            deleted_at: None,
        };
        if !store.snooze_delivery(delivery.as_str(), &schedule_to_row(&follow_up)?)? {
            return Err(LumaError::InvalidTransition {
                from: d.status,
                to: DeliveryStatus::Snoozed.to_string(),
                reason: Some("only a delivered reminder can be snoozed".into()),
            });
        }
        drop(store);
        self.wake();
        Ok(follow_up)
    }

    /// Marks a delivered (or needs-attention) occurrence as done.
    pub fn mark_done(&self, delivery: &DeliveryId) -> Result<Delivery> {
        self.finish_delivery(delivery, DeliveryStatus::Done)
    }

    /// Marks a delivered (or needs-attention) occurrence as cancelled.
    pub fn mark_cancelled(&self, delivery: &DeliveryId) -> Result<Delivery> {
        self.finish_delivery(delivery, DeliveryStatus::Cancelled)
    }

    fn finish_delivery(&self, id: &DeliveryId, to: DeliveryStatus) -> Result<Delivery> {
        let mut store = self.db();
        let row = store
            .get_delivery_row(id.as_str())?
            .ok_or_else(|| LumaError::invalid_input(format!("unknown delivery {id}")))?;
        let from = [
            DeliveryStatus::Delivered.as_str(),
            DeliveryStatus::NeedsAttention.as_str(),
        ];
        if !store.transition_delivery(id.as_str(), &from, to.as_str())? {
            return Err(LumaError::InvalidTransition {
                from: row.status,
                to: to.to_string(),
                reason: Some(
                    "only delivered or needs_attention occurrences can be completed".into(),
                ),
            });
        }
        let updated = store
            .get_delivery_row(id.as_str())?
            .ok_or_else(|| LumaError::invalid_input(format!("unknown delivery {id}")))?;
        row_to_delivery(&updated)
    }

    /// Resolves a vague time ("明早") with the configured defaults. Never guesses: returns
    /// [`TimeResolution::NeedsClarification`] when no default applies.
    pub fn resolve_vague_time(&self, vague: VagueTime, timezone: &str) -> Result<TimeResolution> {
        resolve_vague(
            vague,
            &self.config.time_defaults,
            parse_tz(timezone)?,
            self.clock.now_utc(),
        )
    }

    // --------------------------------------------------------------- engine

    /// One evaluation pass against `Clock::now_utc`: records every due occurrence (atomically
    /// with advancing the schedule), then sends pending notifications. [`Scheduler::run`] calls
    /// this in a loop; it is public for shells that drive their own loop and for tests.
    pub async fn tick(&self) -> Result<TickReport> {
        let now = self.clock.now_utc();
        let (mut report, attention) = self.commit_due(now)?;
        report.notifications_sent += self.flush_pending().await?;
        for n in attention {
            // Informational only; the rows are already durable and listed as needs_attention.
            if self.notifier.notify(&n).await.is_ok() {
                report.notifications_sent += 1;
            }
        }
        Ok(report)
    }

    /// Runs until `shutdown` is cancelled. Sleeps through the injected [`Clock`] until the
    /// next due time (at most `max_sleep` at a time, re-reading the wall clock after every
    /// sleep so time changes are noticed), and wakes early when schedules change.
    pub async fn run(&self, shutdown: CancellationToken) -> Result<()> {
        loop {
            if shutdown.is_cancelled() {
                return Ok(());
            }
            self.tick().await?;
            let wall_before = self.clock.now_utc();
            let mono_before = self.clock.monotonic();
            let sleep_for = self.next_sleep()?;
            tokio::select! {
                biased;
                () = shutdown.cancelled() => return Ok(()),
                () = self.shared.wake.notified() => {}
                () = self.clock.sleep(sleep_for) => {}
            }
            self.note_clock_jump(wall_before, mono_before);
        }
    }

    fn note_clock_jump(&self, wall_before: Timestamp, mono_before: crate::ports::MonoInstant) {
        let wall = self.clock.now_utc() - wall_before;
        let mono = self.clock.monotonic().duration_since(mono_before);
        let mono = ChronoDuration::from_std(mono).unwrap_or(ChronoDuration::zero());
        let threshold = ChronoDuration::from_std(self.config.clock_jump_threshold)
            .unwrap_or(ChronoDuration::seconds(5));
        if (wall - mono).abs() > threshold {
            self.shared.clock_jumps.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn next_sleep(&self) -> Result<Duration> {
        let now = self.clock.now_utc();
        let mut d = self.config.max_sleep;
        if let Some(next) = self.db().earliest_next_run()? {
            d = d.min((next - now).to_std().unwrap_or(Duration::ZERO));
        }
        if self.shared.dirty.load(Ordering::Relaxed) {
            d = d.min(self.config.retry_delay);
        }
        Ok(d.max(Duration::from_millis(1)))
    }

    /// Phase 1 of a pass (synchronous, durable): for every enabled schedule due at `now`,
    /// classifies its occurrences and writes the `deliveries` rows and the advanced
    /// `next_run_at` in one transaction. Nothing is notified here. Returns notices for
    /// occurrences that need the user's attention.
    pub(crate) fn commit_due(&self, now: Timestamp) -> Result<(TickReport, Vec<Notification>)> {
        let mut report = TickReport::default();
        let mut notices = Vec::new();
        // The lock is held for the whole pass, so a concurrent disable/delete is ordered
        // strictly before or after it.
        let mut store = self.db();
        for id in store.due_schedule_ids(now)? {
            let Some(row) = store.get_schedule_row(&id)? else {
                continue;
            };
            if !row.enabled || row.deleted_at.is_some() || row.next_run_at.is_none_or(|n| n > now) {
                continue;
            }
            match self.plan_schedule(&row, now) {
                Ok(plan) => {
                    let new_rows: Vec<NewDeliveryRow> = plan
                        .rows
                        .iter()
                        .map(|(due, status)| NewDeliveryRow {
                            id: DeliveryId::new().to_string(),
                            occurrence_key: ts_to_sql(due),
                            status: status.as_str().to_owned(),
                        })
                        .collect();
                    let inserted = store.commit_occurrences(&id, &new_rows, plan.next)?;
                    let mut attention_dues = Vec::new();
                    for (due, status) in &plan.rows {
                        // A key that already existed was handled before: count and notify nothing.
                        if !inserted.contains(&ts_to_sql(due)) {
                            continue;
                        }
                        match status {
                            DeliveryStatus::Pending => report.due += 1,
                            DeliveryStatus::Missed => report.missed += 1,
                            DeliveryStatus::NeedsAttention => {
                                report.needs_attention += 1;
                                attention_dues.push(*due);
                            }
                            _ => {}
                        }
                    }
                    if report.due > 0 {
                        self.shared.dirty.store(true, Ordering::Relaxed);
                    }
                    if !attention_dues.is_empty() {
                        notices.push(attention_notice(&plan.schedule, &attention_dues));
                    }
                }
                Err(_) => {
                    // A schedule that cannot be evaluated (corrupt row, unknown timezone) is
                    // quarantined instead of wedging the loop.
                    let mut quarantined = row.clone();
                    quarantined.enabled = false;
                    store.replace_schedule_row(&quarantined)?;
                }
            }
        }
        Ok((report, notices))
    }

    fn plan_schedule(&self, row: &ScheduleRow, now: Timestamp) -> Result<Plan> {
        let schedule = row_to_schedule(row)?;
        let tz = parse_tz(&schedule.timezone)?;
        let first = schedule
            .next_run_at
            .ok_or_else(|| LumaError::invalid_input("schedule has no next run"))?;

        let mut dues = vec![first];
        let mut next = None;
        if let Some(rec) = &schedule.recurrence {
            let mut cur = first;
            for _ in 0..MAX_ENUMERATED {
                let n = rec.next_after(tz, cur)?;
                if n > now {
                    next = Some(n);
                    break;
                }
                dues.push(n);
                cur = n;
            }
            if next.is_none() {
                return Err(LumaError::invalid_input("too many missed occurrences"));
            }
        }

        let policy = if schedule.action == ActionKind::Reminder {
            schedule.misfire_policy
        } else {
            MisfirePolicy::NeedsAttention
        };
        let grace =
            ChronoDuration::from_std(self.config.on_time_grace).unwrap_or(ChronoDuration::zero());
        let window = ChronoDuration::seconds(secs_i64(schedule.useful_window_secs));
        let mut rows: Vec<(Timestamp, DeliveryStatus)> = dues
            .into_iter()
            .map(|due| {
                let late = now - due;
                let status = if late <= grace {
                    DeliveryStatus::Pending
                } else {
                    match policy {
                        MisfirePolicy::Skip => DeliveryStatus::Missed,
                        MisfirePolicy::NeedsAttention => DeliveryStatus::NeedsAttention,
                        MisfirePolicy::Coalesce if late > window => DeliveryStatus::Missed,
                        MisfirePolicy::Coalesce => DeliveryStatus::Pending,
                    }
                };
                (due, status)
            })
            .collect();
        let missed = rows
            .iter()
            .filter(|(_, s)| *s == DeliveryStatus::Missed)
            .count();
        if missed > MAX_MISSED_RECORDS {
            let mut to_drop = missed - MAX_MISSED_RECORDS;
            rows.retain(|(_, s)| {
                if *s == DeliveryStatus::Missed && to_drop > 0 {
                    to_drop -= 1;
                    false
                } else {
                    true
                }
            });
        }
        Ok(Plan {
            schedule,
            rows,
            next,
        })
    }

    /// Phase 2 of a pass: sends one notification per schedule for its `pending` rows and
    /// marks them delivered. A notifier failure leaves the rows pending for a retry.
    async fn flush_pending(&self) -> Result<usize> {
        if !self.shared.dirty.swap(false, Ordering::Relaxed) {
            return Ok(0);
        }
        let batches = self.pending_batches()?;
        let mut sent = 0;
        let mut failed = false;
        for (notification, ids) in batches {
            match self.notifier.notify(&notification).await {
                Ok(()) => {
                    // If we crash right here the rows stay pending and are re-sent after
                    // restart: delivery is at-least-once (see the module docs).
                    self.db()
                        .mark_deliveries_delivered(&ids, self.clock.now_utc())?;
                    sent += 1;
                }
                Err(_) => failed = true,
            }
        }
        if failed {
            self.shared.dirty.store(true, Ordering::Relaxed);
        }
        Ok(sent)
    }

    pub(super) fn pending_batches(&self) -> Result<Vec<(Notification, Vec<String>)>> {
        let now = self.clock.now_utc();
        let store = self.db();
        let pending = store.pending_delivery_rows()?;
        let mut batches: Vec<(Notification, Vec<String>)> = Vec::new();
        let mut i = 0;
        while i < pending.len() {
            let schedule_id = &pending[i].schedule_id;
            let mut j = i;
            while j < pending.len() && pending[j].schedule_id == *schedule_id {
                j += 1;
            }
            let group = &pending[i..j];
            i = j;
            let Some(row) = store
                .get_schedule_row(schedule_id)?
                .filter(|r| r.deleted_at.is_none())
            else {
                continue;
            };
            let (title, body, tz) = match row_to_schedule(&row) {
                Ok(s) => (s.title, s.body, parse_tz(&s.timezone).unwrap_or(Tz::UTC)),
                Err(_) => ("提醒".to_owned(), String::new(), Tz::UTC),
            };
            let dues = group
                .iter()
                .map(|r| ts_from_sql(&r.occurrence_key))
                .collect::<Result<Vec<_>>>()?;
            let grace = ChronoDuration::from_std(self.config.on_time_grace)
                .unwrap_or(ChronoDuration::zero());
            let notification = build_notification(&title, &body, tz, &dues, now, grace);
            batches.push((notification, group.iter().map(|r| r.id.clone()).collect()));
        }
        Ok(batches)
    }
}

struct Plan {
    schedule: Schedule,
    rows: Vec<(Timestamp, DeliveryStatus)>,
    next: Option<Timestamp>,
}

fn fmt_local(tz: Tz, t: Timestamp) -> String {
    t.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string()
}

fn list_times(tz: Tz, dues: &[Timestamp]) -> String {
    let mut out = String::new();
    for d in dues.iter().take(MAX_LISTED) {
        out.push_str("\n- ");
        out.push_str(&fmt_local(tz, *d));
    }
    if dues.len() > MAX_LISTED {
        out.push_str(&format!("\n…另有 {} 次", dues.len() - MAX_LISTED));
    }
    out
}

fn build_notification(
    title: &str,
    body: &str,
    tz: Tz,
    dues: &[Timestamp],
    now: Timestamp,
    grace: ChronoDuration,
) -> Notification {
    let text = if body.is_empty() { title } else { body };
    if let [due] = dues {
        let late = now - *due > grace;
        let body = if late {
            format!("{text}\n（原定 {}，逾時補送）", fmt_local(tz, *due))
        } else {
            text.to_owned()
        };
        return Notification {
            title: title.to_owned(),
            body,
            task_id: None,
        };
    }
    Notification {
        title: format!("{title}（錯過 {} 次）", dues.len()),
        body: format!(
            "{text}\n以下時間點在應用未執行期間到期，已合併為一則補送：{}",
            list_times(tz, dues)
        ),
        task_id: None,
    }
}

fn attention_notice(schedule: &Schedule, dues: &[Timestamp]) -> Notification {
    let tz = parse_tz(&schedule.timezone).unwrap_or(Tz::UTC);
    Notification {
        title: format!("{}：需要您確認", schedule.title),
        body: format!(
            "此排程的動作（{}）逾時未執行，系統不會補執行，請自行決定是否處理：{}",
            schedule.action,
            list_times(tz, dues)
        ),
        task_id: None,
    }
}
