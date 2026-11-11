use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::ports::fakes::FakeClock;
use crate::ports::Clock;
use crate::schema::{hash_arguments, Trust};
use crate::storage::{GrantScope, NewGrant};
use crate::task::{SideEffect, StepStatus, TaskStatus};

struct Env {
    store: Store,
    clock: FakeClock,
    dir: tempfile::TempDir,
    root: PathBuf,
    task: TaskId,
}

fn env() -> Env {
    let clock = FakeClock::at_default_start();
    let mut store = Store::open_in_memory(&clock).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::create_dir_all(root.join("granted/out")).unwrap();
    fs::write(root.join("granted/a.txt"), "a").unwrap();
    let scope = json!({"output_dirs": [root.join("granted/out")]});
    let task = store.create_task("t", &scope, clock.now_utc()).unwrap().id;
    Env {
        store,
        clock,
        dir,
        root,
        task,
    }
}

impl Env {
    fn grant(&mut self, action: &str, scope: &str) -> NewGrant {
        NewGrant {
            action: action.into(),
            resource_scope: scope.into(),
            arguments_hash: None,
            expires_at: None,
            max_uses: None,
            source_task: None,
            scope: GrantScope::UserStanding,
            tool: None,
        }
    }
    fn add(&mut self, g: NewGrant) -> crate::storage::Grant {
        self.store.insert_grant(&g, self.clock.now_utc()).unwrap()
    }
    fn granted(&self) -> String {
        self.root.join("granted").display().to_string()
    }
    fn req(&self, action: Action, resource: Resource, hash: &str) -> ActionRequest {
        ActionRequest::new(self.task.clone(), action, resource, hash)
    }
    fn check(&mut self, r: &ActionRequest) -> Decision {
        evaluate(&mut self.store, r, &self.clock).unwrap()
    }
    fn path(&self, rel: &str) -> Resource {
        Resource::Path(self.root.join(rel))
    }
}

#[test]
fn standing_grant_allows_inside_and_consumes_a_use() {
    let mut e = env();
    let g = e.grant("read", &e.granted());
    let g = e.add(g);
    let r = e.req(Action::Read, e.path("granted/a.txt"), "h");
    assert_eq!(
        e.check(&r),
        Decision::Allow {
            grant_id: g.id.clone()
        }
    );
    assert_eq!(e.store.get_grant(&g.id).unwrap().used_count, 1);
    // not yet existing file inside the directory is also covered
    let r = e.req(Action::Read, e.path("granted/new/file.md"), "h");
    assert!(matches!(e.check(&r), Decision::Allow { .. }));
    // other action or other directory needs approval
    let r = e.req(Action::Write, e.path("granted/a.txt"), "h");
    assert_eq!(
        e.check(&r),
        Decision::NeedsApproval {
            reason: ApprovalReason::NoMatchingGrant
        }
    );
    let r = e.req(Action::Read, e.path("elsewhere/a.txt"), "h");
    assert_eq!(
        e.check(&r),
        Decision::NeedsApproval {
            reason: ApprovalReason::NoMatchingGrant
        }
    );
    let r = e.req(
        Action::Read,
        Resource::Path(PathBuf::from(format!("{}-evil/a.txt", e.granted()))),
        "h",
    );
    assert!(matches!(e.check(&r), Decision::NeedsApproval { .. }));
}

