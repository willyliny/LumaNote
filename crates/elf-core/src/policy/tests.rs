use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::ports::fakes::FakeClock;
use crate::ports::Clock;
use crate::schema::{hash_arguments, Trust};
use crate::storage::NewGrant;

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
        }
    }
    fn add(&mut self, g: NewGrant) -> crate::storage::Grant {
        self.store.insert_grant(&g, self.clock.now_utc()).unwrap()
    }
    fn granted(&self) -> String {
        self.root.join("granted").display().to_string()
    }
    fn req(&self, action: Action, resource: Resource, hash: &str) -> ActionRequest {
        ActionRequest {
            task_id: self.task.clone(),
            action,
            resource,
            arguments_hash: hash.into(),
        }
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
        Err(ElfError::NoPermission { .. })
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
    // scope given through a symlinked alias still works for honest paths
    symlink(e.root.join("granted"), e.root.join("alias")).unwrap();
    let r = e.req(Action::Read, e.path("alias/a.txt"), "h");
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
        Err(ElfError::ApprovalRequired { .. })
    ));
    assert_eq!(e.store.get_grant(&g.id).unwrap().used_count, 1);
}

#[test]
fn arguments_hash_must_match_exactly() {
    let mut e = env();
    let approved = hash_arguments(&json!({"to": "a@x.com", "body": "hi"}));
    let changed = hash_arguments(&json!({"to": "a@x.com", "body": "hi, plus a link"}));
    let mut g = e.grant("send", "mail:a@x.com");
    g.arguments_hash = Some(approved.clone());
    g.max_uses = Some(1);
    e.add(g);
    let mail = Resource::Other("mail:a@x.com".into());
    let r = e.req(Action::Send, mail.clone(), &changed);
    assert_eq!(
        e.check(&r),
        Decision::NeedsApproval {
            reason: ApprovalReason::NoMatchingGrant
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

    // before the flag: standing grants apply
    for r in [&in_scope_write, &out_scope_write, &delete, &send, &read] {
        assert!(
            matches!(e.check(r), Decision::Allow { .. }),
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
            ..e.grant("send", "mail:*")
        },
        NewGrant {
            arguments_hash: None,
            max_uses: Some(1),
            source_task: Some(e.task.clone()),
            ..e.grant("send", "mail:*")
        },
        NewGrant {
            arguments_hash: Some(h.clone()),
            max_uses: Some(1),
            source_task: Some(other_task),
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
fn trusted_sources_do_not_set_the_flag() {
    let mut e = env();
    let g = e.grant("send", "mail:*");
    e.add(g);
    e.store
        .record_tool_result_trust(&e.task.clone(), Trust::LocalGranted, e.clock.now_utc())
        .unwrap();
    let send = e.req(Action::Send, Resource::Other("mail:x@y.z".into()), "h");
    assert!(matches!(e.check(&send), Decision::Allow { .. }));
}

#[test]
fn unknown_task_is_invalid_input_and_helpers_work() {
    let mut e = env();
    let mut r = e.req(Action::Read, e.path("granted/a.txt"), "h");
    r.task_id = TaskId::from_string("missing");
    assert!(matches!(
        evaluate(&mut e.store, &r, &e.clock),
        Err(ElfError::InvalidInput { .. })
    ));
    assert_eq!(Action::parse("read"), Action::Read);
    assert_eq!(Action::parse("calendar.create").as_str(), "calendar.create");
    assert_eq!(Action::Other("x".into()).class(), ActionClass::Other);
    let _keep = &e.dir;
}
