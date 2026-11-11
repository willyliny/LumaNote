//! Scheduler tests. Everything runs on the fake clock; nothing really sleeps.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{NaiveDate, NaiveTime, Weekday};
use chrono_tz::Tz;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::ports::fakes::{FakeClock, FakeNotifier};
use crate::ports::{Clock, Notification, Notifier};
use crate::schema::{ts_to_sql, LumaError, Result, Timestamp};
use crate::storage::schedules::NewDeliveryRow;
use crate::storage::Store;

fn t(s: &str) -> Timestamp {
    chrono::DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone(&chrono::Utc)
}

fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(h, min, 0)
        .unwrap()
}

struct Env {
    clock: FakeClock,
    notifier: Arc<FakeNotifier>,
    sched: Scheduler,
    path: PathBuf,
    _dir: tempfile::TempDir,
}

fn config() -> SchedulerConfig {
    SchedulerConfig::default()
}

impl Env {
    fn at(start: &str) -> Self {
        Self::with_config(start, config())
    }

    fn with_config(start: &str, cfg: SchedulerConfig) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lumanote.db");
        let clock = FakeClock::new(t(start));
        let notifier = Arc::new(FakeNotifier::new());
        let sched = Self::build(&path, &clock, notifier.clone(), cfg);
        Self {
            clock,
            notifier,
            sched,
            path,
            _dir: dir,
        }
    }

    fn build(
        path: &std::path::Path,
        clock: &FakeClock,
        notifier: Arc<dyn Notifier>,
        cfg: SchedulerConfig,
    ) -> Scheduler {
        let store = Store::open(path, clock).unwrap();
        Scheduler::new(share_store(store), Arc::new(clock.clone()), notifier, cfg)
    }

    /// Drops the engine and opens a fresh one over the same database file (a process restart).
    fn restart(&mut self) {
        let dummy = Scheduler::new(
            share_store(Store::open_in_memory(&self.clock).unwrap()),
            Arc::new(self.clock.clone()),
            self.notifier.clone(),
            config(),
        );
        drop(std::mem::replace(&mut self.sched, dummy));
        self.sched = Self::build(&self.path, &self.clock, self.notifier.clone(), config());
    }

    fn advance(&self, secs: u64) {
        self.clock.advance(Duration::from_secs(secs));
    }

    fn sent(&self) -> Vec<Notification> {
        self.notifier.sent()
    }

    fn deliveries(&self, id: &ScheduleId) -> Vec<Delivery> {
        self.sched
            .list_deliveries(&DeliveryFilter {
                schedule_id: Some(id.clone()),
                ..Default::default()
            })
            .unwrap()
    }
}

async fn settle() {
    for _ in 0..200 {
        tokio::task::yield_now().await;
    }
}

fn one_shot(title: &str, tz: &str, at: Timestamp) -> NewSchedule {
    NewSchedule::reminder(title, "", tz, StartTime::Instant { at })
}

fn daily(title: &str, tz: &str, first: chrono::NaiveDateTime) -> NewSchedule {
    let mut n = NewSchedule::reminder(title, "", tz, StartTime::Local { at: first });
    n.repeat = Some(RepeatSpec::daily());
    n
}

// ---------------------------------------------------------------- one-shot / recurring

#[tokio::test]
async fn one_shot_fires_exactly_once() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(one_shot("喝水", "Asia/Taipei", t("2026-11-12T01:00:00Z")))
        .unwrap();
    env.sched.tick().await.unwrap();
    assert!(env.sent().is_empty());

    env.advance(3600);
    let r = env.sched.tick().await.unwrap();
    assert_eq!(r.due, 1);
    assert_eq!(env.sent().len(), 1);
    assert_eq!(env.sent()[0].title, "喝水");

    env.sched.tick().await.unwrap();
    env.advance(86_400);
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 1, "a one-shot never fires again");

    let after = env.sched.get_schedule(&s.id).unwrap();
    assert_eq!(after.next_run_at, None);
    let d = env.deliveries(&s.id);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].status, DeliveryStatus::Delivered);
    assert_eq!(d[0].due_at, t("2026-11-12T01:00:00Z"));
    assert!(d[0].delivered_at.is_some());
}

#[tokio::test]
async fn daily_recurrence_advances_at_local_time() {
    let env = Env::at("2026-11-12T00:00:00Z"); // 08:00 in Taipei
    let mut n = daily("日報", "Asia/Taipei", local(2026, 11, 12, 9, 0));
    n.original_rule = Some("每天早上九點".into());
    let s = env.sched.create_schedule(n).unwrap();
    assert_eq!(s.next_run_at, Some(t("2026-11-12T01:00:00Z")));
    assert_eq!(s.original_rule, "每天早上九點");
    assert_eq!(s.timezone, "Asia/Taipei");

    for day in 0..3 {
        env.advance(3600 + if day > 0 { 23 * 3600 } else { 0 });
        env.sched.tick().await.unwrap();
        assert_eq!(env.sent().len(), day + 1);
    }
    let after = env.sched.get_schedule(&s.id).unwrap();
    assert_eq!(after.next_run_at, Some(t("2026-11-15T01:00:00Z")));
    assert!(env
        .deliveries(&s.id)
        .iter()
        .all(|d| d.status == DeliveryStatus::Delivered));
}

#[test]
fn weekly_monthly_and_interval_rules() {
    let tz: Tz = "Asia/Taipei".parse().unwrap();
    // Mon+Wed at 09:00, anchored on Monday 2026-11-09.
    let mut spec = RepeatSpec::weekly(vec![Weekday::Wed, Weekday::Mon]);
    let rec = Recurrence::new(&spec, local(2026, 11, 9, 9, 0)).unwrap();
    let mut at = t("2026-11-09T00:00:00Z");
    let mut seen = Vec::new();
    for _ in 0..4 {
        at = rec.next_after(tz, at).unwrap();
        seen.push(at);
    }
    assert_eq!(
        seen,
        vec![
            t("2026-11-09T01:00:00Z"),
            t("2026-11-11T01:00:00Z"),
            t("2026-11-16T01:00:00Z"),
            t("2026-11-18T01:00:00Z")
        ]
    );
    // every second week skips the in-between week
    spec.interval = 2;
    let rec2 = Recurrence::new(&spec, local(2026, 11, 9, 9, 0)).unwrap();
    let second = rec2.next_after(tz, t("2026-11-11T01:00:00Z")).unwrap();
    assert_eq!(second, t("2026-11-23T01:00:00Z"));

    // monthly on the 31st clamps to the end of shorter months
    let m = Recurrence::new(&RepeatSpec::monthly(31), local(2027, 1, 31, 9, 0)).unwrap();
    let feb = m.next_after(tz, t("2027-01-31T01:00:00Z")).unwrap();
    assert_eq!(feb, t("2027-02-28T01:00:00Z"));
    let mar = m.next_after(tz, feb).unwrap();
    assert_eq!(mar, t("2027-03-31T01:00:00Z"));

    // RRULE text round-trips
    for r in [&rec, &rec2, &m] {
        assert_eq!(&Recurrence::parse(&r.to_rrule()).unwrap(), r);
    }
    assert_eq!(
        rec.to_rrule(),
        "DTSTART=2026-11-09;FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=0;BYSECOND=0"
    );
    assert!(Recurrence::parse("FREQ=YEARLY;DTSTART=2026-01-01").is_err());
    assert!(Recurrence::parse("garbage").is_err());
}

