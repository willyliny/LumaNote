use std::collections::HashSet;

use rusqlite::params;
use serde_json::json;

use super::*;
use crate::model::{MessagePart, Role};
use crate::ports::fakes::FakeClock;
use crate::ports::Clock;
use crate::schema::{EvidenceRef, StepId, TaskId, ToolCallId, Trust};
use crate::task::{SideEffect, StepStatus, TaskStatus};

fn latest_version() -> u32 {
    MIGRATIONS.last().map(|m| m.version).unwrap_or(0)
}

fn mem() -> (Store, FakeClock) {
    let clock = FakeClock::at_default_start();
    (Store::open_in_memory(&clock).unwrap(), clock)
}

fn table_names(s: &Store) -> HashSet<String> {
    let mut stmt = s
        .conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn new_artifact(s: &mut Store, t: &TaskId, c: &FakeClock) -> EvidenceRef {
    let a = s
        .add_artifact(t, "markdown", "/out/a.md", "abc", 3, c.now_utc())
        .unwrap();
    EvidenceRef::Artifact { id: a.id }
}

/// Drives a fresh task into `target` using only legal transitions.
fn task_in(s: &mut Store, c: &FakeClock, target: TaskStatus) -> Task {
    use TaskStatus::*;
    let t = s.create_task("do it", &json!({}), c.now_utc()).unwrap();
    let go = |s: &mut Store, to: TaskStatus, ev: &[EvidenceRef]| {
        s.transition_task(&t.id, to, ev, None, c).unwrap();
    };
    match target {
        Queued => {}
        Running => go(s, Running, &[]),
        AwaitingUser | Paused | Verifying | Uncertain | Failed => {
            go(s, Running, &[]);
            go(s, target, &[]);
        }
        Cancelled => go(s, Cancelled, &[]),
        Completed => {
            go(s, Running, &[]);
            go(s, Verifying, &[]);
            let ev = new_artifact(s, &t.id, c);
            go(s, Completed, &[ev]);
        }
    }
    s.get_task(&t.id).unwrap()
}

#[test]
fn migrations_create_every_handbook_table() {
    let (s, _) = mem();
    assert_eq!(s.schema_version().unwrap(), latest_version());
    assert!(s.foreign_keys_enabled().unwrap());
    let names = table_names(&s);
    for t in [
        "tasks",
        "task_steps",
        "activity_segments",
        "summaries",
        "memories",
        "suggestions",
        "schedules",
        "deliveries",
        "grants",
        "connections",
        "model_profiles",
        "model_routes",
        "model_runs",
        "conversations",
        "messages",
        "task_events",
        "artifacts",
        "consents",
        "exclusions",
        "settings",
        "schema_version",
    ] {
        assert!(names.contains(t), "missing table {t}");
    }
}

#[test]
fn file_database_uses_wal_and_migrating_twice_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lumanote.db");
    let clock = FakeClock::at_default_start();
    let task_id;
    {
        let mut s = Store::open(&path, &clock).unwrap();
        assert_eq!(s.journal_mode().unwrap(), "wal");
        task_id = s
            .create_task("keep me", &json!({}), clock.now_utc())
            .unwrap()
            .id;
    }
    let mut conn = rusqlite::Connection::open(&path).unwrap();
    let applied = migrate::apply(&mut conn, MIGRATIONS, "2026-01-01T00:00:00Z").unwrap();
    assert!(applied.is_empty());
    drop(conn);
    let s = Store::open(&path, &clock).unwrap();
    assert_eq!(s.schema_version().unwrap(), latest_version());
    let versions: i64 = s
        .conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(versions, MIGRATIONS.len() as i64);
    assert_eq!(s.get_task(&task_id).unwrap().intent, "keep me");
}