#[test]
fn traversal_and_relative_paths_are_denied() {
    let mut e = env();
    let g = e.grant("read", &e.granted());
    e.add(g);
    let r = e.req(Action::Read, e.path("granted/../granted/a.txt"), "h");
    assert!(matches!(e.check(&r), Decision::Deny { .. }));
    let r = e.req(Action::Read, e.path("granted/../../etc/passwd"), "h");
    let d = e.check(&r);
    assert!(matches!(d, Decision::Deny { .. }));
    assert!(matches!(
        d.into_result(&r),
        Err(LumaError::NoPermission { .. })
    ));
    let r = e.req(
        Action::Read,
        Resource::Path(PathBuf::from("granted/a.txt")),
        "h",
    );
    assert!(matches!(e.check(&r), Decision::Deny { .. }));
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_denied_even_with_a_matching_grant() {
    use std::os::unix::fs::symlink;
    let mut e = env();
    fs::create_dir_all(e.root.join("secret")).unwrap();
    fs::write(e.root.join("secret/key.txt"), "k").unwrap();
    symlink(e.root.join("secret"), e.root.join("granted/link")).unwrap();
    let g = e.grant("read", &e.granted());
    let g = e.add(g);
    for rel in ["granted/link/key.txt", "granted/link/new.txt"] {
        let r = e.req(Action::Read, e.path(rel), "h");
        assert!(matches!(e.check(&r), Decision::Deny { .. }), "{rel}");
    }
    assert_eq!(e.store.get_grant(&g.id).unwrap().used_count, 0);
    // A path that is only granted through a symlinked alias is NOT lexically covered by
    // the grant on `granted`: it is not resolved to find out, it simply needs approval.
    symlink(e.root.join("granted"), e.root.join("alias")).unwrap();
    let r = e.req(Action::Read, e.path("alias/a.txt"), "h");
    assert!(matches!(e.check(&r), Decision::NeedsApproval { .. }));
    // a scope given through the alias still works for honest paths
    let g2 = e.grant("read", &e.root.join("alias").display().to_string());
    e.add(g2);
    assert!(matches!(e.check(&r), Decision::Allow { .. }));
}

#[test]
fn grant_expiry_is_exclusive_at_the_boundary() {
    let mut e = env();
    let mut g = e.grant("read", &e.granted());
    g.expires_at = Some(e.clock.now_utc() + chrono::Duration::minutes(10));
    e.add(g);
    let r = e.req(Action::Read, e.path("granted/a.txt"), "h");
    assert!(matches!(e.check(&r), Decision::Allow { .. }));
    e.clock.advance(Duration::from_secs(10 * 60 - 1));
    assert!(matches!(e.check(&r), Decision::Allow { .. }));
    e.clock.advance(Duration::from_secs(1));
    assert_eq!(
        e.check(&r),
        Decision::NeedsApproval {
            reason: ApprovalReason::NoMatchingGrant
        }
    );
}

#[test]
fn max_uses_is_exhausted_exactly() {
    let mut e = env();
    let mut g = e.grant("read", &e.granted());
    g.max_uses = Some(2);
    let g = e.add(g);
    let r = e.req(Action::Read, e.path("granted/a.txt"), "h");
    assert!(matches!(e.check(&r), Decision::Allow { .. }));
    assert!(matches!(e.check(&r), Decision::Allow { .. }));
    assert!(matches!(e.check(&r), Decision::NeedsApproval { .. }));
    assert!(matches!(e.check(&r), Decision::NeedsApproval { .. }));
    assert_eq!(e.store.get_grant(&g.id).unwrap().used_count, 2);
    // zero uses is never usable
    let mut z = e.grant("read", &e.granted());
    z.max_uses = Some(0);
    e.add(z);
    assert!(matches!(e.check(&r), Decision::NeedsApproval { .. }));
}

#[test]
fn revocation_takes_effect_immediately() {
    let mut e = env();
    let g = e.grant("read", &e.granted());
    let g = e.add(g);
    let r = e.req(Action::Read, e.path("granted/a.txt"), "h");
    assert!(matches!(e.check(&r), Decision::Allow { .. }));
    e.store.revoke_grant(&g.id, e.clock.now_utc()).unwrap();
    let d = e.check(&r);
    assert_eq!(
        d,
        Decision::NeedsApproval {
            reason: ApprovalReason::NoMatchingGrant
        }
    );
    assert!(matches!(
        d.into_result(&r),
        Err(LumaError::ApprovalRequired { .. })
    ));
    assert_eq!(e.store.get_grant(&g.id).unwrap().used_count, 1);
}

#[test]
fn arguments_hash_must_match_exactly() {
    let mut e = env();
    let approved = hash_arguments(&json!({"to": "a@x.com", "body": "hi"}));
    let changed = hash_arguments(&json!({"to": "a@x.com", "body": "hi, plus a link"}));
    let task = e.task.clone();
    e.one_shot(&task, "send", "mail:a@x.com", &approved);
    let mail = Resource::Other("mail:a@x.com".into());
    let r = e.req(Action::Send, mail.clone(), &changed);
    // a send is confirmed every time: changed arguments never ride on the approval
    assert_eq!(
        e.check(&r),
        Decision::NeedsApproval {
            reason: ApprovalReason::ConfirmEveryUse
        }
    );
    let r = e.req(Action::Send, mail, &approved);
    assert!(matches!(e.check(&r), Decision::Allow { .. }));
    assert!(matches!(e.check(&r), Decision::NeedsApproval { .. })); // one-shot is spent
}

#[test]
fn non_path_resources_match_exactly_or_by_prefix_wildcard() {
    let mut e = env();
    let g = e.grant("web_fetch", "https://docs.rs/*");
    e.add(g);
    let g = e.grant("calendar_write", "cal-1");
    e.add(g);
    let ok = e.req(
        Action::Other("web_fetch".into()),
        Resource::Other("https://docs.rs/serde".into()),
        "h",
    );
    assert!(matches!(e.check(&ok), Decision::Allow { .. }));
    let no = e.req(
        Action::Other("web_fetch".into()),
        Resource::Other("https://evil.example/".into()),
        "h",
    );
    assert!(matches!(e.check(&no), Decision::NeedsApproval { .. }));
    let ok = e.req(
        Action::Other("calendar_write".into()),
        Resource::Other("cal-1".into()),
        "h",
    );
    assert!(matches!(e.check(&ok), Decision::Allow { .. }));
    let no = e.req(
        Action::Other("calendar_write".into()),
        Resource::Other("cal-10".into()),
        "h",
    );
    assert!(matches!(e.check(&no), Decision::NeedsApproval { .. }));
}

#[test]
fn untrusted_flag_overrides_standing_grants_for_send_delete_and_out_of_scope_write() {
    let mut e = env();
    for (a, s) in [
        ("write", e.granted()),
        ("delete", e.granted()),
        ("read", e.granted()),
        ("send", "mail:*".to_string()),
    ] {
        let g = e.grant(a, &s);
        e.add(g);
    }
    let in_scope_write = e.req(Action::Write, e.path("granted/out/report.md"), "h");
    let out_scope_write = e.req(Action::Write, e.path("granted/a.txt"), "h");
    let delete = e.req(Action::Delete, e.path("granted/a.txt"), "h");
    let send = e.req(Action::Send, Resource::Other("mail:boss@x.com".into()), "h");
    let read = e.req(Action::Read, e.path("granted/a.txt"), "h");

    // before the flag: standing grants apply to reads and writes; a delete or a send never
    // accepts a standing grant (one approval per use, taint or not)
    for r in [&in_scope_write, &out_scope_write, &read] {
        assert!(
            matches!(e.check(r), Decision::Allow { .. }),
            "{:?}",
            r.action
        );
    }
    for r in [&delete, &send] {
        assert_eq!(
            e.check(r),
            Decision::NeedsApproval {
                reason: ApprovalReason::ConfirmEveryUse
            },
            "{:?}",
            r.action
        );
    }
    // external content enters the task
    e.store
        .record_tool_result_trust(&e.task.clone(), Trust::External, e.clock.now_utc())
        .unwrap();
    let ask = Decision::NeedsApproval {
        reason: ApprovalReason::UntrustedContent,
    };
    for r in [&out_scope_write, &delete, &send] {
        assert_eq!(e.check(r), ask, "{:?}", r.action);
    }
    // reads and in-scope writes still follow the normal rules
    assert!(matches!(e.check(&read), Decision::Allow { .. }));
    assert!(matches!(e.check(&in_scope_write), Decision::Allow { .. }));
}

#[test]
fn untrusted_task_accepts_only_its_own_one_shot_approval() {
    let mut e = env();
    let standing = e.grant("send", "mail:*");
    e.add(standing);
    e.store
        .mark_task_untrusted(&e.task.clone(), e.clock.now_utc())
        .unwrap();
    let h = hash_arguments(&json!({"to": "boss@x.com"}));
    let send = e.req(Action::Send, Resource::Other("mail:boss@x.com".into()), &h);
    assert!(matches!(
        e.check(&send),
        Decision::NeedsApproval {
            reason: ApprovalReason::UntrustedContent
        }
    ));

    // approvals that are not one-shot, not bound to the arguments, or from another task do not count
    let other_task = e
        .store
        .create_task("o", &json!({}), e.clock.now_utc())
        .unwrap()
        .id;
    let mut bad = [
        NewGrant {
            arguments_hash: Some(h.clone()),
            max_uses: Some(2),
            source_task: Some(e.task.clone()),
            scope: GrantScope::Task,
            ..e.grant("send", "mail:*")
        },
        NewGrant {
            arguments_hash: None,
            max_uses: Some(1),
            source_task: Some(e.task.clone()),
            scope: GrantScope::Task,
            ..e.grant("send", "mail:*")
        },
        NewGrant {
            arguments_hash: Some(h.clone()),
            max_uses: Some(1),
            source_task: Some(other_task),
            scope: GrantScope::Task,
            ..e.grant("send", "mail:*")
        },
        // looks like a one-shot but is a user-standing grant: never counts as an approval
        NewGrant {
            arguments_hash: Some(h.clone()),
            max_uses: Some(1),
            source_task: Some(e.task.clone()),
            scope: GrantScope::UserStanding,
            ..e.grant("send", "mail:*")
        },
    ]
    .to_vec();
    for g in bad.drain(..) {
        e.add(g);
    }
    assert!(matches!(e.check(&send), Decision::NeedsApproval { .. }));

    let one_shot = NewGrant {
        arguments_hash: Some(h.clone()),
        max_uses: Some(1),
        source_task: Some(e.task.clone()),
        scope: GrantScope::Task,
        tool: Some("send".into()),
        ..e.grant("send", "mail:boss@x.com")
    };
    let g = e.add(one_shot);
    assert_eq!(
        e.check(&send),
        Decision::Allow {
            grant_id: g.id.clone()
        }
    );
    assert!(matches!(
        e.check(&send),
        Decision::NeedsApproval {
            reason: ApprovalReason::UntrustedContent
        }
    ));
}

#[test]
fn only_content_the_user_typed_leaves_the_flag_unset() {
    let mut e = env();
    let g = e.grant("write", &e.granted());
    e.add(g);
    let outside_out = e.req(Action::Write, e.path("granted/a.txt"), "h");
    e.store
        .record_tool_result_trust(&e.task.clone(), Trust::UserProvided, e.clock.now_utc())
        .unwrap();
    assert!(matches!(e.check(&outside_out), Decision::Allow { .. }));
    // a local file the user shared is still not something the user typed (handbook 8.1)
    e.store
        .record_tool_result_trust(&e.task.clone(), Trust::LocalGranted, e.clock.now_utc())
        .unwrap();
    assert_eq!(
        e.check(&outside_out),
        Decision::NeedsApproval {
            reason: ApprovalReason::UntrustedContent
        }
    );
}

#[test]
fn unknown_task_is_invalid_input_and_helpers_work() {
    let mut e = env();
    let mut r = e.req(Action::Read, e.path("granted/a.txt"), "h");
    r.task_id = TaskId::from_string("missing");
    assert!(matches!(
        evaluate(&mut e.store, &r, &e.clock),
        Err(LumaError::InvalidInput { .. })
    ));
    assert_eq!(Action::parse("read"), Action::Read);
    assert_eq!(Action::parse("calendar.create").as_str(), "calendar.create");
    assert_eq!(
        Action::Other("x".into()).class(),
        ActionClass::ExternalWrite
    );
    let _keep = &e.dir;
}

// ---------------------------------------------------------------- helpers for the tests below

impl Env {
    fn new_task(&mut self, out_rel: &str) -> TaskId {
        fs::create_dir_all(self.root.join(out_rel)).unwrap();
        let scope = json!({"output_dirs": [self.root.join(out_rel)]});
        self.store
            .create_task("other", &scope, self.clock.now_utc())
            .unwrap()
            .id
    }
    fn for_task(
        &self,
        task: &TaskId,
        action: Action,
        resource: Resource,
        hash: &str,
    ) -> ActionRequest {
        ActionRequest::new(task.clone(), action, resource, hash)
    }
    /// A task-scoped grant, as the orchestrator creates for a task.
    fn task_grant(&mut self, task: &TaskId, action: &str, scope: &str) -> crate::storage::Grant {
        let g = NewGrant {
            source_task: Some(task.clone()),
            scope: GrantScope::Task,
            tool: Some(action.into()),
            ..self.grant(action, scope)
        };
        self.add(g)
    }
    fn one_shot(
        &mut self,
        task: &TaskId,
        action: &str,
        scope: &str,
        hash: &str,
    ) -> crate::storage::Grant {
        let g = NewGrant {
            arguments_hash: Some(hash.into()),
            max_uses: Some(1),
            source_task: Some(task.clone()),
            scope: GrantScope::Task,
            tool: Some(action.into()),
            ..self.grant(action, scope)
        };
        self.add(g)
    }
    fn run_task(&mut self, task: &TaskId) {
        self.store
            .transition_task(task, TaskStatus::Running, &[], Some("t"), &self.clock)
            .unwrap();
    }
}

const NEEDS: Decision = Decision::NeedsApproval {
    reason: ApprovalReason::NoMatchingGrant,
};

// ---------------------------------------------------------------- 1. grant scope

#[test]
fn task_scoped_grants_authorise_only_their_own_task() {
    let mut e = env();
    let a = e.task.clone();
    let b = e.new_task("b_out");
    // the implied grants of task A: its read dir, its output dir, create_todos
    let read = e.task_grant(&a, "read", &e.granted());
    let write = e.task_grant(
        &a,
        "write",
        &e.root.join("granted/out").display().to_string(),
    );
    let todos = e.task_grant(&a, "create_todos", "local:todos");
    let todo_res = || Resource::Other("local:todos".into());

    // A is served
    for r in [
        e.for_task(&a, Action::Read, e.path("granted/a.txt"), "h"),
        e.for_task(&a, Action::Write, e.path("granted/out/new.md"), "h"),
        e.for_task(&a, Action::Other("create_todos".into()), todo_res(), "h"),
    ] {
        assert!(
            matches!(e.check(&r), Decision::Allow { .. }),
            "{:?}",
            r.action
        );
    }
    let used =
        |e: &Env| [&read, &write, &todos].map(|g| e.store.get_grant(&g.id).unwrap().used_count);
    assert_eq!(used(&e), [1, 1, 1]);

    // B is not: it cannot read A's directory, write into A's output dir or create todos
    for r in [
        e.for_task(&b, Action::Read, e.path("granted/a.txt"), "h"),
        e.for_task(&b, Action::Read, e.path("granted/never/seen.txt"), "h"),
        e.for_task(&b, Action::Write, e.path("granted/out/evil.md"), "h"),
        e.for_task(&b, Action::Other("create_todos".into()), todo_res(), "h"),
    ] {
        assert_eq!(e.check(&r), NEEDS, "{:?} {:?}", r.action, r.resource);
    }
    assert_eq!(
        used(&e),
        [1, 1, 1],
        "B's attempts consumed nothing of A's grants"
    );
    // not even if B's own output dir is a parent of A's (scope is about the grant, not B)
    let r = e.for_task(&b, Action::Write, e.path("b_out/ok.md"), "h");
    assert_eq!(e.check(&r), NEEDS);
}

#[test]
fn a_one_shot_approval_for_one_task_cannot_be_spent_by_another() {
    let mut e = env();
    let (a, b) = (e.task.clone(), e.new_task("b_out"));
    let h = hash_arguments(&json!({"to": "boss@x.com", "body": "hi"}));
    let g = e.one_shot(&a, "send", "mail:boss@x.com", &h);
    let mail = || Resource::Other("mail:boss@x.com".into());

    let rb = e.for_task(&b, Action::Send, mail(), &h);
    assert_eq!(
        e.check(&rb),
        Decision::NeedsApproval {
            reason: ApprovalReason::ConfirmEveryUse
        }
    );
    assert_eq!(
        e.store.get_grant(&g.id).unwrap().used_count,
        0,
        "B did not spend it"
    );
    // B being untrusted does not change that
    e.store.mark_task_untrusted(&b, e.clock.now_utc()).unwrap();
    assert!(matches!(
        e.check(&rb),
        Decision::NeedsApproval {
            reason: ApprovalReason::UntrustedContent
        }
    ));
    assert_eq!(e.store.get_grant(&g.id).unwrap().used_count, 0);

    let ra = e.for_task(&a, Action::Send, mail(), &h);
    assert_eq!(
        e.check(&ra),
        Decision::Allow {
            grant_id: g.id.clone()
        }
    );
    assert_eq!(
        e.check(&ra),
        Decision::NeedsApproval {
            reason: ApprovalReason::ConfirmEveryUse
        },
        "spent after one use"
    );
}

#[test]
fn user_standing_grants_serve_every_task() {
    let mut e = env();
    let (a, b) = (e.task.clone(), e.new_task("b_out"));
    let g = e.grant("read", &e.granted());
    let g = e.add(g); // user-standing, no source task
    let g2 = NewGrant {
        source_task: Some(a.clone()), // provenance only
        ..e.grant("web_fetch", "https://docs.rs/*")
    };
    e.add(g2);
    for t in [&a, &b] {
        let r = e.for_task(t, Action::Read, e.path("granted/a.txt"), "h");
        assert_eq!(
            e.check(&r),
            Decision::Allow {
                grant_id: g.id.clone()
            }
        );
        let r = e.for_task(
            t,
            Action::Other("web_fetch".into()),
            Resource::Other("https://docs.rs/serde".into()),
            "h",
        );
        assert!(matches!(e.check(&r), Decision::Allow { .. }));
    }
    assert_eq!(e.store.get_grant(&g.id).unwrap().used_count, 2);
}

#[test]
fn untrusted_content_rule_overrides_standing_grants_for_that_task_only() {
    let mut e = env();
    let (a, b) = (e.task.clone(), e.new_task("b_out"));
    let g = e.grant("write", &e.granted());
    e.add(g);
    e.store.mark_task_untrusted(&b, e.clock.now_utc()).unwrap();
    let target = e.path("granted/a.txt");
    let mail = || target.clone();
    assert!(matches!(
        e.check(&e.for_task(&a, Action::Write, mail(), "h")),
        Decision::Allow { .. }
    ));
    assert_eq!(
        e.check(&e.for_task(&b, Action::Write, mail(), "h")),
        Decision::NeedsApproval {
            reason: ApprovalReason::UntrustedContent
        }
    );
}

#[test]
fn task_scoped_grant_needs_a_source_task_and_an_approval_cannot_create_a_standing_one() {
    let mut e = env();
    let bad = NewGrant {
        scope: GrantScope::Task,
        source_task: None,
        ..e.grant("read", &e.granted())
    };
    assert!(matches!(
        e.store.insert_grant(&bad, e.clock.now_utc()),
        Err(LumaError::InvalidInput { .. })
    ));
    // a task-scoped grant whose task row is gone (source_task became NULL) matches nobody
    let orphan = e.task_grant(&e.task.clone(), "read", &e.granted());
    e.store
        .conn
        .execute(
            "UPDATE grants SET source_task = NULL WHERE id = ?1",
            [orphan.id.as_str()],
        )
        .unwrap();
    let r = e.req(Action::Read, e.path("granted/a.txt"), "h");
    assert_eq!(e.check(&r), NEEDS);

    // approvals: the grant they create must be task-scoped to the approval's own task
    let other = e.new_task("b_out");
    let approval = e
        .store
        .create_approval(
            &crate::storage::NewApproval {
                task_id: e.task.clone(),
                tool_call_id: crate::schema::ToolCallId::from_string("c1"),
                tool: "t".into(),
                action: "write".into(),
                resource: "r".into(),
                resolved_resource: None,
                arguments: json!({}),
                arguments_hash: "h".into(),
                preview: "p".into(),
                detail: None,
                detail_truncated: false,
                reason: "no_matching_grant".into(),
                expires_at: e.clock.now_utc() + chrono::Duration::minutes(10),
            },
            e.clock.now_utc(),
        )
        .unwrap();
    for g in [
        NewGrant {
            scope: GrantScope::UserStanding,
            source_task: Some(e.task.clone()),
            ..e.grant("write", "/x")
        },
        NewGrant {
            scope: GrantScope::Task,
            source_task: Some(other),
            ..e.grant("write", "/x")
        },
    ] {
        assert!(e
            .store
            .resolve_approval(&approval.id, Some(&g), e.clock.now_utc())
            .is_err());
    }
    let ok = NewGrant {
        scope: GrantScope::Task,
        source_task: Some(e.task.clone()),
        ..e.grant("write", "/x")
    };
    assert!(e
        .store
        .resolve_approval(&approval.id, Some(&ok), e.clock.now_utc())
        .is_ok());
}

// ---------------------------------------------------------------- 2. consumption + write-ahead step

fn start<'a>(key: &'a str, side: SideEffect) -> StepStart<'a> {
    StepStart {
        tool: "t",
        args_redacted: Some("{}"),
        side_effect: side,
        idempotency_key: Some(key),
        reopen: None,
    }
}

