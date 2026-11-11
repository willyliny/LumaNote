//! `write_markdown`: writes a new `.md` file under the task's output directory.
//!
//! Never overwrites (a clashing name gets ` (2)`, ` (3)`, ...), reads the file back to
//! verify it, records an artifact row (hash and size) and returns it as evidence.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::fsutil::{
    create_unique_file, floor_char_boundary, looks_absolute, numbered_name, sanitize_relative_md,
};
use super::{Tool, ToolContext, ToolOutput};
use crate::policy::paths::{display_path, path_within, relative_below, resolve_within};
use crate::policy::{Action, Resource};
use crate::schema::{sha256_hex, ElfError, EvidenceRef, Result, Trust};
use crate::task::SideEffect;

/// Largest document accepted.
pub const MAX_CONTENT_BYTES: usize = 1024 * 1024;

/// The `write_markdown` tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct WriteMarkdownTool;

/// Where the file will go, before a free name is picked.
struct Plan {
    out_dir: PathBuf,
    /// Sanitised path below `out_dir`; the last element is the `.md` file name.
    rel: Vec<String>,
}

impl Plan {
    fn requested(&self) -> PathBuf {
        self.rel.iter().fold(self.out_dir.clone(), |p, c| p.join(c))
    }
}

fn plan(args: &Value, ctx: &ToolContext) -> Result<Plan> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| ElfError::invalid_input("path is required"))?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| ElfError::invalid_input("content is required"))?;
    if content.len() > MAX_CONTENT_BYTES {
        return Err(ElfError::invalid_input(format!(
            "content is {} bytes; the limit is {MAX_CONTENT_BYTES}",
            content.len()
        )));
    }
    let outside = |why: &str| ElfError::NoPermission {
        action: "write".into(),
        resource: format!("{raw} ({why})"),
    };
    let Some(first) = ctx.scope.output_dirs.first() else {
        return Err(outside("this task has no output directory"));
    };
    let (out_dir, rel_raw) = if looks_absolute(raw) {
        let p = Path::new(raw);
        let hit = ctx
            .scope
            .output_dirs
            .iter()
            .find_map(|d| relative_below(p, d).map(|r| (d.clone(), r)));
        match hit {
            Some((d, rel)) => (d, rel.join("/")),
            None => {
                return Err(outside(
                    "write_markdown only writes inside the output directory",
                ))
            }
        }
    } else {
        (first.clone(), raw.to_owned())
    };
    let rel = sanitize_relative_md(&rel_raw)?;
    let plan = Plan { out_dir, rel };
    // symlinks inside the output directory must not lead out of it
    resolve_within(&plan.out_dir, &plan.requested())?;
    Ok(plan)
}

/// `a (3).md` and `a.md` are the same logical name.
fn same_logical_name(existing: &str, requested: &str) -> bool {
    (1..=50).any(|n| numbered_name(requested, n) == existing)
}

#[async_trait]
impl Tool for WriteMarkdownTool {
    fn name(&self) -> &str {
        "write_markdown"
    }