#[test]
fn failed_migration_rolls_back_and_leaves_database_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lumanote.db");
    let clock = FakeClock::at_default_start();
    drop(Store::open(&path, &clock).unwrap());

    let mut bad = MIGRATIONS.to_vec();
    bad.push(Migration {
        version: latest_version() + 1,
        name: "bad",
        sql: "CREATE TABLE half_done (x INTEGER); INSERT INTO tasks (id) VALUES ('x'); CREATE TABLE never (y);",
    });
    let err = Store::open_with(&path, &clock, &bad).unwrap_err();
    assert!(matches!(err, LumaError::Storage { .. }));
    assert!(err
        .to_string()
        .contains(&format!("migration {}", latest_version() + 1)));

    let s = Store::open(&path, &clock).unwrap();
    assert_eq!(s.schema_version().unwrap(), latest_version());
    assert!(!table_names(&s).contains("half_done"));
    assert!(!table_names(&s).contains("never"));

    // A failing first migration on a fresh database leaves nothing behind.
    let fresh = dir.path().join("fresh.db");
    let only_bad = [Migration {
        version: 1,
        name: "bad",
        sql: "CREATE TABLE a (x); SELECT * FROM missing_table;",
    }];
    assert!(Store::open_with(&fresh, &clock, &only_bad).is_err());
    let conn = rusqlite::Connection::open(&fresh).unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn newer_database_and_unordered_migrations_are_refused() {
    let clock = FakeClock::at_default_start();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lumanote.db");
    let mut two = MIGRATIONS.to_vec();
    two.push(Migration {
        version: latest_version() + 1,
        name: "more",
        sql: "CREATE TABLE extra (x);",
    });
    drop(Store::open_with(&path, &clock, &two).unwrap());
    assert!(Store::open(&path, &clock).is_err()); // build only knows the shipped versions
    let unordered = [two[1], two[0]];
    assert!(Store::open_with(&dir.path().join("o.db"), &clock, &unordered).is_err());
}

#[test]
fn delivery_uniqueness_per_schedule_and_occurrence() {
    let (s, _) = mem();
    for id in ["s1", "s2"] {
        s.conn
            .execute(
                "INSERT INTO schedules (id, task_template, timezone) VALUES (?1, '{}', 'Asia/Taipei')",
                [id],
            )
            .unwrap();
    }
    let ins = |id: &str, sched: &str, key: &str| {
        s.conn.execute(
            "INSERT INTO deliveries (id, schedule_id, occurrence_key, status) VALUES (?1, ?2, ?3, 'delivered')",
            params![id, sched, key],
        )
    };
    ins("d1", "s1", "2026-11-13T09:00").unwrap();
    assert!(ins("d2", "s1", "2026-11-13T09:00").is_err());
    ins("d3", "s1", "2026-11-14T09:00").unwrap();
    ins("d4", "s2", "2026-11-13T09:00").unwrap();
}

#[test]
fn foreign_keys_are_enforced() {
    let (mut s, c) = mem();
    let bogus = TaskId::from_string("nope");
    assert!(s
        .add_artifact(&bogus, "k", "p", "h", 1, c.now_utc())
        .is_err());
    assert!(s.conn.execute("INSERT INTO task_events (task_id, sequence, type, created_at) VALUES ('nope', 1, 't', 'x')", []).is_err());
    assert!(matches!(
        s.get_task(&bogus),
        Err(LumaError::InvalidInput { .. })
    ));
}

#[test]
fn every_transition_pair_against_the_database() {
    let (mut s, c) = mem();
    let mut legal = 0;
    let mut illegal = 0;
    for &from in TaskStatus::ALL {
        for &to in TaskStatus::ALL {
            let t = task_in(&mut s, &c, from);
            let before = s.list_task_events(&t.id, 0).unwrap().len();
            let evidence = if to == TaskStatus::Completed && from.can_transition_to(to) {
                vec![new_artifact(&mut s, &t.id, &c)]
            } else {
                vec![]
            };
            let r = s.transition_task(&t.id, to, &evidence, Some("test"), &c);
            let after = s.get_task(&t.id).unwrap();
            let events = s.list_task_events(&t.id, 0).unwrap();
            assert_eq!(events.len(), before + 1, "{from}->{to}: exactly one event");
            let last = events.last().unwrap();
            if from.can_transition_to(to) {
                legal += 1;
                let ev = r.unwrap();
                assert_eq!(after.status, to);
                assert_eq!(ev.event_type, "status_changed");
                assert_eq!(ev.payload["from"], from.as_str());
                assert_eq!(ev.payload["to"], to.as_str());
                assert_eq!(last.sequence, ev.sequence);
            } else {
                illegal += 1;
                assert!(
                    matches!(r, Err(LumaError::InvalidTransition { .. })),
                    "{from}->{to}"
                );
                assert_eq!(
                    after.status, from,
                    "state must not change on illegal {from}->{to}"
                );
                assert_eq!(last.event_type, "transition_rejected");
            }
        }
    }
    assert_eq!((legal, illegal), (20, 61));
}

