//! Unit tests of the registry and the three built-in tools.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::policy::{evaluate, ActionRequest, ApprovalReason, Decision};
use crate::ports::fakes::FakeClock;
use crate::schema::{hash_arguments, sha256_hex};
use crate::storage::{NewGrant, Store};

struct Env {
    store: SharedStore,
    clock: FakeClock,
    _dir: tempfile::TempDir,
    out: PathBuf,
    shared: PathBuf,
    outside: PathBuf,
    task: TaskId,
    scope: TaskScope,
}

fn env() -> Env {
    let clock = FakeClock::at_default_start();
    let store = SharedStore::new(Store::open_in_memory(&clock).unwrap());
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (out, shared, outside) = (root.join("out"), root.join("shared"), root.join("outside"));
    for d in [&out, &shared, &outside] {
        std::fs::create_dir_all(d).unwrap();
    }
    let scope = TaskScope {
        output_dirs: vec![out.clone()],
        read_dirs: vec![shared.clone()],
        allowed_tools: None,
    };
    let now = clock.now_utc();
    let task = store
        .with(|s| s.create_task("t", &scope.to_json(), now))
        .unwrap()
        .id;
    store
        .with(|s| {
            for (a, d) in [("write", &out), ("read", &shared)] {
                s.insert_grant(
                    &NewGrant {
                        action: a.into(),
                        resource_scope: d.display().to_string(),
                        arguments_hash: None,
                        expires_at: None,
                        max_uses: None,
                        source_task: None,
                    },
                    now,
                )?;
            }
            Ok(())
        })
        .unwrap();
    Env {
        store,
        clock,
        _dir: dir,
        out,
        shared,
        outside,
        task,
        scope,
    }
}

impl Env {
    fn ctx(&self) -> ToolContext {
        ToolContext {
            task_id: self.task.clone(),
            store: self.store.clone(),
            ports: Ports::new(Arc::new(self.clock.clone())),
            cancel: CancellationToken::new(),
            scope: self.scope.clone(),
            idempotency_key: "k".into(),
        }
    }
    fn policy(&self, tool: &dyn Tool, args: &Value) -> Result<Decision> {
        let resource = tool.resource(args, &self.ctx())?;
        let req = ActionRequest {
            task_id: self.task.clone(),
            action: tool.required_action(),
            resource,
            arguments_hash: hash_arguments(args),
        };
        self.store.with(|s| evaluate(s, &req, &self.clock))
    }
}

fn md(path: &str, content: &str) -> Value {
    json!({"path": path, "content": content})
}

// ----- registry --------------------------------------------------------------------------

#[test]
fn registry_validates_schemas_names_and_duplicates() {
    let mut r = Registry::with_builtin_tools().unwrap();
    assert_eq!(
        r.names(),
        vec!["create_todos", "read_file", "write_markdown"]
    );
    assert!(
        r.register(Arc::new(ReadFileTool::default())).is_err(),
        "duplicate"
    );

    struct Bad(&'static str, Value, bool);
    #[async_trait::async_trait]
    impl Tool for Bad {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            ""
        }
        fn parameters_schema(&self) -> Value {
            self.1.clone()
        }
        fn required_action(&self) -> Action {
            Action::Read
        }
        fn side_effect(&self) -> SideEffect {
            SideEffect::None
        }
        fn timeout(&self) -> Duration {
            Duration::from_secs(1)
        }
        fn max_output_bytes(&self) -> usize {
            10
        }
        fn implemented(&self) -> bool {
            self.2
        }
        fn resource(&self, _: &Value, _: &ToolContext) -> Result<Resource> {
            Ok(Resource::Other("x".into()))
        }
        fn preview(&self, _: &Value, _: &Resource) -> String {
            String::new()
        }
        async fn execute(&self, _: Value, _: ToolContext) -> Result<ToolOutput> {
            Err(ElfError::Cancelled)
        }
    }
    let ok_schema = json!({"type": "object"});
    assert!(r
        .register(Arc::new(Bad("fine", ok_schema.clone(), true)))
        .is_ok());
    assert!(r
        .register(Arc::new(Bad("has space", ok_schema.clone(), true)))
        .is_err());
    assert!(r
        .register(Arc::new(Bad("", ok_schema.clone(), true)))
        .is_err());
    assert!(r
        .register(Arc::new(Bad("x", json!({"type": "string"}), true)))
        .is_err());
    assert!(r
        .register(Arc::new(Bad(
            "y",
            json!({"type": "object", "required": 3}),
            true
        )))
        .is_err());
    assert!(
        r.register(Arc::new(Bad("stub", ok_schema, false))).is_err(),
        "stubs are not registered"
    );
    r.declare_planned("calendar_create");
    assert!(r.get("calendar_create").is_none() && r.is_planned("calendar_create"));
    assert!(r.validate_arguments("nope", &json!({})).is_err());
}