#[test]
fn invalid_repeat_specs_are_rejected() {
    let start = local(2026, 11, 9, 9, 0);
    let mut s = RepeatSpec::daily();
    s.interval = 0;
    assert!(Recurrence::new(&s, start).is_err());
    assert!(Recurrence::new(&RepeatSpec::monthly(32), start).is_err());
    let mut d = RepeatSpec::daily();
    d.weekdays = vec![Weekday::Mon];
    assert!(Recurrence::new(&d, start).is_err());
    let mut w = RepeatSpec::weekly(vec![]);
    w.month_day = Some(3);
    assert!(Recurrence::new(&w, start).is_err());
}

// ---------------------------------------------------------------- restart / crash

#[tokio::test]
async fn restart_midway_neither_duplicates_nor_loses() {
    let mut env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(daily("日報", "Asia/Taipei", local(2026, 11, 12, 9, 0)))
        .unwrap();
    env.advance(3600);
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 1);

    env.restart();
    env.sched.tick().await.unwrap();
    assert_eq!(
        env.sent().len(),
        1,
        "restart must not re-notify a delivered occurrence"
    );

    // the schedule survived and still fires the next day
    env.advance(86_400);
    env.restart();
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 2);
    assert_eq!(env.deliveries(&s.id).len(), 2);
}

#[tokio::test]
async fn crash_between_row_and_notify_is_at_least_once() {
    let mut env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(one_shot("藥", "Asia/Taipei", t("2026-11-12T01:00:00Z")))
        .unwrap();
    env.advance(3600);
    // Phase 1 only (row written, schedule advanced), then the process "dies".
    env.sched.commit_due(env.clock.now_utc()).unwrap();
    assert!(env.sent().is_empty());
    assert_eq!(env.deliveries(&s.id)[0].status, DeliveryStatus::Pending);

    env.restart();
    env.sched.tick().await.unwrap();
    assert_eq!(
        env.sent().len(),
        1,
        "the pending row is recovered, not lost"
    );
    assert_eq!(env.deliveries(&s.id)[0].status, DeliveryStatus::Delivered);

    env.restart();
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 1, "and not repeated once delivered");
}

#[tokio::test]
async fn crash_after_notify_before_mark_may_repeat_once() {
    // Documents the at-least-once window: toast shown, then the process dies before the
    // row is marked delivered. After restart the reminder is shown again, once.
    let mut env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(one_shot("藥", "Asia/Taipei", t("2026-11-12T01:00:00Z")))
        .unwrap();
    env.advance(3600);
    env.sched.commit_due(env.clock.now_utc()).unwrap();
    let batches = env.sched.pending_batches().unwrap();
    assert_eq!(batches.len(), 1);
    env.notifier.notify(&batches[0].0).await.unwrap(); // shown, but never marked

    env.restart();
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 2);
    assert_eq!(env.deliveries(&s.id)[0].status, DeliveryStatus::Delivered);
    env.restart();
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 2);
}

struct ToggleNotifier {
    fail: AtomicBool,
    sent: Mutex<Vec<Notification>>,
}

#[async_trait]
impl Notifier for ToggleNotifier {
    async fn notify(&self, n: &Notification) -> Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(LumaError::ExternalFailure {
                message: "toast unavailable".into(),
                retryable: true,
            });
        }
        self.sent.lock().unwrap().push(n.clone());
        Ok(())
    }
}