#[test]
fn event_sequence_is_increasing_per_task() {
    let (mut s, c) = mem();
    let a = s.create_task("a", &json!({}), c.now_utc()).unwrap();
    let b = s.create_task("b", &json!({}), c.now_utc()).unwrap();
    s.transition_task(&a.id, TaskStatus::Running, &[], None, &c)
        .unwrap();
    s.transition_task(&b.id, TaskStatus::Cancelled, &[], None, &c)
        .unwrap();
    s.transition_task(&a.id, TaskStatus::Paused, &[], None, &c)
        .unwrap();
    let seqs: Vec<i64> = s
        .list_task_events(&a.id, 0)
        .unwrap()
        .iter()
        .map(|e| e.sequence)
        .collect();
    assert_eq!(seqs, vec![1, 2, 3]);
    let seqs: Vec<i64> = s
        .list_task_events(&b.id, 0)
        .unwrap()
        .iter()
        .map(|e| e.sequence)
        .collect();
    assert_eq!(seqs, vec![1, 2]);
    assert_eq!(s.list_task_events(&a.id, 2).unwrap().len(), 1);
    assert_eq!(s.list_tasks(Some(TaskStatus::Paused)).unwrap().len(), 1);
    assert_eq!(s.list_tasks(None).unwrap().len(), 2);
}

#[test]
fn completed_requires_real_evidence() {
    let (mut s, c) = mem();
    for from in [TaskStatus::Verifying, TaskStatus::Uncertain] {
        let t = task_in(&mut s, &c, from);
        // no evidence
        let e = s
            .transition_task(&t.id, TaskStatus::Completed, &[], None, &c)
            .unwrap_err();
        assert!(
            matches!(
                e,
                LumaError::InvalidTransition {
                    reason: Some(_),
                    ..
                }
            ),
            "{e}"
        );
        assert_eq!(s.get_task(&t.id).unwrap().status, from);
        // nonexistent evidence
        let bogus = EvidenceRef::Artifact {
            id: crate::schema::ArtifactId::new(),
        };
        assert!(s
            .transition_task(&t.id, TaskStatus::Completed, &[bogus], None, &c)
            .is_err());
        // evidence belonging to another task
        let other = s.create_task("other", &json!({}), c.now_utc()).unwrap();
        let foreign = new_artifact(&mut s, &other.id, &c);
        assert!(s
            .transition_task(&t.id, TaskStatus::Completed, &[foreign], None, &c)
            .is_err());
        assert_eq!(s.get_task(&t.id).unwrap().status, from);
        // real evidence
        let ev = new_artifact(&mut s, &t.id, &c);
        s.transition_task(
            &t.id,
            TaskStatus::Completed,
            std::slice::from_ref(&ev),
            None,
            &c,
        )
        .unwrap();
        let done = s.get_task(&t.id).unwrap();
        assert_eq!(done.status, TaskStatus::Completed);
        assert_eq!(
            done.result_ref.as_deref(),
            Some(ev.to_ref_string().as_str())
        );
    }
}

#[test]
fn step_evidence_must_be_a_succeeded_step() {
    let (mut s, c) = mem();
    let t = task_in(&mut s, &c, TaskStatus::Running);
    let step = s
        .begin_step(
            &t.id,
            "read_file",
            None,
            SideEffect::None,
            None,
            c.now_utc(),
        )
        .unwrap();
    s.transition_task(&t.id, TaskStatus::Verifying, &[], None, &c)
        .unwrap();
    let ev = EvidenceRef::Step {
        id: step.id.clone(),
    };
    assert!(s
        .transition_task(
            &t.id,
            TaskStatus::Completed,
            std::slice::from_ref(&ev),
            None,
            &c
        )
        .is_err());
    s.finish_step(&step.id, StepOutcome::Succeeded, None, c.now_utc())
        .unwrap();
    s.transition_task(&t.id, TaskStatus::Completed, &[ev], None, &c)
        .unwrap();
}