#[test]
fn only_implemented_permitted_tools_are_exposed() {
    let e = env();
    let r = Registry::with_builtin_tools().unwrap();
    let now = e.clock.now_utc();
    let names = |scope: &TaskScope| -> Vec<String> {
        r.specs_for_task(&e.store, scope, now)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect()
    };
    // grants exist for read and write, but nobody granted create_todos
    assert_eq!(names(&e.scope), vec!["read_file", "write_markdown"]);
    e.store
        .with(|s| {
            s.insert_grant(
                &NewGrant {
                    action: "create_todos".into(),
                    resource_scope: "local:todos".into(),
                    arguments_hash: None,
                    expires_at: None,
                    max_uses: None,
                    source_task: None,
                },
                now,
            )
            .map(|_| ())
        })
        .unwrap();
    assert_eq!(
        names(&e.scope),
        vec!["create_todos", "read_file", "write_markdown"]
    );
    let only = TaskScope {
        allowed_tools: Some(vec!["read_file".into()]),
        ..e.scope.clone()
    };
    assert_eq!(names(&only), vec!["read_file"]);
    // revoking the read grant removes read_file from what the model sees
    let g = e
        .store
        .with(|s| s.list_grants())
        .unwrap()
        .into_iter()
        .find(|g| g.action == "read")
        .unwrap();
    e.store
        .with(|s| s.revoke_grant(&g.id, now).map(|_| ()))
        .unwrap();
    assert_eq!(names(&e.scope), vec!["create_todos", "write_markdown"]);
    // the specs carry real JSON Schemas
    let specs = r.specs_for_task(&e.store, &e.scope, now).unwrap();
    assert!(specs.iter().all(|s| s.parameters_schema["type"] == "object"
        && s.parameters_schema["additionalProperties"] == false));
}

#[test]
fn schema_validation_catches_type_required_enum_and_extra_fields() {
    let r = Registry::with_builtin_tools().unwrap();
    let bad = [
        ("write_markdown", json!({"path": "a"})),
        ("write_markdown", json!({"path": 1, "content": "x"})),
        (
            "write_markdown",
            json!({"path": "a", "content": "x", "mode": "overwrite"}),
        ),
        ("read_file", json!({"path": "/a", "max_bytes": 0})),
        ("read_file", json!({"path": "/a", "max_bytes": 99999999})),
        ("read_file", json!({"path": ""})),
        ("create_todos", json!({"todos": []})),
        (
            "create_todos",
            json!({"todos": [{"title": "t", "due_uncertain": "yes"}]}),
        ),
        (
            "create_todos",
            json!({"todos": [{"due_at": "2026-11-20T09:00:00Z"}]}),
        ),
        (
            "create_todos",
            json!({"todos": [{"title": "t", "due_at": "friday"}]}),
        ),
    ];
    for (tool, args) in bad {
        let e = r.validate_arguments(tool, &args).unwrap_err();
        assert!(
            matches!(&e, ElfError::InvalidInput { message } if message.contains("invalid arguments for")),
            "{args}: {e}"
        );
    }
    assert!(r
        .validate_arguments("write_markdown", &md("a", "b"))
        .is_ok());
    assert!(r
        .validate_arguments("read_file", &json!({"path": "/a", "max_bytes": 10}))
        .is_ok());
}