fn used(e: &Env, id: &GrantId) -> u32 {
    e.store.get_grant(id).unwrap().used_count
}

#[test]
fn consuming_a_grant_and_recording_the_step_are_one_transaction() {
    let mut e = env();
    let a = e.task.clone();
    e.run_task(&a);
    let h = "h1";
    let g = e.one_shot(
        &a,
        "write",
        &e.root.join("granted/out").display().to_string(),
        h,
    );
    let r = e.req(Action::Write, e.path("granted/out/x.md"), h);

    let (d, step) = authorize_and_begin_step(
        &mut e.store,
        &r,
        &e.clock,
        &start("k1", SideEffect::LocalWrite),
    )
    .unwrap();
    assert_eq!(
        d,
        Decision::Allow {
            grant_id: g.id.clone()
        }
    );
    let step = step.unwrap();
    assert_eq!(step.status, StepStatus::Started);
    assert_eq!(step.consumed_grant_id.as_ref(), Some(&g.id));
    assert_eq!(used(&e, &g.id), 1);

    // refusals write nothing
    let (d, step) = authorize_and_begin_step(
        &mut e.store,
        &r,
        &e.clock,
        &start("k2", SideEffect::LocalWrite),
    )
    .unwrap();
    assert_eq!(d, NEEDS);
    assert!(step.is_none());
    assert_eq!(e.store.list_steps(&a).unwrap().len(), 1);
}