#[test]
fn steps_follow_the_write_ahead_rule() {
    let (mut s, c) = mem();
    let queued = task_in(&mut s, &c, TaskStatus::Queued);
    assert!(s
        .begin_step(&queued.id, "t", None, SideEffect::None, None, c.now_utc())
        .is_err());

    let t = task_in(&mut s, &c, TaskStatus::Running);
    let st = s
        .begin_step(
            &t.id,
            "send_mail",
            Some("{\"to\":\"x\"}"),
            SideEffect::ExternalWrite,
            Some("key-1"),
            c.now_utc(),
        )
        .unwrap();
    assert_eq!(
        (st.seq, st.status, st.finished_at),
        (1, StepStatus::Started, None)
    );
    // visible as started before any execution result exists
    assert_eq!(s.list_steps(&t.id).unwrap()[0].status, StepStatus::Started);
    assert!(s
        .begin_step(
            &t.id,
            "send_mail",
            None,
            SideEffect::ExternalWrite,
            Some("key-1"),
            c.now_utc()
        )
        .is_err());
    assert_eq!(
        s.find_step_by_idempotency_key(&t.id, "key-1")
            .unwrap()
            .unwrap()
            .id,
        st.id
    );
    let st2 = s
        .begin_step(&t.id, "t2", None, SideEffect::None, None, c.now_utc())
        .unwrap();
    assert_eq!(st2.seq, 2);

    let done = s
        .finish_step(&st.id, StepOutcome::Succeeded, None, c.now_utc())
        .unwrap();
    assert_eq!(done.status, StepStatus::Succeeded);
    assert!(done.finished_at.is_some());
    assert!(matches!(
        s.finish_step(&st.id, StepOutcome::Failed, None, c.now_utc()),
        Err(LumaError::InvalidTransition { .. })
    ));
    assert!(s
        .finish_step(&StepId::new(), StepOutcome::Failed, None, c.now_utc())
        .is_err());
}

#[test]
fn recovery_pauses_running_tasks_without_inflight_external_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lumanote.db");
    let clock = FakeClock::at_default_start();
    let (id, local, done_ext);
    {
        let mut s = Store::open(&path, &clock).unwrap();
        let t = task_in(&mut s, &clock, TaskStatus::Running);
        id = t.id.clone();
        done_ext = s
            .begin_step(
                &id,
                "send",
                None,
                SideEffect::ExternalWrite,
                None,
                clock.now_utc(),
            )
            .unwrap()
            .id;
        s.finish_step(&done_ext, StepOutcome::Succeeded, None, clock.now_utc())
            .unwrap();
        local = s
            .begin_step(
                &id,
                "write",
                None,
                SideEffect::LocalWrite,
                None,
                clock.now_utc(),
            )
            .unwrap()
            .id;
        // process "crashes" here
    }
    let mut s = Store::open(&path, &clock).unwrap();
    let rec = s.recover_after_restart(&clock).unwrap();
    assert_eq!(rec.len(), 1);
    assert_eq!(
        (rec[0].from, rec[0].to),
        (TaskStatus::Running, TaskStatus::Paused)
    );
    assert!(rec[0].uncertain_steps.is_empty());
    assert_eq!(s.get_task(&id).unwrap().status, TaskStatus::Paused);
    assert_eq!(s.get_step(&local).unwrap().status, StepStatus::Interrupted);
    assert_eq!(s.get_step(&done_ext).unwrap().status, StepStatus::Succeeded);
    let last = s.list_task_events(&id, 0).unwrap().pop().unwrap();
    assert_eq!(last.payload["reason"], "restart_recovery");
    // second recovery run is a no-op
    assert!(s.recover_after_restart(&clock).unwrap().is_empty());
    // paused tasks can resume
    s.transition_task(&id, TaskStatus::Running, &[], None, &clock)
        .unwrap();
}