// ----- create_todos ------------------------------------------------------------------------

#[tokio::test]
async fn create_todos_stores_certain_and_uncertain_dates() {
    let e = env();
    let out = CreateTodosTool
        .execute(
            json!({"todos": [
                {"title": "  Pay invoice ", "due_at": "2026-11-20T09:00:00+08:00", "source_ref": "notes#2"},
                {"title": "Call Lee", "due_uncertain": true, "due_text": "maybe Tuesday"},
                {"title": "Tentative", "due_uncertain": true, "due_at": "2026-12-01T00:00:00Z"},
                {"title": "No date"}
            ]}),
            e.ctx(),
        )
        .await
        .unwrap();
    assert_eq!(out.trust, Trust::UserProvided);
    let v: Value = serde_json::from_str(&out.content).unwrap();
    assert_eq!(
        (
            v["count"].as_u64(),
            v["dates_needing_confirmation"].as_u64()
        ),
        (Some(4), Some(2))
    );
    let todos = e.store.with(|s| s.list_todos(Some(&e.task))).unwrap();
    assert_eq!(todos[0].title, "Pay invoice");
    assert_eq!(
        todos[0].due_at.unwrap().to_rfc3339(),
        "2026-11-20T01:00:00+00:00"
    );
    assert!(!todos[0].due_uncertain);
    assert_eq!(
        (
            todos[1].due_at,
            todos[1].due_uncertain,
            todos[1].due_text.as_deref()
        ),
        (None, true, Some("maybe Tuesday"))
    );
    assert!(todos[2].due_uncertain && todos[2].due_at.is_some());
    assert_eq!((todos[3].due_at, todos[3].due_uncertain), (None, false));
    assert!(
        todos
            .iter()
            .all(|t| t.status == crate::storage::TodoStatus::Open
                && t.created_at == e.clock.now_utc())
    );
}