    fn description(&self) -> &str {
        "Create a new Markdown (.md) document in the task's output folder. `path` is a file name \
         (optionally with sub-folders) relative to that folder. Existing files are never \
         overwritten: if the name is taken, a numbered name is used and returned. The result \
         names the file actually written."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["path", "content"],
            "properties": {
                "path": {"type": "string", "minLength": 1, "maxLength": 512},
                "content": {"type": "string"}
            }
        })
    }

    fn required_action(&self) -> Action {
        Action::Write
    }

    fn side_effect(&self) -> SideEffect {
        SideEffect::LocalWrite
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(30)
    }

    fn max_output_bytes(&self) -> usize {
        8 * 1024
    }

    fn resource(&self, args: &Value, ctx: &ToolContext) -> Result<Resource> {
        Ok(Resource::Path(plan(args, ctx)?.requested()))
    }

    fn preview(&self, args: &Value, resource: &Resource) -> String {
        let content = args.get("content").and_then(Value::as_str).unwrap_or("");
        let head = &content[..floor_char_boundary(content, 120)];
        let target = match resource {
            Resource::Path(p) => p.display().to_string(),
            Resource::Other(s) => s.clone(),
        };
        format!(
            "Create the new Markdown file {target} ({} bytes). An existing file is never overwritten. Starts with: {head:?}",
            content.len()
        )
    }

    fn redact_args(&self, args: &Value) -> Value {
        // keep the target readable, store only the size of the document
        json!({
            "path": args.get("path"),
            "content_bytes": args.get("content").and_then(Value::as_str).map(str::len),
        })
    }

    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let plan = plan(&args, &ctx)?;
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| ElfError::invalid_input("content is required"))?;
        let bytes = content.as_bytes();
        let hash = sha256_hex(bytes);
        let requested = plan.requested();
        let (Some(parent), Some(file_name)) = (
            requested.parent().map(Path::to_path_buf),
            plan.rel.last().cloned(),
        ) else {
            return Err(ElfError::invalid_input("path has no file name"));
        };

        // A re-run after a crash between "file written" and "step finished" must not
        // leave a second copy: reuse an artifact of this task with the same content.
        let existing = ctx.store.with(|s| s.list_artifacts(&ctx.task_id))?;
        let reuse = existing.into_iter().find(|a| {
            let p = Path::new(&a.path);
            a.content_hash == hash
                && p.parent()
                    .is_some_and(|d| path_within(d, &parent) && path_within(&parent, d))
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| same_logical_name(n, &file_name))
                && std::fs::read(p).is_ok_and(|b| sha256_hex(&b) == hash)
        });
        if let Some(a) = reuse {
            let content = json!({
                "artifact_id": a.id, "path": a.path, "bytes": a.bytes, "sha256": a.content_hash,
                "renamed": false, "reused_existing": true,
            });
            return Ok(ToolOutput {
                content: content.to_string(),
                trust: Trust::UserProvided, // app-generated confirmation, no third-party content
                evidence: Some(EvidenceRef::Artifact { id: a.id }),
            });
        }

        std::fs::create_dir_all(&parent).map_err(|e| ElfError::ExternalFailure {
            message: format!("cannot create folder {}: {e}", parent.display()),
            retryable: false,
        })?;
        // re-check after creating folders: the final parent must still be inside
        let real_parent = resolve_within(&plan.out_dir, &parent)?;
        let written = create_unique_file(&real_parent, &file_name, bytes)?;

        // verify by reading back before claiming anything
        let verify = std::fs::read(&written)
            .map_err(|e| format!("cannot read back: {e}"))
            .and_then(|back| {
                if back.len() == bytes.len() && sha256_hex(&back) == hash {
                    Ok(())
                } else {
                    Err("content differs from what was written".to_owned())
                }
            });
        if let Err(why) = verify {
            let _ = std::fs::remove_file(&written); // the file this call just created
            return Err(ElfError::ExternalFailure {
                message: format!("verification of {} failed: {why}", written.display()),
                retryable: true,
            });
        }

        let shown = display_path(&written);
        let now = ctx.now();
        let artifact = ctx.store.with(|s| {
            s.add_artifact(
                &ctx.task_id,
                "markdown",
                &shown.to_string_lossy(),
                &hash,
                bytes.len() as u64,
                now,
            )
        })?;
        let renamed = written.file_name().and_then(|n| n.to_str()) != Some(file_name.as_str());
        let content = json!({
            "artifact_id": artifact.id,
            "path": artifact.path,
            "bytes": artifact.bytes,
            "sha256": artifact.content_hash,
            "renamed": renamed,
            "verified": true,
        });
        Ok(ToolOutput {
            content: content.to_string(),
            trust: Trust::UserProvided, // app-generated confirmation, no third-party content
            evidence: Some(EvidenceRef::Artifact { id: artifact.id }),
        })
    }
}