#[test]
fn recovery_marks_inflight_external_write_uncertain_and_never_reruns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lumanote.db");
    let clock = FakeClock::at_default_start();
    let (id, ext, other);
    {
        let mut s = Store::open(&path, &clock).unwrap();
        let t = task_in(&mut s, &clock, TaskStatus::Running);
        id = t.id.clone();
        ext = s
            .begin_step(
                &id,
                "create_event",
                None,
                SideEffect::ExternalWrite,
                Some("k"),
                clock.now_utc(),
            )
            .unwrap()
            .id;
        other = s
            .begin_step(&id, "read", None, SideEffect::None, None, clock.now_utc())
            .unwrap()
            .id;
    }
    let mut s = Store::open(&path, &clock).unwrap();
    // tasks in other states must be left alone
    let waiting = task_in(&mut s, &clock, TaskStatus::AwaitingUser);
    let finished = task_in(&mut s, &clock, TaskStatus::Completed);
    let rec = s.recover_after_restart(&clock).unwrap();
    assert_eq!(rec.len(), 1);
    assert_eq!(rec[0].to, TaskStatus::Uncertain);
    assert_eq!(rec[0].uncertain_steps, vec![ext.clone()]);
    assert_eq!(s.get_task(&id).unwrap().status, TaskStatus::Uncertain);
    assert_eq!(s.get_step(&ext).unwrap().status, StepStatus::Uncertain);
    assert_eq!(s.get_step(&other).unwrap().status, StepStatus::Interrupted);
    assert_eq!(
        s.get_task(&waiting.id).unwrap().status,
        TaskStatus::AwaitingUser
    );
    assert_eq!(
        s.get_task(&finished.id).unwrap().status,
        TaskStatus::Completed
    );
    // the only ways out of uncertain are completed (with evidence), failed, cancelled
    assert!(s
        .transition_task(&id, TaskStatus::Running, &[], None, &clock)
        .is_err());
    s.transition_task(
        &id,
        TaskStatus::Failed,
        &[],
        Some("user confirmed not sent"),
        &clock,
    )
    .unwrap();
}

#[test]
fn untrusted_flag_is_sticky_and_set_by_anything_the_user_did_not_type() {
    let (mut s, c) = mem();
    let t = s.create_task("x", &json!({}), c.now_utc()).unwrap();
    assert!(!s
        .record_tool_result_trust(&t.id, Trust::UserProvided, c.now_utc())
        .unwrap());
    assert!(!s.get_task(&t.id).unwrap().untrusted_content);
    // a local file the user shared is not something the user typed: it sets the flag too
    assert!(s
        .record_tool_result_trust(&t.id, Trust::LocalGranted, c.now_utc())
        .unwrap());
    assert!(s.get_task(&t.id).unwrap().untrusted_content);
    assert!(!s
        .record_tool_result_trust(&t.id, Trust::External, c.now_utc())
        .unwrap());
    assert!(!s.mark_task_untrusted(&t.id, c.now_utc()).unwrap());
    assert!(s.get_task(&t.id).unwrap().untrusted_content);
    s.transition_task(&t.id, TaskStatus::Running, &[], None, &c)
        .unwrap();
    assert!(s.get_task(&t.id).unwrap().untrusted_content);
}

#[test]
fn messages_and_artifacts_round_trip() {
    let (mut s, c) = mem();
    let conv = s.create_conversation("hello", c.now_utc()).unwrap();
    let t = s.create_task("x", &json!({}), c.now_utc()).unwrap();
    let call = ToolCallId::from_string("call-1");
    let parts = vec![
        MessagePart::Text { text: "hi".into() },
        MessagePart::ToolCall {
            id: call.clone(),
            name: "read".into(),
            arguments: json!({"p": 1}),
        },
    ];
    c.advance(std::time::Duration::from_secs(5));
    s.append_message(
        &conv.id,
        Some(&t.id),
        Role::Assistant,
        &parts,
        None,
        c.now_utc(),
    )
    .unwrap();
    s.append_message(
        &conv.id,
        None,
        Role::Tool,
        &[MessagePart::ToolResult {
            call_id: call.clone(),
            content: "ok".into(),
            is_error: false,
        }],
        Some(&call),
        c.now_utc(),
    )
    .unwrap();
    let msgs = s.list_messages(&conv.id).unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].parts, parts);
    assert_eq!(msgs[0].task_id.as_ref(), Some(&t.id));
    assert_eq!(msgs[1].tool_call_id.as_ref(), Some(&call));
    assert!(s.get_conversation(&conv.id).unwrap().updated_at > conv.updated_at);

    let a = s
        .add_artifact(
            &t.id,
            "markdown",
            "/o/a.md",
            &crate::schema::sha256_hex(b"abc"),
            3,
            c.now_utc(),
        )
        .unwrap();
    assert_eq!(s.get_artifact(&a.id).unwrap(), a);
    assert_eq!(s.list_artifacts(&t.id).unwrap(), vec![a]);
}