#[tokio::test]
async fn notifier_failure_keeps_row_pending_and_retries() {
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new(t("2026-11-12T00:00:00Z"));
    let notifier = Arc::new(ToggleNotifier {
        fail: AtomicBool::new(true),
        sent: Mutex::new(Vec::new()),
    });
    let store = Store::open(&dir.path().join("a.db"), &clock).unwrap();
    let sched = Scheduler::new(
        share_store(store),
        Arc::new(clock.clone()),
        notifier.clone(),
        config(),
    );
    let s = sched
        .create_schedule(one_shot("x", "UTC", t("2026-11-12T00:10:00Z")))
        .unwrap();
    clock.advance(Duration::from_secs(600));
    sched.tick().await.unwrap();
    let d = sched
        .list_deliveries(&DeliveryFilter {
            schedule_id: Some(s.id.clone()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(d[0].status, DeliveryStatus::Pending);
    assert!(notifier.sent.lock().unwrap().is_empty());

    notifier.fail.store(false, Ordering::SeqCst);
    sched.tick().await.unwrap();
    assert_eq!(notifier.sent.lock().unwrap().len(), 1);
    sched.tick().await.unwrap();
    assert_eq!(notifier.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn existing_occurrence_key_is_never_notified_again() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let due = t("2026-11-12T01:00:00Z");
    let s = env
        .sched
        .create_schedule(one_shot("x", "UTC", due))
        .unwrap();
    // Simulate "already handled": a row for this occurrence exists, yet next_run_at still points at it.
    {
        let mut store = env.sched.db();
        let key = ts_to_sql(&due);
        store
            .commit_occurrences(
                s.id.as_str(),
                &[NewDeliveryRow {
                    id: "pre".into(),
                    occurrence_key: key,
                    status: "delivered".into(),
                }],
                Some(due),
            )
            .unwrap();
    }
    env.advance(3600);
    let r = env.sched.tick().await.unwrap();
    assert_eq!((r.due, r.notifications_sent), (0, 0));
    assert!(env.sent().is_empty());
    assert_eq!(env.sched.get_schedule(&s.id).unwrap().next_run_at, None);
    assert_eq!(env.deliveries(&s.id).len(), 1);
}

// ---------------------------------------------------------------- misfire

fn gap_env() -> (Env, Schedule) {
    let env = Env::at("2026-11-12T00:00:00Z");
    let mut n = daily("日報", "Asia/Taipei", local(2026, 11, 12, 9, 0));
    n.useful_window_secs = Some(7 * 86_400);
    let s = env.sched.create_schedule(n).unwrap();
    (env, s)
}

#[tokio::test]
async fn sleep_resume_gap_coalesces_into_one_notification() {
    let (env, s) = gap_env();
    // laptop asleep until 2026-11-15 18:00 Taipei: due at 11-12, 13, 14, 15 (01:00Z)
    env.advance(3 * 86_400 + 10 * 3600);
    let r = env.sched.tick().await.unwrap();
    assert_eq!(r.due, 4);
    let sent = env.sent();
    assert_eq!(sent.len(), 1, "one catch-up notification");
    assert!(sent[0].title.contains("錯過 4 次"), "{}", sent[0].title);
    for day in [
        "2026-11-12 09:00",
        "2026-11-13 09:00",
        "2026-11-14 09:00",
        "2026-11-15 09:00",
    ] {
        assert!(
            sent[0].body.contains(day),
            "{day} missing from {}",
            sent[0].body
        );
    }
    let d = env.deliveries(&s.id);
    assert_eq!(d.len(), 4);
    assert!(d.iter().all(|x| x.status == DeliveryStatus::Delivered));
    assert_eq!(
        env.sched.get_schedule(&s.id).unwrap().next_run_at,
        Some(t("2026-11-16T01:00:00Z"))
    );
}

#[tokio::test]
async fn occurrences_past_their_window_are_marked_missed_not_delivered() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let mut n = daily("日報", "Asia/Taipei", local(2026, 11, 12, 9, 0));
    n.useful_window_secs = Some(10 * 3600);
    let s = env.sched.create_schedule(n).unwrap();
    env.advance(3 * 86_400 + 10 * 3600); // now 11-15T10:00Z; last occurrence is 9h late
    env.sched.tick().await.unwrap();

    let sent = env.sent();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].body.contains("2026-11-15 09:00") && sent[0].body.contains("逾時補送"));
    let d = env.deliveries(&s.id);
    let missed = d
        .iter()
        .filter(|x| x.status == DeliveryStatus::Missed)
        .count();
    let delivered = d
        .iter()
        .filter(|x| x.status == DeliveryStatus::Delivered)
        .count();
    assert_eq!((missed, delivered), (3, 1));
    assert!(d
        .iter()
        .filter(|x| x.status == DeliveryStatus::Missed)
        .all(|x| x.delivered_at.is_none()));
}

#[tokio::test]
async fn everything_expired_delivers_nothing() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let mut n = one_shot("會議", "Asia/Taipei", t("2026-11-12T01:00:00Z"));
    n.useful_window_secs = Some(900);
    let s = env.sched.create_schedule(n).unwrap();
    env.advance(3600 + 901);
    env.sched.tick().await.unwrap();
    assert!(env.sent().is_empty());
    assert_eq!(env.deliveries(&s.id)[0].status, DeliveryStatus::Missed);
}

#[tokio::test]
async fn skip_policy_marks_all_late_occurrences_missed() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let mut n = daily("x", "UTC", local(2026, 11, 12, 9, 0));
    n.misfire_policy = Some(MisfirePolicy::Skip);
    let s = env.sched.create_schedule(n).unwrap();
    env.advance(2 * 86_400 + 11 * 3600); // 2026-11-14T11:00Z, 2h after the third occurrence
    env.sched.tick().await.unwrap();
    assert!(env.sent().is_empty());
    assert!(env
        .deliveries(&s.id)
        .iter()
        .all(|d| d.status == DeliveryStatus::Missed));
    assert_eq!(env.deliveries(&s.id).len(), 3);
}

#[tokio::test]
async fn on_time_occurrence_is_not_treated_as_late() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let mut n = one_shot("x", "UTC", t("2026-11-12T00:10:00Z"));
    n.misfire_policy = Some(MisfirePolicy::Skip);
    env.sched.create_schedule(n).unwrap();
    env.advance(600 + 90); // 90 s late is within the 120 s grace
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 1);
    assert!(!env.sent()[0].body.contains("逾時補送"));
}

#[tokio::test]
async fn external_write_is_never_executed_late() {
    let env = Env::at("2026-11-12T00:00:00Z");
    for action in [ActionKind::ExternalWrite, ActionKind::Payment] {
        let mut n = one_shot("寄出報告", "Asia/Taipei", t("2026-11-12T01:00:00Z"));
        n.action = action;
        let s = env.sched.create_schedule(n).unwrap();
        assert_eq!(s.misfire_policy, MisfirePolicy::NeedsAttention);
    }
    env.advance(3 * 3600); // 2h late; a coalescing reminder would still have been delivered
    let r = env.sched.tick().await.unwrap();
    assert_eq!((r.needs_attention, r.due), (2, 0));
    let all = env
        .sched
        .list_deliveries(&DeliveryFilter {
            status: Some(DeliveryStatus::NeedsAttention),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(all.len(), 2);
    assert!(env
        .sched
        .list_deliveries(&DeliveryFilter {
            status: Some(DeliveryStatus::Delivered),
            ..Default::default()
        })
        .unwrap()
        .is_empty());
    // only informational notices, which say it was not executed
    assert!(env
        .sent()
        .iter()
        .all(|n| n.title.contains("需要您確認") && n.body.contains("不會補執行")));
    // the user can acknowledge it
    let done = env.sched.mark_done(&all[0].id).unwrap();
    assert_eq!(done.status, DeliveryStatus::Done);
}

#[tokio::test]
async fn external_write_on_time_only_notifies() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let mut n = one_shot("轉帳", "UTC", t("2026-11-12T00:05:00Z"));
    n.action = ActionKind::Payment;
    let s = env.sched.create_schedule(n).unwrap();
    env.advance(300);
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 1);
    assert_eq!(env.deliveries(&s.id)[0].status, DeliveryStatus::Delivered);
}

