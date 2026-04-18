//! JSON-Schema → Anthropic tool-definition translation for MCP tools.
//!
//! MCP tool schemas are JSON Schema Draft 7. Anthropic's tool format
//! accepts a subset of JSON Schema directly in the `input_schema`
//! field, so in practice this translation is a pass-through in
//! nearly all cases. This module exists as the explicit seam for
//! the edge cases we might need to fix up over time (e.g. `$ref`
//! resolution, `oneOf` flattening) without scattering those fixes.

use serde_json::Value;

/// Sanitize an MCP input schema for Anthropic's API.
///
/// Current policy: pass through as-is. Anthropic's tool-use
/// endpoint accepts typical JSON Schema shapes (type, properties,
/// required, enum, nested objects, arrays). We keep this as a
/// function so that if any server's schema turns out to be rejected,
/// we have one place to normalize.
pub fn sanitize_input_schema(schema: Value) -> Value {
    if schema.is_null() {
        return serde_json::json!({ "type": "object", "properties": {} });
    }
    schema
}