#[test]
fn crash_before_the_step_record_commits_leaves_the_approval_unspent() {
    let mut e = env();
    let a = e.task.clone();
    e.run_task(&a);
    let g = e.one_shot(
        &a,
        "write",
        &e.root.join("granted/out").display().to_string(),
        "h",
    );
    let r = e.req(Action::Write, e.path("granted/out/x.md"), "h");

    // (a) the process dies after the grant was consumed but before the step is written:
    // both live in the same uncommitted transaction, which a crash rolls back.
    {
        let tx = e.store.conn.transaction().unwrap();
        let d = super::evaluate_in(&tx, &r, e.clock.now_utc(), &super::OsFs).unwrap();
        assert!(matches!(d, Decision::Allow { .. }));
        drop(tx); // crash
    }
    assert_eq!(used(&e, &g.id), 0);
    assert!(e.store.list_steps(&a).unwrap().is_empty());

    // (b) writing the step fails: the consumption is rolled back with it
    let before = e.store.list_steps(&a).unwrap().len();
    e.store
        .transition_task(&a, TaskStatus::Paused, &[], Some("t"), &e.clock)
        .unwrap(); // begin_step needs a running task
    let err = authorize_and_begin_step(
        &mut e.store,
        &r,
        &e.clock,
        &start("k", SideEffect::LocalWrite),
    );
    assert!(err.is_err());
    assert_eq!(used(&e, &g.id), 0, "approval not spent without a step");
    assert_eq!(e.store.list_steps(&a).unwrap().len(), before);

    // the approval still works afterwards
    e.store
        .transition_task(&a, TaskStatus::Running, &[], Some("t"), &e.clock)
        .unwrap();
    let (d, step) = authorize_and_begin_step(
        &mut e.store,
        &r,
        &e.clock,
        &start("k", SideEffect::LocalWrite),
    )
    .unwrap();
    assert!(matches!(d, Decision::Allow { .. }) && step.is_some());
}