#[test]
fn external_write_cannot_choose_a_late_executing_policy() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let mut n = one_shot("x", "UTC", t("2026-11-12T01:00:00Z"));
    n.action = ActionKind::ExternalWrite;
    n.misfire_policy = Some(MisfirePolicy::Coalesce);
    assert!(matches!(
        env.sched.create_schedule(n),
        Err(LumaError::InvalidInput { .. })
    ));
}

#[test]
fn misfire_policy_strings_match_the_column() {
    for p in MisfirePolicy::ALL {
        assert_eq!(MisfirePolicy::parse(p.as_str()).unwrap(), *p);
    }
    assert_eq!(MisfirePolicy::Coalesce.as_str(), "coalesce"); // the column default in 0001
}

// ---------------------------------------------------------------- DST

#[test]
fn spring_forward_keeps_local_time_and_shifts_nonexistent_times() {
    let ny: Tz = "America/New_York".parse().unwrap();
    // 09:00 local before (EST, UTC-5) and after (EDT, UTC-4) the 2026-03-08 change
    let rec = Recurrence::new(&RepeatSpec::daily(), local(2026, 3, 7, 9, 0)).unwrap();
    let a = rec.next_after(ny, t("2026-03-07T00:00:00Z")).unwrap();
    let b = rec.next_after(ny, a).unwrap();
    let c = rec.next_after(ny, b).unwrap();
    assert_eq!(
        (a, b, c),
        (
            t("2026-03-07T14:00:00Z"),
            t("2026-03-08T13:00:00Z"),
            t("2026-03-09T13:00:00Z")
        )
    );

    // 02:30 does not exist on 2026-03-08 -> fires at 03:30 EDT (07:30Z), once
    let rec = Recurrence::new(&RepeatSpec::daily(), local(2026, 3, 7, 2, 30)).unwrap();
    let a = rec.next_after(ny, t("2026-03-07T00:00:00Z")).unwrap();
    let b = rec.next_after(ny, a).unwrap();
    let c = rec.next_after(ny, b).unwrap();
    assert_eq!(a, t("2026-03-07T07:30:00Z")); // 02:30 EST
    assert_eq!(b, t("2026-03-08T07:30:00Z")); // 03:30 EDT
    assert_eq!(c, t("2026-03-09T06:30:00Z")); // 02:30 EDT
    let r = recurrence::resolve_local(ny, local(2026, 3, 8, 2, 30)).unwrap();
    assert_eq!(
        (r.kind, r.instant),
        (LocalTimeKind::Nonexistent, t("2026-03-08T07:30:00Z"))
    );
}

#[test]
fn fall_back_fires_the_ambiguous_time_once_at_its_first_occurrence() {
    let ny: Tz = "America/New_York".parse().unwrap();
    // 01:30 on 2026-11-01 happens twice (EDT 05:30Z, then EST 06:30Z)
    let rec = Recurrence::new(&RepeatSpec::daily(), local(2026, 10, 31, 1, 30)).unwrap();
    let mut seen = Vec::new();
    let mut at = t("2026-10-31T00:00:00Z");
    for _ in 0..3 {
        at = rec.next_after(ny, at).unwrap();
        seen.push(at);
    }
    assert_eq!(
        seen,
        vec![
            t("2026-10-31T05:30:00Z"),
            t("2026-11-01T05:30:00Z"),
            t("2026-11-02T06:30:00Z")
        ]
    );
    let r = recurrence::resolve_local(ny, local(2026, 11, 1, 1, 30)).unwrap();
    assert_eq!(
        (r.kind, r.instant),
        (LocalTimeKind::Ambiguous, t("2026-11-01T05:30:00Z"))
    );

    // 09:00 stays 09:00 local: 13:00Z in EDT, 14:00Z in EST
    let rec = Recurrence::new(&RepeatSpec::daily(), local(2026, 10, 31, 9, 0)).unwrap();
    let a = rec.next_after(ny, t("2026-11-01T00:00:00Z")).unwrap();
    let b = rec.next_after(ny, a).unwrap();
    assert_eq!(
        (a, b),
        (t("2026-11-01T14:00:00Z"), t("2026-11-02T14:00:00Z"))
    );
}

#[tokio::test]
async fn engine_fires_through_spring_forward_exactly_once_per_day() {
    let env = Env::at("2026-03-07T00:00:00Z");
    let s = env
        .sched
        .create_schedule(daily("x", "America/New_York", local(2026, 3, 7, 2, 30)))
        .unwrap();
    for _ in 0..16 {
        env.advance(6 * 3600);
        env.sched.tick().await.unwrap();
    }
    let dues: Vec<_> = {
        let mut v: Vec<_> = env.deliveries(&s.id).iter().map(|d| d.due_at).collect();
        v.sort();
        v
    };
    assert_eq!(
        dues,
        vec![
            t("2026-03-07T07:30:00Z"),
            t("2026-03-08T07:30:00Z"),
            t("2026-03-09T06:30:00Z"),
            t("2026-03-10T06:30:00Z")
        ]
    );
    assert_eq!(env.sent().len(), 4);
}

// ---------------------------------------------------------------- clock jumps

#[tokio::test]
async fn wall_clock_jump_forward_is_detected_and_catches_up() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(one_shot("x", "UTC", t("2026-11-12T10:00:00Z")))
        .unwrap();
    let token = CancellationToken::new();
    let h = tokio::spawn({
        let sc = env.sched.clone();
        let tk = token.clone();
        async move { sc.run(tk).await }
    });
    settle().await;
    // system time corrected forward by 3 h without any monotonic time passing
    env.clock.set_utc(t("2026-11-12T13:00:00Z"));
    settle().await;
    assert!(
        env.sent().is_empty(),
        "the sleep itself is monotonic and has not elapsed"
    );
    env.advance(30); // next wake (max_sleep): the loop re-reads the wall clock
    settle().await;
    assert_eq!(env.sent().len(), 1);
    assert!(env.sent()[0].body.contains("逾時補送"));
    assert!(env.sched.clock_jumps_detected() >= 1);
    assert_eq!(env.deliveries(&s.id).len(), 1);
    token.cancel();
    h.await.unwrap().unwrap();
}

