//! Validation of model-supplied tool arguments against the tool's JSON Schema.
//!
//! Uses the maintained `jsonschema` crate (Draft 2020-12, formats asserted, no network or
//! file `$ref` resolution because the HTTP/file features are compiled out).

use jsonschema::{Draft, Validator};
use serde_json::Value;

use crate::schema::{LumaError, Result};

/// A compiled argument schema.
pub struct ArgsValidator {
    validator: Validator,
}

impl std::fmt::Debug for ArgsValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArgsValidator")
    }
}

/// Most error messages reported back to the model for one call.
const MAX_REPORTED_ERRORS: usize = 8;

impl ArgsValidator {
    /// Compiles `schema`, which must describe a JSON object.
    pub fn compile(schema: &Value) -> Result<Self> {
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(LumaError::invalid_input(
                "a tool parameter schema must have \"type\": \"object\"",
            ));
        }
        let validator = jsonschema::options()
            .with_draft(Draft::Draft202012)
            .should_validate_formats(true)
            .build(schema)
            .map_err(|e| LumaError::invalid_input(format!("invalid tool schema: {e}")))?;
        Ok(Self { validator })
    }

    /// Checks `args`; on failure returns human-readable problems (path and reason).
    pub fn check(&self, args: &Value) -> std::result::Result<(), Vec<String>> {
        let errors: Vec<String> = self
            .validator
            .iter_errors(args)
            .take(MAX_REPORTED_ERRORS)
            .map(|e| {
                let path = e.instance_path().to_string();
                if path.is_empty() {
                    e.to_string()
                } else {
                    format!("at {path}: {e}")
                }
            })
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v() -> ArgsValidator {
        ArgsValidator::compile(&json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["path", "mode"],
            "properties": {
                "path": {"type": "string", "minLength": 1},
                "mode": {"enum": ["read", "list"]},
                "limit": {"type": "integer", "minimum": 1, "maximum": 10},
                "when": {"type": "string", "format": "date-time"}
            }
        }))
        .unwrap()
    }

    #[test]
    fn accepts_valid_and_reports_every_kind_of_violation() {
        let v = v();
        assert!(v
            .check(&json!({"path": "a", "mode": "read", "limit": 3}))
            .is_ok());
        assert!(v
            .check(&json!({"path": "a", "mode": "read", "when": "2026-11-12T09:00:00+08:00"}))
            .is_ok());
        let bad = [
            json!({"mode": "read"}),                                  // missing required
            json!({"path": 5, "mode": "read"}),                       // wrong type
            json!({"path": "a", "mode": "write"}),                    // enum
            json!({"path": "a", "mode": "read", "extra": true}),      // additionalProperties
            json!({"path": "a", "mode": "read", "limit": 11}),        // maximum
            json!({"path": "a", "mode": "read", "limit": 1.5}),       // integer
            json!({"path": "", "mode": "read"}),                      // minLength
            json!({"path": "a", "mode": "read", "when": "tomorrow"}), // format
            json!([1, 2]),                                            // not an object
            json!("x"),
        ];
        for b in bad {
            let errs = v.check(&b).unwrap_err();
            assert!(!errs.is_empty(), "{b}");
        }
    }

    #[test]
    fn rejects_unusable_schemas() {
        assert!(ArgsValidator::compile(&json!({"type": "string"})).is_err());
        assert!(ArgsValidator::compile(&json!({"type": "object", "properties": 3})).is_err());
        // remote references are not resolved (feature compiled out)
        assert!(ArgsValidator::compile(
            &json!({"type": "object", "properties": {"a": {"$ref": "https://example.com/s.json"}}})
        )
        .is_err());
    }
}