#[test]
fn crash_after_commit_external_write_is_uncertain_and_the_approval_stays_spent() {
    let mut e = env();
    let a = e.task.clone();
    e.run_task(&a);
    let g = e.one_shot(&a, "send", "mail:boss@x.com", "h");
    let r = e.req(Action::Send, Resource::Other("mail:boss@x.com".into()), "h");
    let (_, step) = authorize_and_begin_step(
        &mut e.store,
        &r,
        &e.clock,
        &start("mail:h", SideEffect::ExternalWrite),
    )
    .unwrap();
    let step = step.unwrap();
    // crash: nothing else happens; restart recovery runs
    let rec = e.store.recover_after_restart(&e.clock).unwrap();
    assert_eq!((rec.len(), rec[0].to), (1, TaskStatus::Uncertain));
    assert_eq!(
        e.store.get_step(&step.id).unwrap().status,
        StepStatus::Uncertain
    );
    assert_eq!(
        used(&e, &g.id),
        1,
        "never given back: the send may have happened"
    );
    // it can neither be re-opened nor re-started under the same key, and nothing is consumed
    let again = StepStart {
        reopen: Some(&step.id),
        ..start("mail:h", SideEffect::ExternalWrite)
    };
    if let Ok((d, s)) = authorize_and_begin_step(&mut e.store, &r, &e.clock, &again) {
        assert!(!matches!(d, Decision::Allow { .. }) && s.is_none());
    }
    assert_eq!(used(&e, &g.id), 1);
    // even with a fresh, unlimited grant in place the uncertain external step is not re-run
    let standing = e.one_shot(&a, "send", "mail:boss@x.com", "h");
    let again = StepStart {
        reopen: Some(&step.id),
        ..start("mail:h", SideEffect::ExternalWrite)
    };
    assert!(authorize_and_begin_step(&mut e.store, &r, &e.clock, &again).is_err());
    assert_eq!(
        used(&e, &standing.id),
        0,
        "refused re-open consumed nothing"
    );
    assert_eq!(
        e.store.get_step(&step.id).unwrap().status,
        StepStatus::Uncertain
    );
}