#[tokio::test]
async fn wall_clock_jump_backward_does_not_repeat_or_lose() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let daily_s = env
        .sched
        .create_schedule(daily("日報", "UTC", local(2026, 11, 12, 9, 0)))
        .unwrap();
    let once = env
        .sched
        .create_schedule(one_shot("一次", "UTC", t("2026-11-12T20:00:00Z")))
        .unwrap();
    env.advance(9 * 3600);
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 1);

    // clock corrected 2 h backwards: 09:00Z occurrence is "in the future" again
    env.clock.set_utc(t("2026-11-12T07:00:00Z"));
    env.sched.tick().await.unwrap();
    env.advance(3 * 3600); // wall passes 09:00Z a second time
    env.sched.tick().await.unwrap();
    assert_eq!(
        env.sent().len(),
        1,
        "the 09:00Z occurrence already has its delivery"
    );
    assert_eq!(env.deliveries(&daily_s.id).len(), 1);

    // later occurrences still work: one-shot at 20:00Z, daily next day
    env.advance(10 * 3600);
    env.sched.tick().await.unwrap();
    assert_eq!(env.deliveries(&once.id).len(), 1);
    env.advance(24 * 3600);
    env.sched.tick().await.unwrap();
    assert_eq!(env.deliveries(&daily_s.id).len(), 2);
}

#[tokio::test]
async fn backward_jump_before_creation_time_does_not_fire_early() {
    let env = Env::at("2026-11-12T12:00:00Z");
    env.sched
        .create_schedule(one_shot("x", "UTC", t("2026-11-12T13:00:00Z")))
        .unwrap();
    env.clock.set_utc(t("2026-11-12T05:00:00Z"));
    env.sched.tick().await.unwrap();
    assert!(env.sent().is_empty());
    env.clock.set_utc(t("2026-11-12T13:00:00Z"));
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 1);
}

#[tokio::test]
async fn schedule_timezone_is_independent_of_the_machine_and_editable() {
    // The scheduler never reads the machine timezone; only the schedule's own IANA zone counts.
    let env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(daily("x", "Asia/Taipei", local(2026, 11, 13, 9, 0)))
        .unwrap();
    assert_eq!(s.next_run_at, Some(t("2026-11-13T01:00:00Z")));
    // the user moves the schedule to New York: 09:00 local there (EST in November)
    let u = env
        .sched
        .update_schedule(
            &s.id,
            ScheduleUpdate {
                timezone: Some("America/New_York".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(u.next_run_at, Some(t("2026-11-13T14:00:00Z")));
    assert_eq!(u.timezone, "America/New_York");
    // a one-shot cannot silently reinterpret its wall-clock time
    let o = env
        .sched
        .create_schedule(one_shot("o", "UTC", t("2026-11-14T00:00:00Z")))
        .unwrap();
    let err = env
        .sched
        .update_schedule(
            &o.id,
            ScheduleUpdate {
                timezone: Some("Asia/Tokyo".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(err, LumaError::InvalidInput { .. }));
    let ok = env
        .sched
        .update_schedule(
            &o.id,
            ScheduleUpdate {
                timezone: Some("Asia/Tokyo".into()),
                start: Some(StartTime::Local {
                    at: local(2026, 11, 14, 9, 0),
                }),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(ok.next_run_at, Some(t("2026-11-14T00:00:00Z")));
}

// ---------------------------------------------------------------- loop behaviour

#[tokio::test]
async fn loop_sleeps_on_the_clock_and_wakes_early_for_new_schedules() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let token = CancellationToken::new();
    let h = tokio::spawn({
        let sc = env.sched.clone();
        let tk = token.clone();
        async move { sc.run(tk).await }
    });
    settle().await; // engine is now asleep for max_sleep (30 s) with nothing scheduled
    env.sched
        .create_schedule(one_shot("快", "UTC", t("2026-11-12T00:00:10Z")))
        .unwrap();
    settle().await; // woken, recomputed: sleeping 10 s
    env.advance(9);
    settle().await;
    assert!(env.sent().is_empty());
    env.advance(1);
    settle().await;
    assert_eq!(
        env.sent().len(),
        1,
        "fired at 10 s although the original sleep was 30 s"
    );
    token.cancel();
    h.await.unwrap().unwrap();
}

#[tokio::test]
async fn disabling_while_sleep_is_pending_prevents_the_delivery() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(one_shot("x", "UTC", t("2026-11-12T01:00:00Z")))
        .unwrap();
    let token = CancellationToken::new();
    let h = tokio::spawn({
        let sc = env.sched.clone();
        let tk = token.clone();
        async move { sc.run(tk).await }
    });
    settle().await;
    env.sched.set_enabled(&s.id, false).unwrap();
    for _ in 0..130 {
        env.advance(30);
        settle().await;
    }
    assert!(env.sent().is_empty());
    assert!(env.deliveries(&s.id).is_empty());
    token.cancel();
    h.await.unwrap().unwrap();
}

#[tokio::test]
async fn deleting_while_sleep_is_pending_prevents_the_delivery() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(daily("x", "UTC", local(2026, 11, 12, 1, 0)))
        .unwrap();
    let token = CancellationToken::new();
    let h = tokio::spawn({
        let sc = env.sched.clone();
        let tk = token.clone();
        async move { sc.run(tk).await }
    });
    settle().await;
    env.sched.delete_schedule(&s.id).unwrap();
    for _ in 0..130 {
        env.advance(30);
        settle().await;
    }
    assert!(env.sent().is_empty());
    assert!(env.sched.list_schedules().unwrap().is_empty());
    assert!(env.sched.get_schedule(&s.id).is_err());
    assert!(env.sched.delete_schedule(&s.id).is_err());
    token.cancel();
    h.await.unwrap().unwrap();
}

#[tokio::test]
async fn reenabling_does_not_back_fill_the_disabled_period() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(daily("x", "UTC", local(2026, 11, 12, 9, 0)))
        .unwrap();
    env.sched.set_enabled(&s.id, false).unwrap();
    env.advance(3 * 86_400 + 3600);
    env.sched.tick().await.unwrap();
    assert!(env.sent().is_empty());
    let r = env.sched.set_enabled(&s.id, true).unwrap();
    // now is 2026-11-15T01:00Z: the 11-12..11-14 occurrences are not back-filled
    assert_eq!(r.next_run_at, Some(t("2026-11-15T09:00:00Z")));
    env.sched.tick().await.unwrap();
    assert!(env.sent().is_empty());

    // a one-shot whose time passed while disabled cannot be re-enabled as is
    let o = env
        .sched
        .create_schedule(one_shot("o", "UTC", t("2026-11-15T02:00:00Z")))
        .unwrap();
    env.sched.set_enabled(&o.id, false).unwrap();
    env.advance(7200);
    assert!(env.sched.set_enabled(&o.id, true).is_err());
}

#[tokio::test]
async fn many_schedules_due_at_the_same_instant_each_fire_once() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let due = t("2026-11-12T08:00:00Z");
    let mut ids = Vec::new();
    for i in 0..60 {
        ids.push(
            env.sched
                .create_schedule(one_shot(&format!("n{i:02}"), "UTC", due))
                .unwrap()
                .id,
        );
    }
    env.advance(8 * 3600);
    let r = env.sched.tick().await.unwrap();
    assert_eq!((r.due, r.notifications_sent), (60, 60));
    let mut titles: Vec<_> = env.sent().into_iter().map(|n| n.title).collect();
    titles.sort();
    titles.dedup();
    assert_eq!(titles.len(), 60);
    for id in &ids {
        assert_eq!(env.deliveries(id).len(), 1);
    }
    env.sched.tick().await.unwrap();
    env.advance(86_400);
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 60);
}

#[tokio::test]
async fn corrupt_schedule_is_quarantined_and_does_not_block_others() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let good = env
        .sched
        .create_schedule(one_shot("ok", "UTC", t("2026-11-12T00:05:00Z")))
        .unwrap();
    {
        let mut store = env.sched.db();
        store
            .insert_schedule_row(&crate::storage::schedules::ScheduleRow {
                id: "bad".into(),
                task_template: "{not json".into(),
                timezone: "Mars/Olympus".into(),
                next_run_at: Some(t("2026-11-12T00:01:00Z")),
                recurrence: None,
                misfire_policy: "coalesce".into(),
                enabled: true,
                deleted_at: None,
            })
            .unwrap();
    }
    env.advance(600);
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 1);
    assert_eq!(env.deliveries(&good.id).len(), 1);
    assert!(
        !env.sched
            .db()
            .get_schedule_row("bad")
            .unwrap()
            .unwrap()
            .enabled
    );
}