#[tokio::test]
async fn create_todos_is_all_or_nothing_and_never_guesses() {
    let e = env();
    for bad in [
        json!({"todos": [{"title": "ok"}, {"title": "bad", "due_at": "2026-11-20"}]}),
        json!({"todos": [{"title": "ok"}, {"title": "vague", "due_uncertain": true}]}),
        json!({"todos": [{"title": "ok"}, {"title": "   "}]}),
    ] {
        let err = CreateTodosTool.execute(bad, e.ctx()).await.unwrap_err();
        assert!(matches!(err, ElfError::InvalidInput { .. }), "{err}");
    }
    assert!(
        e.store.with(|s| s.list_todos(None)).unwrap().is_empty(),
        "nothing partially stored"
    );
    let err = CreateTodosTool
        .execute(
            json!({"todos": [{"title": "x", "due_at": "tomorrow"}]}),
            e.ctx(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("due_uncertain"), "{err}");
}

// ----- read_file -------------------------------------------------------------------------------

#[tokio::test]
async fn read_file_reads_text_and_reports_truncation() {
    let e = env();
    let t = ReadFileTool::default();
    let p = e.shared.join("n.txt");
    std::fs::write(&p, "héllo 世界, 第二行").unwrap();
    let args = json!({"path": p.display().to_string()});
    assert!(matches!(
        e.policy(&t, &args).unwrap(),
        Decision::Allow { .. }
    ));
    let out = t.execute(args.clone(), e.ctx()).await.unwrap();
    assert_eq!(out.trust, Trust::LocalGranted);
    let v: Value = serde_json::from_str(&out.content).unwrap();
    assert_eq!(v["text"], "héllo 世界, 第二行");
    assert_eq!(
        (v["truncated"].clone(), v["encoding"].clone()),
        (json!(false), json!("utf-8"))
    );

    // cap lands in the middle of a multi-byte character: dropped cleanly, flagged
    let v: Value = serde_json::from_str(
        &t.execute(
            json!({"path": p.display().to_string(), "max_bytes": 8}),
            e.ctx(),
        )
        .await
        .unwrap()
        .content,
    )
    .unwrap();
    assert_eq!(v["truncated"], true);
    assert_eq!(v["text"], "héllo ");
    assert_eq!(
        v["file_size_bytes"].as_u64(),
        Some(std::fs::metadata(&p).unwrap().len())
    );

    // default cap applies to big files
    let big = e.shared.join("big.txt");
    std::fs::write(&big, "a".repeat(300_000)).unwrap();
    let v: Value = serde_json::from_str(
        &t.execute(json!({"path": big.display().to_string()}), e.ctx())
            .await
            .unwrap()
            .content,
    )
    .unwrap();
    assert_eq!(
        (v["truncated"].clone(), v["bytes_read"].clone()),
        (json!(true), json!(128 * 1024))
    );
}

#[tokio::test]
async fn read_file_handles_encodings_and_refuses_binary_or_directories() {
    let e = env();
    let t = ReadFileTool::default();
    let utf16: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain("備忘錄".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    std::fs::write(e.shared.join("notepad.txt"), utf16).unwrap();
    std::fs::write(e.shared.join("bin.dat"), [0u8, 1, 2, 255]).unwrap();
    std::fs::write(e.shared.join("latin1.txt"), [0x63, 0x61, 0x66, 0xE9]).unwrap();
    std::fs::create_dir(e.shared.join("sub")).unwrap();
    let read = |name: &str| {
        let p = e.shared.join(name).display().to_string();
        let t = ReadFileTool::default();
        let ctx = e.ctx();
        async move { t.execute(json!({"path": p}), ctx).await }
    };
    let v: Value = serde_json::from_str(&read("notepad.txt").await.unwrap().content).unwrap();
    assert_eq!(
        (v["text"].clone(), v["encoding"].clone()),
        (json!("備忘錄"), json!("utf-16le"))
    );
    for bad in ["bin.dat", "latin1.txt", "sub", "missing.txt"] {
        assert!(
            matches!(read(bad).await, Err(ElfError::InvalidInput { .. })),
            "{bad}"
        );
    }
    let _ = t;
}

#[test]
fn read_file_targets_are_checked_by_policy() {
    let e = env();
    let t = ReadFileTool::default();
    std::fs::write(e.outside.join("secret.txt"), "s").unwrap();
    let args = |p: PathBuf| json!({"path": p.display().to_string()});
    // outside every granted directory: the user must decide
    assert!(matches!(
        e.policy(&t, &args(e.outside.join("secret.txt"))).unwrap(),
        Decision::NeedsApproval {
            reason: ApprovalReason::NoMatchingGrant
        }
    ));
    // traversal is refused outright
    let trav = e.shared.join("..").join("outside").join("secret.txt");
    assert!(matches!(
        e.policy(&t, &args(trav)).unwrap(),
        Decision::Deny { .. }
    ));
    // relative paths and device paths never become targets
    assert!(t.resource(&json!({"path": "notes.txt"}), &e.ctx()).is_err());
    assert!(matches!(
        t.resource(&json!({"path": r"\\.\pipe\x"}), &e.ctx()),
        Err(ElfError::NoPermission { .. })
    ));
    // look-alike sibling of the granted directory
    let sibling = PathBuf::from(format!("{}-evil", e.shared.display()));
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(sibling.join("a.txt"), "x").unwrap();
    assert!(matches!(
        e.policy(&t, &args(sibling.join("a.txt"))).unwrap(),
        Decision::NeedsApproval { .. }
    ));
}

#[cfg(unix)]
#[test]
fn read_file_symlink_out_of_the_shared_folder_is_denied() {
    let e = env();
    std::fs::write(e.outside.join("secret.txt"), "s").unwrap();
    std::os::unix::fs::symlink(&e.outside, e.shared.join("link")).unwrap();
    let args = json!({"path": e.shared.join("link/secret.txt").display().to_string()});
    assert!(matches!(
        e.policy(&ReadFileTool::default(), &args).unwrap(),
        Decision::Deny { .. }
    ));
}

// ----- write_markdown --------------------------------------------------------------------------

#[tokio::test]
async fn write_markdown_writes_verifies_and_records_the_artifact() {
    let e = env();
    let t = WriteMarkdownTool;
    let args = md("Meeting 11-12: notes?", "# Hello\n世界\n");
    assert!(matches!(
        e.policy(&t, &args).unwrap(),
        Decision::Allow { .. }
    ));
    let out = t.execute(args, e.ctx()).await.unwrap();
    let Some(EvidenceRef::Artifact { id }) = out.evidence.clone() else {
        panic!("no artifact evidence")
    };
    let a = e.store.with(|s| s.get_artifact(&id)).unwrap();
    assert_eq!(
        a.path,
        e.out.join("Meeting 11-12_ notes_.md").display().to_string()
    );
    let bytes = std::fs::read(&a.path).unwrap();
    assert_eq!(bytes, "# Hello\n世界\n".as_bytes());
    assert_eq!(
        (a.content_hash.clone(), a.bytes),
        (sha256_hex(&bytes), bytes.len() as u64)
    );
    assert_eq!(
        (a.kind.as_str(), a.task_id.clone()),
        ("markdown", e.task.clone())
    );
    let v: Value = serde_json::from_str(&out.content).unwrap();
    assert_eq!(
        (
            v["verified"].clone(),
            v["renamed"].clone(),
            v["sha256"].clone()
        ),
        (json!(true), json!(false), json!(a.content_hash))
    );
}

#[tokio::test]
async fn write_markdown_never_overwrites_and_picks_new_names() {
    let e = env();
    let t = WriteMarkdownTool;
    std::fs::write(e.out.join("plan.md"), "ORIGINAL").unwrap();
    let r1 = t.execute(md("plan", "one"), e.ctx()).await.unwrap();
    let r2 = t.execute(md("plan.md", "two"), e.ctx()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(e.out.join("plan.md")).unwrap(),
        "ORIGINAL"
    );
    assert_eq!(
        std::fs::read_to_string(e.out.join("plan (2).md")).unwrap(),
        "one"
    );
    assert_eq!(
        std::fs::read_to_string(e.out.join("plan (3).md")).unwrap(),
        "two"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&r1.content).unwrap()["renamed"],
        true
    );
    assert_ne!(r1.evidence, r2.evidence);
    assert_eq!(
        e.store.with(|s| s.list_artifacts(&e.task)).unwrap().len(),
        2
    );
}

#[tokio::test]
async fn write_markdown_rerun_after_a_crash_does_not_duplicate_the_document() {
    let e = env();
    let t = WriteMarkdownTool;
    let first = t.execute(md("doc", "same"), e.ctx()).await.unwrap();
    // the step was never marked finished, so the orchestrator runs the tool once more
    let again = t.execute(md("doc", "same"), e.ctx()).await.unwrap();
    assert_eq!(first.evidence, again.evidence);
    assert_eq!(std::fs::read_dir(&e.out).unwrap().count(), 1);
    assert_eq!(
        e.store.with(|s| s.list_artifacts(&e.task)).unwrap().len(),
        1
    );
    // different content under the same name is a different document
    t.execute(md("doc", "other"), e.ctx()).await.unwrap();
    assert_eq!(std::fs::read_dir(&e.out).unwrap().count(), 2);
}

#[tokio::test]
async fn write_markdown_sanitises_names_and_creates_subfolders() {
    let e = env();
    let t = WriteMarkdownTool;
    for (given, expected) in [
        ("CON", "_CON.md"),
        ("aux.md", "_aux.md"),
        ("report. ", "report.md"),
        ("a<b>c|d", "a_b_c_d.md"),
        ("日本語のメモ", "日本語のメモ.md"),
        ("sub\\inner/deep name", "sub/inner/deep name.md"),
    ] {
        let out = t.execute(md(given, "x"), e.ctx()).await.unwrap();
        let v: Value = serde_json::from_str(&out.content).unwrap();
        let path = PathBuf::from(v["path"].as_str().unwrap());
        let rel: Vec<String> = path
            .strip_prefix(&e.out)
            .unwrap()
            .iter()
            .map(|c| c.to_string_lossy().into_owned())
            .collect();
        assert_eq!(rel.join("/"), expected, "{given}");
    }
}

#[tokio::test]
async fn write_markdown_stays_inside_the_output_directory() {
    let e = env();
    let t = WriteMarkdownTool;
    let ctx = e.ctx();
    for (path, why) in [
        ("../outside/x", "relative traversal"),
        ("sub/../../outside/x", "nested traversal"),
        (
            &e.outside.join("x.md").display().to_string(),
            "absolute outside",
        ),
        (
            &format!("{}-evil/x.md", e.out.display()),
            "look-alike sibling",
        ),
    ] {
        let err = t.resource(&md(path, "x"), &ctx).unwrap_err();
        assert!(matches!(err, ElfError::NoPermission { .. }), "{why}: {err}");
        assert!(t.execute(md(path, "x"), e.ctx()).await.is_err(), "{why}");
    }
    assert_eq!(std::fs::read_dir(&e.outside).unwrap().count(), 0);
    // absolute path inside the output directory is fine
    let inside = e.out.join("ok.md").display().to_string();
    t.execute(md(&inside, "x"), e.ctx()).await.unwrap();
    assert!(e.out.join("ok.md").exists());
    // wrong extension, oversize content, no output directory
    assert!(matches!(
        t.resource(&md("run.exe", "x"), &ctx),
        Err(ElfError::InvalidInput { .. })
    ));
    let huge = "x".repeat(write_markdown::MAX_CONTENT_BYTES + 1);
    assert!(t.resource(&md("big", &huge), &ctx).is_err());
    let mut none = e.ctx();
    none.scope.output_dirs.clear();
    assert!(matches!(
        t.resource(&md("a", "x"), &none),
        Err(ElfError::NoPermission { .. })
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn write_markdown_does_not_follow_symlinks_out_of_the_output_directory() {
    let e = env();
    let t = WriteMarkdownTool;
    std::os::unix::fs::symlink(&e.outside, e.out.join("escape")).unwrap();
    let err = t
        .execute(md("escape/stolen", "x"), e.ctx())
        .await
        .unwrap_err();
    assert!(matches!(err, ElfError::NoPermission { .. }), "{err}");
    assert_eq!(std::fs::read_dir(&e.outside).unwrap().count(), 0);
    // a dangling symlink squatting on the target name is refused, never followed
    std::os::unix::fs::symlink(e.outside.join("target.md"), e.out.join("note.md")).unwrap();
    let err = t.execute(md("note", "x"), e.ctx()).await.unwrap_err();
    assert!(matches!(err, ElfError::NoPermission { .. }), "{err}");
    assert!(!e.outside.join("target.md").exists());
    // a symlink to an existing file inside the folder just makes the name taken
    std::fs::write(e.out.join("real.md"), "keep").unwrap();
    std::os::unix::fs::symlink(e.out.join("real.md"), e.out.join("alias.md")).unwrap();
    t.execute(md("alias", "x"), e.ctx()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(e.out.join("real.md")).unwrap(),
        "keep"
    );
    assert!(e.out.join("alias (2).md").exists());
}

#[test]
fn write_markdown_preview_is_concrete() {
    let e = env();
    let t = WriteMarkdownTool;
    let args = md("plan", "# Title\nbody");
    let r = t.resource(&args, &e.ctx()).unwrap();
    let p = t.preview(&args, &r);
    assert!(
        p.contains(&e.out.join("plan.md").display().to_string())
            && p.contains("12 bytes")
            && p.contains("# Title"),
        "{p}"
    );
    assert_eq!(
        t.redact_args(&args),
        json!({"path": "plan", "content_bytes": 12})
    );
    let long = json!({"a": "x".repeat(500)});
    assert!(shorten_strings(&long, 10)["a"]
        .as_str()
        .unwrap()
        .ends_with("(+490 chars)"));
}

// ----- Windows-only behaviour (not run on Linux CI) -----------------------------------------------

#[cfg(windows)]
mod windows {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn output_directory_given_as_verbatim_path_accepts_ordinary_spellings_in_any_case() {
        let mut e = env();
        let canon = std::fs::canonicalize(&e.out).unwrap(); // \\?\C:\...
        assert!(canon.to_string_lossy().starts_with(r"\\?\"));
        e.scope.output_dirs = vec![canon.clone()];
        let t = WriteMarkdownTool;
        let plain = crate::policy::paths::strip_verbatim_prefix(&canon);
        let upper = PathBuf::from(plain.to_string_lossy().to_uppercase()).join("Case.md");
        let out = t
            .execute(md(&upper.display().to_string(), "x"), e.ctx())
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&out.content).unwrap();
        // artifact path is stored without the \\?\ prefix
        assert!(!v["path"].as_str().unwrap().starts_with(r"\\?\"), "{v}");
        assert!(
            plain.join("case.md").exists(),
            "file system is case-insensitive"
        );
        // forward slashes work as separators
        let slashed = plain.display().to_string().replace('\\', "/") + "/fwd.md";
        t.execute(md(&slashed, "x"), e.ctx()).await.unwrap();
        // other drives / UNC paths are outside
        assert!(t
            .resource(&md(r"D:\elsewhere\x.md", "x"), &e.ctx())
            .is_err());
        assert!(t
            .resource(&md(r"\\server\share\x.md", "x"), &e.ctx())
            .is_err());
    }

    #[tokio::test]
    async fn reserved_names_and_alternate_streams_cannot_be_created() {
        let e = env();
        let t = WriteMarkdownTool;
        for given in [
            "NUL",
            "com1",
            "LPT1.md",
            "evil:stream",
            "trailing...",
            "CON.txt.md",
        ] {
            let r = t.execute(md(given, "x"), e.ctx()).await;
            let out = r.unwrap_or_else(|err| panic!("{given}: {err}"));
            let v: Value = serde_json::from_str(&out.content).unwrap();
            assert!(Path::new(v["path"].as_str().unwrap()).is_file(), "{given}");
        }
        assert!(
            !e.out.join("evil").exists(),
            "no alternate data stream host file named by the prefix"
        );
    }

    #[tokio::test]
    async fn read_file_matches_granted_directory_case_insensitively() {
        let e = env();
        std::fs::write(e.shared.join("N.txt"), "hi").unwrap();
        let t = ReadFileTool::default();
        let weird = PathBuf::from(e.shared.display().to_string().to_lowercase()).join("n.TXT");
        let args = json!({"path": weird.display().to_string()});
        assert!(matches!(
            e.policy(&t, &args).unwrap(),
            Decision::Allow { .. }
        ));
        let out = t.execute(args, e.ctx()).await.unwrap();
        assert!(out.content.contains("hi"));
        // device names are not regular files
        let dev = json!({"path": e.shared.join("CON").display().to_string()});
        assert!(t.execute(dev, e.ctx()).await.is_err());
    }
}