#[test]
fn crash_after_commit_local_write_is_interrupted_and_its_use_is_restored_once() {
    let mut e = env();
    let a = e.task.clone();
    e.run_task(&a);
    let scope = e.root.join("granted/out").display().to_string();
    let g = e.one_shot(&a, "write", &scope, "h");
    let r = e.req(Action::Write, e.path("granted/out/x.md"), "h");
    let (_, step) = authorize_and_begin_step(
        &mut e.store,
        &r,
        &e.clock,
        &start("k", SideEffect::LocalWrite),
    )
    .unwrap();
    let step = step.unwrap();
    assert_eq!(used(&e, &g.id), 1);

    let rec = e.store.recover_after_restart(&e.clock).unwrap();
    assert_eq!((rec.len(), rec[0].to), (1, TaskStatus::Paused));
    let s = e.store.get_step(&step.id).unwrap();
    assert_eq!(s.status, StepStatus::Interrupted);
    assert_eq!(s.consumed_grant_id, None);
    assert_eq!(
        used(&e, &g.id),
        0,
        "the interrupted attempt's use is given back"
    );
    // a second recovery must not give back more
    e.store.recover_after_restart(&e.clock).unwrap();
    assert_eq!(used(&e, &g.id), 0);

    // resume: the re-run is authorised again with the same approval, once
    e.store
        .transition_task(&a, TaskStatus::Running, &[], Some("resume"), &e.clock)
        .unwrap();
    let again = StepStart {
        reopen: Some(&step.id),
        ..start("k", SideEffect::LocalWrite)
    };
    let (d, s2) = authorize_and_begin_step(&mut e.store, &r, &e.clock, &again).unwrap();
    assert!(matches!(d, Decision::Allow { .. }));
    assert_eq!(s2.unwrap().id, step.id, "same step row reused");
    assert_eq!(used(&e, &g.id), 1);
    let (d, _) = authorize_and_begin_step(
        &mut e.store,
        &r,
        &e.clock,
        &start("k2", SideEffect::LocalWrite),
    )
    .unwrap();
    assert_eq!(d, NEEDS, "the approval covers one operation, not two");
}

#[test]
fn crash_before_policy_at_all_changes_nothing() {
    // boundary "after the model asked, before authorisation": no step, no grant touched
    let mut e = env();
    let a = e.task.clone();
    e.run_task(&a);
    let g = e.one_shot(
        &a,
        "write",
        &e.root.join("granted/out").display().to_string(),
        "h",
    );
    let rec = e.store.recover_after_restart(&e.clock).unwrap();
    assert_eq!(rec[0].to, TaskStatus::Paused);
    assert_eq!(used(&e, &g.id), 0);
    assert!(e.store.list_steps(&a).unwrap().is_empty());
}

// ---------------------------------------------------------------- 4a. lifetime of task-scoped grants

#[test]
fn finishing_a_task_revokes_its_task_scoped_grants_only() {
    let mut e = env();
    let (a, b) = (e.task.clone(), e.new_task("b_out"));
    let implied = e.task_grant(&a, "read", &e.granted());
    let unspent = e.one_shot(&a, "send", "mail:x@y.z", "h");
    let others = e.task_grant(&b, "read", &e.granted());
    let standing = e.grant("read", &e.granted());
    let standing = e.add(standing);
    e.run_task(&a);
    e.store
        .transition_task(&a, TaskStatus::Failed, &[], Some("t"), &e.clock)
        .unwrap();
    for g in [&implied, &unspent] {
        assert!(e.store.get_grant(&g.id).unwrap().revoked_at.is_some());
    }
    assert!(e.store.get_grant(&others.id).unwrap().revoked_at.is_none());
    assert!(e
        .store
        .get_grant(&standing.id)
        .unwrap()
        .revoked_at
        .is_none());
    // cancelled (from queued) behaves the same
    e.store
        .transition_task(&b, TaskStatus::Cancelled, &[], Some("t"), &e.clock)
        .unwrap();
    assert!(e.store.get_grant(&others.id).unwrap().revoked_at.is_some());
    assert!(e
        .store
        .get_grant(&standing.id)
        .unwrap()
        .revoked_at
        .is_none());
}

#[test]
fn a_finished_task_cannot_act_and_recovery_repairs_leftover_grants() {
    let mut e = env();
    let a = e.task.clone();
    let g = e.task_grant(&a, "read", &e.granted());
    // simulate a crash between "task became terminal" and "grants revoked" (as the old
    // code could leave it): the status is written without going through the transition
    e.store
        .conn
        .execute(
            "UPDATE tasks SET status = 'cancelled' WHERE id = ?1",
            [a.as_str()],
        )
        .unwrap();
    assert!(e.store.get_grant(&g.id).unwrap().revoked_at.is_none());
    // even before repair the grant cannot be used: a terminal task is refused outright
    let r = e.req(Action::Read, e.path("granted/a.txt"), "h");
    assert!(matches!(e.check(&r), Decision::Deny { .. }));
    assert_eq!(used(&e, &g.id), 0);

    e.store.recover_after_restart(&e.clock).unwrap();
    assert!(e.store.get_grant(&g.id).unwrap().revoked_at.is_some());
    assert!(!e
        .store
        .has_active_grant_for_action("read", e.clock.now_utc())
        .unwrap());
}

#[test]
fn expired_and_revoked_task_scoped_grants_are_ignored() {
    let mut e = env();
    let a = e.task.clone();
    let mut exp = NewGrant {
        source_task: Some(a.clone()),
        scope: GrantScope::Task,
        ..e.grant("read", &e.granted())
    };
    exp.expires_at = Some(e.clock.now_utc() + chrono::Duration::minutes(5));
    let exp = e.add(exp);
    let rev = e.task_grant(&a, "write", &e.granted());
    let r = e.req(Action::Read, e.path("granted/a.txt"), "h");
    assert!(matches!(e.check(&r), Decision::Allow { .. }));
    e.clock.advance(Duration::from_secs(300));
    assert_eq!(e.check(&r), NEEDS, "expired exactly at the boundary");
    e.store.revoke_grant(&rev.id, e.clock.now_utc()).unwrap();
    let w = e.req(Action::Write, e.path("granted/a.txt"), "h");
    assert_eq!(e.check(&w), NEEDS);
    assert_eq!(used(&e, &exp.id), 1);
    assert_eq!(used(&e, &rev.id), 0);
}

// ---------------------------------------------------------------- 4b. non-path wildcard rule