// ---------------------------------------------------------------- snooze / done

#[tokio::test]
async fn snooze_creates_a_one_shot_follow_up() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let mut n = one_shot("吃藥", "Asia/Taipei", t("2026-11-12T01:00:00Z"));
    n.body = "飯後吃藥".into();
    let s = env.sched.create_schedule(n).unwrap();
    env.advance(3600);
    env.sched.tick().await.unwrap();
    let d = env.deliveries(&s.id).remove(0);

    let follow = env
        .sched
        .snooze(&d.id, SnoozeUntil::After { secs: 600 })
        .unwrap();
    assert_eq!(follow.next_run_at, Some(t("2026-11-12T01:10:00Z")));
    assert!(follow.recurrence.is_none());
    assert_eq!(
        (follow.title.as_str(), follow.body.as_str()),
        ("吃藥", "飯後吃藥")
    );
    assert_eq!(follow.snoozed_from.as_ref(), Some(&d.id));
    assert_eq!(env.deliveries(&s.id)[0].status, DeliveryStatus::Snoozed);

    // snoozing twice is an invalid transition, and nothing extra was created
    let err = env
        .sched
        .snooze(&d.id, SnoozeUntil::After { secs: 60 })
        .unwrap_err();
    assert!(matches!(err, LumaError::InvalidTransition { .. }));
    assert_eq!(env.sched.list_schedules().unwrap().len(), 2);

    env.advance(599);
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 1);
    env.advance(1);
    env.sched.tick().await.unwrap();
    assert_eq!(env.sent().len(), 2);
    assert_eq!(env.sent()[1].body, "飯後吃藥");

    // the follow-up's own delivery can be snoozed again, or finished
    let d2 = env.deliveries(&follow.id).remove(0);
    let again = env
        .sched
        .snooze(
            &d2.id,
            SnoozeUntil::At {
                at: t("2026-11-12T03:00:00Z"),
            },
        )
        .unwrap();
    assert_eq!(again.next_run_at, Some(t("2026-11-12T03:00:00Z")));
    assert!(env
        .sched
        .snooze(&d2.id, SnoozeUntil::After { secs: 0 })
        .is_err());
}

#[tokio::test]
async fn mark_done_and_cancelled_follow_the_allowed_transitions() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(daily("x", "UTC", local(2026, 11, 12, 1, 0)))
        .unwrap();
    env.advance(3600);
    env.sched.tick().await.unwrap();
    env.advance(86_400);
    env.sched.tick().await.unwrap();
    let ds = env.deliveries(&s.id); // newest first
    assert_eq!(ds.len(), 2);
    assert_eq!(
        env.sched.mark_done(&ds[0].id).unwrap().status,
        DeliveryStatus::Done
    );
    assert_eq!(
        env.sched.mark_cancelled(&ds[1].id).unwrap().status,
        DeliveryStatus::Cancelled
    );
    assert!(matches!(
        env.sched.mark_done(&ds[0].id),
        Err(LumaError::InvalidTransition { .. })
    ));
    // a done reminder cannot be snoozed
    assert!(env
        .sched
        .snooze(&ds[0].id, SnoozeUntil::After { secs: 60 })
        .is_err());
    assert!(env
        .sched
        .mark_done(&DeliveryId::from_string("nope"))
        .is_err());
    // the recurring schedule itself keeps running
    assert!(env.sched.get_schedule(&s.id).unwrap().next_run_at.is_some());
}

#[tokio::test]
async fn pending_and_missed_deliveries_cannot_be_snoozed_or_completed() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let mut n = one_shot("x", "UTC", t("2026-11-12T00:10:00Z"));
    n.useful_window_secs = Some(60);
    let s = env.sched.create_schedule(n).unwrap();
    env.advance(7200);
    env.sched.tick().await.unwrap();
    let d = env.deliveries(&s.id).remove(0);
    assert_eq!(d.status, DeliveryStatus::Missed);
    assert!(env
        .sched
        .snooze(&d.id, SnoozeUntil::After { secs: 60 })
        .is_err());
    assert!(env.sched.mark_done(&d.id).is_err());
}

// ---------------------------------------------------------------- vague times / validation / update