#[test]
fn grants_round_trip_and_revocation_is_idempotent() {
    let (mut s, c) = mem();
    let g = s
        .insert_grant(
            &NewGrant {
                action: "read".into(),
                resource_scope: "/data".into(),
                arguments_hash: None,
                expires_at: Some(c.now_utc() + chrono::Duration::hours(1)),
                max_uses: Some(3),
                source_task: None,
                scope: GrantScope::UserStanding,
                tool: None,
            },
            c.now_utc(),
        )
        .unwrap();
    assert_eq!((g.used_count, g.max_uses), (0, Some(3)));
    assert!(g.is_active(c.now_utc()));
    let r1 = s.revoke_grant(&g.id, c.now_utc()).unwrap();
    c.advance(std::time::Duration::from_secs(10));
    let r2 = s.revoke_grant(&g.id, c.now_utc()).unwrap();
    assert_eq!(r1.revoked_at, r2.revoked_at);
    assert!(!r2.is_active(c.now_utc()));
    assert_eq!(s.list_grants().unwrap().len(), 1);
    assert!(s
        .revoke_grant(&crate::schema::GrantId::new(), c.now_utc())
        .is_err());
}

#[test]
fn upgrade_from_version_one_keeps_data_and_adds_task_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lumanote.db");
    let clock = FakeClock::at_default_start();
    let id;
    {
        let s = Store::open_with(&path, &clock, &MIGRATIONS[..1]).unwrap();
        assert_eq!(s.schema_version().unwrap(), 1);
        id = TaskId::new();
        s.conn
            .execute(
                "INSERT INTO tasks (id, intent, status, created_at, updated_at) VALUES (?1, 'old', 'paused', 't', 't')",
                [id.as_str()],
            )
            .unwrap();
    }
    // created_at 't' is not a timestamp, so check the new columns with raw SQL.
    let s = Store::open(&path, &clock).unwrap();
    assert_eq!(s.schema_version().unwrap(), latest_version());
    let (limits, count, started, parent, base): (String, i64, Option<String>, Option<String>, i64) = s
        .conn
        .query_row(
            "SELECT limits_json, tool_call_count, started_at, parent_task_id, tool_call_base FROM tasks WHERE id = ?1",
            [id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        (limits.as_str(), count, started, parent, base),
        ("{}", 0, None, None, 0)
    );
    let names = table_names(&s);
    for t in ["todos", "approvals"] {
        assert!(names.contains(t), "missing {t}");
    }
}

#[test]
fn new_task_columns_round_trip_and_started_at_is_set_on_first_run() {
    let (mut s, c) = mem();
    let conv = s.create_conversation("c", c.now_utc()).unwrap();
    let parent = s.create_task("first", &json!({}), c.now_utc()).unwrap();
    let t = s
        .create_task_with(
            &NewTask {
                intent: "follow up".into(),
                scope: json!({"output_dirs": ["/o"]}),
                limits: json!({"max_tool_calls": 5}),
                parent_task_id: Some(parent.id.clone()),
                conversation_id: Some(conv.id.clone()),
            },
            c.now_utc(),
        )
        .unwrap();
    assert_eq!(t.parent_task_id.as_ref(), Some(&parent.id));
    assert_eq!(t.limits, json!({"max_tool_calls": 5}));
    assert_eq!(t.conversation_id.as_ref(), Some(&conv.id));
    assert_eq!((t.tool_call_count, t.started_at), (0, None));
    assert!(s
        .create_task_with(
            &NewTask {
                intent: "x".into(),
                parent_task_id: Some(TaskId::from_string("missing")),
                ..NewTask::default()
            },
            c.now_utc()
        )
        .is_err());

    let t0 = c.now_utc();
    s.transition_task(&t.id, TaskStatus::Running, &[], None, &c)
        .unwrap();
    assert_eq!(s.get_task(&t.id).unwrap().started_at, Some(t0));
    // pause / resume does not move started_at; only an explicit new window does
    c.advance(std::time::Duration::from_secs(30));
    s.transition_task(&t.id, TaskStatus::Paused, &[], None, &c)
        .unwrap();
    s.transition_task(&t.id, TaskStatus::Running, &[], None, &c)
        .unwrap();
    assert_eq!(s.get_task(&t.id).unwrap().started_at, Some(t0));
    assert_eq!(s.increment_tool_calls(&t.id, c.now_utc()).unwrap(), 1);
    assert_eq!(s.increment_tool_calls(&t.id, c.now_utc()).unwrap(), 2);
    s.begin_budget_window(&t.id, c.now_utc()).unwrap();
    let after = s.get_task(&t.id).unwrap();
    assert_eq!(after.started_at, Some(c.now_utc()));
    assert_eq!((after.tool_call_count, after.tool_call_base), (2, 2));
    s.shift_budget_window(&t.id, std::time::Duration::from_secs(60))
        .unwrap();
    assert_eq!(
        s.get_task(&t.id).unwrap().started_at,
        Some(c.now_utc() + chrono::Duration::seconds(60))
    );
}