#[test]
fn wildcard_scopes_follow_the_documented_prefix_rule() {
    let m = other_scope_matches;
    // exact
    assert!(m("cal-1", "cal-1"));
    assert!(!m("cal-1", "cal-10"));
    assert!(!m("cal-1", "CAL-1"), "case-sensitive");
    // plain prefixes
    assert!(m("mail:*", "mail:a@x.com"));
    assert!(m("mail:*", "mail:"));
    assert!(!m("mail:*", "mailto:a@x.com"));
    assert!(m("cal-*", "cal-10"));
    // `*` only counts as the last character; a bare or doubled star matches nothing
    assert!(!m("*", "anything"));
    assert!(!m("a*b*", "a-b-c"));
    assert!(m("a*b", "a*b"));
    assert!(!m("a*b", "axb"));
    // URL-like: scheme and whole host must be pinned by the prefix
    assert!(m("https://docs.rs/*", "https://docs.rs/serde/latest"));
    assert!(!m("https://docs.rs/*", "https://docs.rs.evil.example/x"));
    assert!(!m("https://docs.rs/*", "https://docs.rs@evil.example/x"));
    assert!(!m("https://docs.rs/*", "http://docs.rs/x"));
    assert!(!m("https://docs.rs*", "https://docs.rs.evil.example/x"));
    assert!(
        !m("https://docs.rs*", "https://docs.rs/x"),
        "unpinned host: no match at all"
    );
    assert!(!m("https*", "https://anything/"));
    assert!(!m("https:*", "https://anything/"));
    assert!(!m("http*", "https://docs.rs/"));
    assert!(!m("https://*", "https://anything/"));
    // a path prefix inside a pinned host is a plain string prefix
    assert!(m("https://docs.rs/serde/*", "https://docs.rs/serde/1.0"));
    assert!(m(
        "https://docs.rs/serde*",
        "https://docs.rs/serde_json/1.0"
    ));
    // traversal / encoded separators cannot climb out of the prefix
    assert!(!m("https://docs.rs/api/*", "https://docs.rs/api/../admin"));
    assert!(!m(
        "https://docs.rs/api/*",
        "https://docs.rs/api/%2e%2e/admin"
    ));
    assert!(!m(
        "https://docs.rs/api/*",
        "https://docs.rs/api/%2E%2E%2fadmin"
    ));
    assert!(!m(
        "https://docs.rs/api/*",
        "https://docs.rs/api\\..\\admin"
    ));
    assert!(m("https://docs.rs/api/*", "https://docs.rs/api/v1..2/x"));
    // exact scopes are compared verbatim
    assert!(m("https://docs.rs/a/../b", "https://docs.rs/a/../b"));
}

// ---------------------------------------------------------------- 4c. arguments hash

fn h(text: &str) -> String {
    hash_arguments(&serde_json::from_str(text).unwrap())
}