#[test]
fn vague_times_use_defaults_and_never_guess() {
    let none = Env::at("2026-11-12T00:00:00Z"); // 08:00 in Taipei
    let tomorrow_morning = VagueTime {
        day: DayRef::Tomorrow,
        part: DayPart::Morning,
    };
    assert_eq!(
        none.sched
            .resolve_vague_time(tomorrow_morning, "Asia/Taipei")
            .unwrap(),
        TimeResolution::NeedsClarification {
            reason: ClarificationReason::NoDefaultForDayPart {
                part: DayPart::Morning
            }
        }
    );

    let cfg = SchedulerConfig {
        time_defaults: TimeDefaults {
            morning: NaiveTime::from_hms_opt(9, 0, 0),
            ..Default::default()
        },
        ..config()
    };
    let env = Env::with_config("2026-11-12T00:00:00Z", cfg);
    match env
        .sched
        .resolve_vague_time(tomorrow_morning, "Asia/Taipei")
        .unwrap()
    {
        TimeResolution::Resolved { local: l, at } => {
            assert_eq!(l, local(2026, 11, 13, 9, 0));
            assert_eq!(at, t("2026-11-13T01:00:00Z"));
            // and the result feeds straight into a schedule
            let s = env
                .sched
                .create_schedule(NewSchedule::reminder(
                    "x",
                    "",
                    "Asia/Taipei",
                    StartTime::Local { at: l },
                ))
                .unwrap();
            assert_eq!(s.next_run_at, Some(at));
        }
        other => panic!("{other:?}"),
    }
    // other parts of the day still have no default
    let evening = VagueTime {
        day: DayRef::Today,
        part: DayPart::Evening,
    };
    assert!(matches!(
        env.sched
            .resolve_vague_time(evening, "Asia/Taipei")
            .unwrap(),
        TimeResolution::NeedsClarification { .. }
    ));
    // "this morning" at 08:00 is still ahead of a 09:00 default, but not once it has passed
    let today = VagueTime {
        day: DayRef::Today,
        part: DayPart::Morning,
    };
    assert!(matches!(
        env.sched.resolve_vague_time(today, "Asia/Taipei").unwrap(),
        TimeResolution::Resolved { .. }
    ));
    env.advance(2 * 3600);
    assert_eq!(
        env.sched.resolve_vague_time(today, "Asia/Taipei").unwrap(),
        TimeResolution::NeedsClarification {
            reason: ClarificationReason::TimeAlreadyPassed
        }
    );
    assert!(env.sched.resolve_vague_time(today, "Not/AZone").is_err());
}

#[test]
fn create_validates_input() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let bad = |n: NewSchedule| {
        matches!(
            env.sched.create_schedule(n),
            Err(LumaError::InvalidInput { .. })
        )
    };
    assert!(bad(one_shot("x", "Mars/Base", t("2026-11-13T00:00:00Z"))));
    assert!(bad(one_shot("  ", "UTC", t("2026-11-13T00:00:00Z"))));
    assert!(bad(one_shot("x", "UTC", t("2026-11-11T00:00:00Z")))); // in the past
    let mut r = daily("x", "UTC", local(2026, 11, 13, 9, 0));
    r.repeat = Some(RepeatSpec {
        interval: 0,
        ..RepeatSpec::daily()
    });
    assert!(bad(r));
    assert!(env.sched.list_schedules().unwrap().is_empty());
}

#[tokio::test]
async fn update_recomputes_and_preserves_original_rule() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let mut n = daily("x", "Asia/Taipei", local(2026, 11, 12, 9, 0));
    n.original_rule = Some("每天早上九點".into());
    let s = env.sched.create_schedule(n).unwrap();
    let u = env
        .sched
        .update_schedule(
            &s.id,
            ScheduleUpdate {
                title: Some("新標題".into()),
                start: Some(StartTime::Local {
                    at: local(2026, 11, 12, 20, 0),
                }),
                repeat: Some(Some(RepeatSpec::weekly(vec![Weekday::Fri]))),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(u.title, "新標題");
    assert_eq!(u.original_rule, "每天早上九點");
    // 2026-11-12 is a Thursday; first Friday 20:00 Taipei is 11-13T12:00Z
    assert_eq!(u.next_run_at, Some(t("2026-11-13T12:00:00Z")));
    assert_eq!(env.sched.get_schedule(&s.id).unwrap(), u);
    let one = env
        .sched
        .update_schedule(
            &s.id,
            ScheduleUpdate {
                repeat: Some(None),
                start: Some(StartTime::Instant {
                    at: t("2026-11-20T00:00:00Z"),
                }),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(one.recurrence.is_none());
    assert_eq!(env.sched.list_schedules().unwrap().len(), 1);
    assert!(env
        .sched
        .update_schedule(&ScheduleId::new(), ScheduleUpdate::default())
        .is_err());
}

#[tokio::test]
async fn schedule_round_trips_through_the_database_columns() {
    let mut env = Env::at("2026-11-12T00:00:00Z");
    let mut n = daily("x", "Asia/Taipei", local(2026, 11, 12, 9, 0));
    n.payload = serde_json::json!({"task": "report"});
    n.original_rule = Some("每天 09:00".into());
    let s = env.sched.create_schedule(n).unwrap();
    env.restart();
    assert_eq!(env.sched.get_schedule(&s.id).unwrap(), s);
    let row = env
        .sched
        .db()
        .get_schedule_row(s.id.as_str())
        .unwrap()
        .unwrap();
    assert_eq!(row.timezone, "Asia/Taipei");
    assert_eq!(row.misfire_policy, "coalesce");
    assert!(row.recurrence.unwrap().contains("FREQ=DAILY"));
    assert_eq!(row.next_run_at, Some(t("2026-11-12T01:00:00Z")));
}

// ---------------------------------------------------------------- soft delete (migration 0003)

#[tokio::test]
async fn deleting_a_schedule_keeps_its_delivery_history() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(daily("日報", "UTC", local(2026, 11, 12, 1, 0)))
        .unwrap();
    env.advance(3600);
    env.sched.tick().await.unwrap();
    env.advance(86_400);
    env.sched.tick().await.unwrap();
    let before = env.deliveries(&s.id);
    assert_eq!(before.len(), 2);
    assert!(before.iter().all(|d| d.status == DeliveryStatus::Delivered));

    env.sched.delete_schedule(&s.id).unwrap();

    // history is untouched and still queryable, also unfiltered and after a restart
    assert_eq!(env.deliveries(&s.id), before);
    let all = env
        .sched
        .list_deliveries(&DeliveryFilter::default())
        .unwrap();
    assert_eq!(all, before);
    // the schedule row survives with its deletion time, so history can show its title
    let kept = env.sched.get_schedule_including_deleted(&s.id).unwrap();
    assert_eq!(kept.title, "日報");
    assert_eq!(kept.deleted_at, Some(env.clock.now_utc()));
    assert!(!kept.enabled);
    assert_eq!(kept.next_run_at, None);
}

#[tokio::test]
async fn deleted_schedules_are_hidden_by_default_and_listed_on_request() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let keep = env
        .sched
        .create_schedule(one_shot("keep", "UTC", t("2026-11-12T05:00:00Z")))
        .unwrap();
    let gone = env
        .sched
        .create_schedule(one_shot("gone", "UTC", t("2026-11-12T06:00:00Z")))
        .unwrap();
    env.sched.delete_schedule(&gone.id).unwrap();

    let ids = |v: Vec<Schedule>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
    assert_eq!(
        ids(env.sched.list_schedules().unwrap()),
        vec![keep.id.clone()]
    );
    assert_eq!(
        ids(env.sched.list_schedules_with(false).unwrap()),
        vec![keep.id.clone()]
    );
    assert_eq!(
        ids(env.sched.list_schedules_with(true).unwrap()),
        vec![keep.id.clone(), gone.id.clone()]
    );
    // a deleted schedule is unknown to every mutating or loading call
    assert!(env.sched.get_schedule(&gone.id).is_err());
    assert!(env.sched.delete_schedule(&gone.id).is_err());
    assert!(env.sched.set_enabled(&gone.id, true).is_err());
    assert!(env
        .sched
        .update_schedule(
            &gone.id,
            ScheduleUpdate {
                title: Some("x".into()),
                ..Default::default()
            }
        )
        .is_err());
    // and the engine no longer plans around it
    assert_eq!(
        env.sched.db().earliest_next_run().unwrap(),
        Some(t("2026-11-12T05:00:00Z"))
    );
}

#[tokio::test]
async fn deleted_schedule_never_fires_even_after_restart_or_a_long_sleep() {
    let mut env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(daily("x", "UTC", local(2026, 11, 12, 1, 0)))
        .unwrap();
    env.sched.delete_schedule(&s.id).unwrap();
    env.advance(3 * 86_400);
    let r = env.sched.tick().await.unwrap();
    assert_eq!((r.due, r.missed, r.needs_attention), (0, 0, 0));
    env.restart();
    env.sched.tick().await.unwrap();
    env.advance(86_400);
    env.sched.tick().await.unwrap();
    assert!(env.sent().is_empty());
    assert!(env.deliveries(&s.id).is_empty());
}

#[tokio::test]
async fn schedule_deleted_while_the_engine_sleeps_does_not_fire_or_record() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(one_shot("x", "UTC", t("2026-11-12T01:00:00Z")))
        .unwrap();
    let token = CancellationToken::new();
    let h = tokio::spawn({
        let sc = env.sched.clone();
        let tk = token.clone();
        async move { sc.run(tk).await }
    });
    settle().await; // the loop is now asleep until 01:00
    env.advance(1800);
    env.sched.delete_schedule(&s.id).unwrap();
    for _ in 0..130 {
        env.advance(30);
        settle().await;
    }
    assert!(env.sent().is_empty());
    assert!(
        env.deliveries(&s.id).is_empty(),
        "nothing is recorded for a deleted schedule"
    );
    token.cancel();
    h.await.unwrap().unwrap();
}

