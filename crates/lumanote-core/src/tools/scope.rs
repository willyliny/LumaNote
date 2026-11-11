//! Typed view of `tasks.scope_json`.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::schema::{LumaError, Result};

/// What a task may touch, as declared when it was created.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskScope {
    /// Directories the task may write new files into (absolute).
    pub output_dirs: Vec<PathBuf>,
    /// Directories the task may read from (absolute).
    pub read_dirs: Vec<PathBuf>,
    /// If set, only these tools are offered to the model.
    pub allowed_tools: Option<Vec<String>>,
}

fn dirs(v: &Value, key: &str) -> Result<Vec<PathBuf>> {
    let Some(raw) = v.get(key) else {
        return Ok(Vec::new());
    };
    let arr = raw
        .as_array()
        .ok_or_else(|| LumaError::invalid_input(format!("scope.{key} must be an array")))?;
    arr.iter()
        .map(|d| {
            let s = d.as_str().ok_or_else(|| {
                LumaError::invalid_input(format!("scope.{key} entries must be strings"))
            })?;
            let p = PathBuf::from(s);
            if p.is_absolute() {
                Ok(p)
            } else {
                Err(LumaError::invalid_input(format!(
                    "scope.{key} entry is not absolute: {s}"
                )))
            }
        })
        .collect()
}

impl TaskScope {
    /// Parses `scope_json`; unknown keys are ignored, malformed known keys are errors.
    pub fn from_json(v: &Value) -> Result<Self> {
        let allowed_tools = match v.get("allowed_tools") {
            None | Some(Value::Null) => None,
            Some(a) => Some(
                a.as_array()
                    .ok_or_else(|| LumaError::invalid_input("scope.allowed_tools must be an array"))?
                    .iter()
                    .map(|n| {
                        n.as_str().map(str::to_owned).ok_or_else(|| {
                            LumaError::invalid_input("scope.allowed_tools entries must be strings")
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
        };
        Ok(Self {
            output_dirs: dirs(v, "output_dirs")?,
            read_dirs: dirs(v, "read_dirs")?,
            allowed_tools,
        })
    }

    /// The JSON form stored in `scope_json`.
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "output_dirs": self.output_dirs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "read_dirs": self.read_dirs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        });
        if let (Some(t), Some(o)) = (&self.allowed_tools, v.as_object_mut()) {
            o.insert("allowed_tools".into(), json!(t));
        }
        v
    }

    /// Whether `tool` may be offered under this scope.
    pub fn permits_tool(&self, tool: &str) -> bool {
        self.allowed_tools
            .as_ref()
            .is_none_or(|l| l.iter().any(|t| t == tool))
    }
}