#[test]
fn recovery_leaves_verifying_and_awaiting_user_tasks_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lumanote.db");
    let clock = FakeClock::at_default_start();
    let (verifying, waiting, running);
    let (v_events, w_events);
    {
        let mut s = Store::open(&path, &clock).unwrap();
        verifying = task_in(&mut s, &clock, TaskStatus::Verifying).id;
        waiting = task_in(&mut s, &clock, TaskStatus::AwaitingUser).id;
        running = task_in(&mut s, &clock, TaskStatus::Running).id;
        v_events = s.list_task_events(&verifying, 0).unwrap();
        w_events = s.list_task_events(&waiting, 0).unwrap();
    }
    let mut s = Store::open(&path, &clock).unwrap();
    let rec = s.recover_after_restart(&clock).unwrap();
    assert_eq!(rec.len(), 1);
    assert_eq!(rec[0].task_id, running);
    assert_eq!(
        s.get_task(&verifying).unwrap().status,
        TaskStatus::Verifying
    );
    assert_eq!(
        s.get_task(&waiting).unwrap().status,
        TaskStatus::AwaitingUser
    );
    // no event, no step change: recovery did not touch them at all
    assert_eq!(s.list_task_events(&verifying, 0).unwrap(), v_events);
    assert_eq!(s.list_task_events(&waiting, 0).unwrap(), w_events);
    // a second pass is still a no-op for them
    assert!(s.recover_after_restart(&clock).unwrap().is_empty());
    assert_eq!(
        s.get_task(&verifying).unwrap().status,
        TaskStatus::Verifying
    );
    // verifying may still be paused or cancelled (amended table)
    s.transition_task(&verifying, TaskStatus::Paused, &[], None, &clock)
        .unwrap();
    s.transition_task(&waiting, TaskStatus::Cancelled, &[], None, &clock)
        .unwrap();
}

#[test]
fn step_results_are_saved_and_failed_steps_can_be_reopened() {
    let (mut s, c) = mem();
    let t = task_in(&mut s, &c, TaskStatus::Running);
    let step = s
        .begin_step(
            &t.id,
            "create_todos",
            None,
            SideEffect::LocalWrite,
            Some("k1"),
            c.now_utc(),
        )
        .unwrap();
    let res = json!({"content": "ok", "is_error": false});
    let done = s
        .finish_step_with_result(
            &step.id,
            StepOutcome::Succeeded,
            None,
            Some(&res),
            c.now_utc(),
        )
        .unwrap();
    assert_eq!(done.result, Some(res));
    assert!(
        s.reopen_step(&step.id, c.now_utc()).is_err(),
        "succeeded steps stay done"
    );

    let f = s
        .begin_step(
            &t.id,
            "x",
            None,
            SideEffect::LocalWrite,
            Some("k2"),
            c.now_utc(),
        )
        .unwrap();
    s.finish_step(&f.id, StepOutcome::Failed, None, c.now_utc())
        .unwrap();
    let again = s.reopen_step(&f.id, c.now_utc()).unwrap();
    assert_eq!(
        (again.status, again.finished_at, again.result),
        (StepStatus::Started, None, None)
    );

    let e = s
        .begin_step(
            &t.id,
            "send",
            None,
            SideEffect::ExternalWrite,
            Some("k3"),
            c.now_utc(),
        )
        .unwrap();
    s.finish_step(&e.id, StepOutcome::Uncertain, None, c.now_utc())
        .unwrap();
    assert!(
        s.reopen_step(&e.id, c.now_utc()).is_err(),
        "external writes are never re-run"
    );
    let ev = s
        .append_task_event(&t.id, "tool_started", &json!({"tool": "x"}), c.now_utc())
        .unwrap();
    assert_eq!(ev.event_type, "tool_started");
    assert_eq!(
        s.list_task_events(&t.id, 0)
            .unwrap()
            .last()
            .unwrap()
            .sequence,
        ev.sequence
    );
}