#[tokio::test]
async fn deleting_cancels_undelivered_notifications_but_keeps_the_rows() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(one_shot("x", "UTC", t("2026-11-12T01:00:00Z")))
        .unwrap();
    env.advance(3600);
    // phase 1 only: the row is written but nothing was notified yet (crash window)
    let (report, _) = env.sched.commit_due(env.clock.now_utc()).unwrap();
    assert_eq!(report.due, 1);
    assert_eq!(env.deliveries(&s.id)[0].status, DeliveryStatus::Pending);

    env.sched.delete_schedule(&s.id).unwrap();
    env.sched.tick().await.unwrap();
    assert!(env.sent().is_empty(), "a deleted reminder is not notified");
    let d = env.deliveries(&s.id);
    assert_eq!(d.len(), 1, "the trigger record is kept");
    assert_eq!(d[0].status, DeliveryStatus::Cancelled);
}

#[tokio::test]
async fn occurrence_uniqueness_still_applies_to_a_deleted_schedule() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(one_shot("x", "UTC", t("2026-11-12T01:00:00Z")))
        .unwrap();
    env.advance(3600);
    env.sched.tick().await.unwrap();
    env.sched.delete_schedule(&s.id).unwrap();
    // the database refuses a duplicate (schedule_id, occurrence_key) outright
    let dup = env.sched.db().conn.execute(
        "INSERT INTO deliveries (id, schedule_id, occurrence_key, status) VALUES ('dup', ?1, ?2, 'pending')",
        rusqlite::params![s.id.as_str(), ts_to_sql(&t("2026-11-12T01:00:00Z"))],
    );
    assert!(
        dup.is_err(),
        "UNIQUE (schedule_id, occurrence_key) must hold"
    );
    // and the engine-level insert records nothing for a deleted schedule
    let inserted = env
        .sched
        .db()
        .commit_occurrences(
            s.id.as_str(),
            &[NewDeliveryRow {
                id: "n1".into(),
                occurrence_key: ts_to_sql(&t("2026-11-13T01:00:00Z")),
                status: "pending".into(),
            }],
            None,
        )
        .unwrap();
    assert!(inserted.is_empty());
    assert_eq!(env.deliveries(&s.id).len(), 1);
}

#[tokio::test]
async fn a_delivery_of_a_deleted_schedule_cannot_be_snoozed() {
    let env = Env::at("2026-11-12T00:00:00Z");
    let s = env
        .sched
        .create_schedule(one_shot("x", "UTC", t("2026-11-12T01:00:00Z")))
        .unwrap();
    env.advance(3600);
    env.sched.tick().await.unwrap();
    let d = env.deliveries(&s.id)[0].clone();
    env.sched.delete_schedule(&s.id).unwrap();
    assert!(env
        .sched
        .snooze(&d.id, SnoozeUntil::After { secs: 600 })
        .is_err());
    assert_eq!(env.deliveries(&s.id)[0].status, DeliveryStatus::Delivered);
    // history can still be closed out by the user
    assert_eq!(
        env.sched.mark_done(&d.id).unwrap().status,
        DeliveryStatus::Done
    );
}