#[test]
fn arguments_hash_ignores_key_order_and_whitespace() {
    let a = h(r#"{"to":"a@x.com","body":"hi","opts":{"b":[1,2,{"y":1,"x":2}],"a":null}}"#);
    let b = h("{ \"opts\" : { \"a\" : null , \"b\" : [ 1 , 2 , { \"x\" : 2 , \"y\" : 1 } ] } ,\n\t\"body\" : \"hi\", \"to\":\"a@x.com\" }");
    assert_eq!(a, b);
    // unicode escapes and literal characters are the same string
    assert_eq!(h(r#"{"t":"café"}"#), h(r#"{"t":"café"}"#));
    // order inside arrays matters; so do types, nulls and missing keys
    assert_ne!(h("[1,2]"), h("[2,1]"));
    assert_ne!(h(r#"{"a":1}"#), h(r#"{"a":"1"}"#));
    assert_ne!(h(r#"{"a":null}"#), h("{}"));
    assert_ne!(h(r#"{"a":true}"#), h(r#"{"a":"true"}"#));
    // keys are compared as bytes, not by locale
    assert_eq!(h(r#"{"b":1,"B":2,"é":3}"#), h(r#"{"é":3,"B":2,"b":1}"#));
}

#[test]
fn arguments_hash_number_rule() {
    // whole numbers hash the same however they are written
    assert_eq!(h(r#"{"n":1}"#), h(r#"{"n":1.0}"#));
    assert_eq!(h(r#"{"n":1}"#), h(r#"{"n":1e0}"#));
    assert_eq!(h(r#"{"n":100}"#), h(r#"{"n":1E2}"#));
    assert_eq!(h(r#"{"n":0}"#), h(r#"{"n":-0.0}"#));
    assert_eq!(h(r#"{"n":-3}"#), h(r#"{"n":-3.0}"#));
    // different values stay different
    assert_ne!(h(r#"{"n":1}"#), h(r#"{"n":1.5}"#));
    assert_ne!(h(r#"{"n":1}"#), h(r#"{"n":-1}"#));
    assert_eq!(h(r#"{"n":1.5}"#), h(r#"{"n":1.50}"#));
    // large integers stay exact (no float rounding collisions)
    assert_ne!(h("9007199254740993"), h("9007199254740992"));
    assert_eq!(h("9007199254740993"), h("9007199254740993"));
    assert_ne!(h("1e300"), h("1e301"));
    // typical arguments hash exactly as before the canonical form was introduced, so
    // approvals stored by earlier builds stay valid
    let v = json!({"to": "a@x.com", "body": "hi", "n": 3, "list": [true, null, "é\n"]});
    assert_eq!(
        hash_arguments(&v),
        crate::schema::sha256_hex(v.to_string().as_bytes())
    );
}

// ---------------------------------------------------------------------------------------
// H3: no file-system access for a path nothing grants; Windows path forms (security review)

use std::sync::Mutex;

/// A file system that records every call policy makes to it.
#[derive(Debug)]
struct SpyFs {
    windows: bool,
    calls: Mutex<Vec<String>>,
}

impl SpyFs {
    fn new(windows: bool) -> Self {
        Self {
            windows,
            calls: Mutex::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl paths::FsPort for SpyFs {
    fn windows_rules(&self) -> bool {
        self.windows
    }
    fn canonicalize(&self, path: &std::path::Path) -> std::io::Result<PathBuf> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("canonicalize {}", path.display()));
        fs::canonicalize(path)
    }
    fn exists_no_follow(&self, path: &std::path::Path) -> bool {
        self.calls
            .lock()
            .unwrap()
            .push(format!("metadata {}", path.display()));
        fs::symlink_metadata(path).is_ok()
    }
}

fn check_with(e: &mut Env, fs: &SpyFs, r: &ActionRequest) -> Decision {
    evaluate_with(&mut e.store, r, &e.clock, fs).unwrap()
}

/// Reviewer repro R4 (H3), inverted: a path that no grant covers used to be looked at
/// (three `statx` calls in the reviewer's strace; on Windows an SMB connection) before the
/// decision. Now the decision is made without touching the file system at all.
#[test]
fn review_r4_policy_never_touches_the_fs_for_a_path_no_grant_covers() {
    let mut e = env();
    let spy = SpyFs::new(false);
    // with no grant at all
    let probe = Resource::Path(PathBuf::from("/review-probe-SECRET-DATA-IN-NAME/share/x"));
    let r = e.req(Action::Read, probe.clone(), "h");
    assert_eq!(
        check_with(&mut e, &spy, &r),
        Decision::NeedsApproval {
            reason: ApprovalReason::NoMatchingGrant
        }
    );
    assert!(spy.calls().is_empty(), "{:?}", spy.calls());

    // with a grant that covers a DIFFERENT directory
    let g = e.grant("read", &e.granted());
    e.add(g);
    for p in [
        "/review-probe-SECRET-DATA-IN-NAME/share/x".to_owned(),
        e.root
            .join("granted-but-not-really/x")
            .display()
            .to_string(), // prefix lookalike
        e.root.join("elsewhere/x").display().to_string(),
    ] {
        let r = e.req(Action::Read, Resource::Path(PathBuf::from(&p)), "h");
        assert!(
            matches!(check_with(&mut e, &spy, &r), Decision::NeedsApproval { .. }),
            "{p}"
        );
    }
    assert!(spy.calls().is_empty(), "{:?}", spy.calls());

    // a path the grant does cover is the only one that may be resolved, and only under
    // the granted root
    let r = e.req(Action::Read, e.path("granted/a.txt"), "h");
    assert!(matches!(
        check_with(&mut e, &spy, &r),
        Decision::Allow { .. }
    ));
    let calls = spy.calls();
    assert!(!calls.is_empty());
    let granted = e.granted();
    assert!(
        calls.iter().all(|c| c.contains(&granted)),
        "only the granted tree was touched: {calls:?}"
    );
}

/// The same holds for writes and for the untrusted-content rule: a tainted task asking
/// for an out-of-scope path is not resolved to find out whether it is out of scope.
#[test]
fn a_tainted_task_asking_for_an_uncovered_path_causes_no_fs_access() {
    let mut e = env();
    let spy = SpyFs::new(false);
    e.store
        .mark_task_untrusted(&e.task.clone(), e.clock.now_utc())
        .unwrap();
    let r = e.req(
        Action::Write,
        Resource::Path(PathBuf::from("/nowhere-special/out.md")),
        "h",
    );
    assert_eq!(
        check_with(&mut e, &spy, &r),
        Decision::NeedsApproval {
            reason: ApprovalReason::UntrustedContent
        }
    );
    assert!(spy.calls().is_empty(), "{:?}", spy.calls());
}

/// Every Windows form that can reach the network, a device or a stream is refused by the
/// lexical check alone: no grant is consulted and the file system is never touched.
#[test]
fn windows_path_forms_are_denied_lexically_without_touching_the_fs() {
    let mut e = env();
    // even a grant that names the very same string cannot make these acceptable
    let spy = SpyFs::new(true);
    let forms = [
        r"\\server\share\x.txt",
        "//server/share/x.txt",
        r"\\.\pipe\secret",
        r"\\.\C:\x.txt",
        r"\\?\GLOBALROOT\Device\HarddiskVolume1\x",
        r"\\?\UNC\server\share\x.txt",
        r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\x",
        r"C:foo.txt",
        r"C:",
        r"\Users\x.txt",
        r"C:\Users\u\x.txt:hidden",
        r"C:\Users\u\x.txt::$DATA",
        "C:\\Users\\u\\x\0.txt",
        r"C:\Users\u\NUL",
        r"C:\Users\u\con.txt",
        r"C:\Users\u\COM1",
        r"C:\Users\u\..\..\Windows\x",
        r"C:\Users\u\x?.txt",
        r"C:\Users\u\ ..\x",
        r"\\?\C:/Users/u/x.txt", // '/' is a literal inside a verbatim path
    ];
    for form in forms {
        for action in [Action::Read, Action::Write] {
            // the form itself is also granted, as a standing grant
            let g = e.grant(action.as_str(), form);
            e.add(g);
            let r = e.req(action.clone(), Resource::Path(PathBuf::from(form)), "h");
            let d = check_with(&mut e, &spy, &r);
            assert!(
                matches!(d, Decision::Deny { .. }),
                "{form:?} {action:?} gave {d:?}"
            );
        }
    }
    assert!(spy.calls().is_empty(), "{:?}", spy.calls());
}

/// An ordinary Windows path that nothing grants asks for approval, again without any fs
/// call (the host here is not Windows, so a call would be visible in the spy).
#[test]
fn an_ungranted_windows_drive_path_needs_approval_without_fs_access() {
    let mut e = env();
    let spy = SpyFs::new(true);
    let g = e.grant("read", r"C:\Users\u\docs");
    e.add(g);
    for p in [
        r"D:\other\x.txt",
        r"C:\Users\u\docs2\x.txt",
        r"C:\Users\v\x.txt",
    ] {
        let r = e.req(Action::Read, Resource::Path(PathBuf::from(p)), "h");
        assert!(
            matches!(check_with(&mut e, &spy, &r), Decision::NeedsApproval { .. }),
            "{p}"
        );
    }
    assert!(spy.calls().is_empty(), "{:?}", spy.calls());
}

/// Review item: the wildcard `example.test*` must not cover `example.test.evil.com`
/// (the host prefix is not pinned). Exact regression, plus the shapes around it.
#[test]
fn a_bare_host_prefix_wildcard_never_covers_a_longer_host() {
    let m = other_scope_matches;
    assert!(!m("example.test*", "example.test.evil.com"));
    assert!(!m("example.test*", "example.test"));
    assert!(!m("example.test*", "example.testing"));
    assert!(!m("example.test-*", "example.test-evil.com"));
    assert!(!m(
        "https://example.test*",
        "https://example.test.evil.com/x"
    ));
    assert!(!m("https://example.test*", "https://example.test/x"));
    // the documented spellings
    assert!(m("example.test", "example.test"));
    assert!(!m("example.test", "example.test.evil.com"));
    assert!(m("https://example.test/*", "https://example.test/x"));
    assert!(!m(
        "https://example.test/*",
        "https://example.test.evil.com/x"
    ));
    assert!(!m(
        "https://example.test/*",
        "https://example.test@evil.com/x"
    ));

    // and through the policy: a grant with that scope authorises nothing
    let mut e = env();
    let g = e.grant("web_fetch", "example.test*");
    e.add(g);
    for host in ["example.test.evil.com", "example.test"] {
        let r = e.req(
            Action::Other("web_fetch".into()),
            Resource::Other(host.into()),
            "h",
        );
        assert!(
            matches!(e.check(&r), Decision::NeedsApproval { .. }),
            "{host}"
        );
    }
}
