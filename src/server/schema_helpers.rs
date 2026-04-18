//! JSON Schema overrides for `u32` / `u64` fields.
//!
//! schemars 1.x tags Rust unsigned integers with a non-canonical `format`
//! string. Those formats are not in the canonical JSON Schema vocabulary
//! (only `int32` / `int64` are), so strict Zod-based MCP clients like
//! opencode 1.4.7+ log ignorable-but-noisy warnings for every unsigned field
//! on `tools/list`.
//!
//! Fields carry `#[schemars(schema_with = "...::u32_schema")]` (or the `u64`
//! / `Option` variants) so the rendered schema is a plain
//! `{ "type": "integer", "minimum": 0 }` that every validator agrees on.
//! The Rust field types stay `u32` / `u64` — we want the type-level
//! non-negativity guarantee at the source level; only the wire schema
//! changes.

use schemars::{Schema, SchemaGenerator, json_schema};

/// JSON Schema for a required [`u32`] field.
pub fn u32_schema(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "minimum": 0 })
}

/// JSON Schema for a required [`u64`] field.
pub fn u64_schema(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "minimum": 0 })
}

/// JSON Schema for an [`Option<u32>`] field. Accepts `null` in addition to the
/// integer type.
pub fn option_u32_schema(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": ["integer", "null"], "minimum": 0 })
}

/// JSON Schema for an [`Option<u64>`] field. Accepts `null` in addition to the
/// integer type.
pub fn option_u64_schema(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": ["integer", "null"], "minimum": 0 })
}

/// JSON Schema for a typed `Conversation` object in tool outputs.
pub fn conversation_output_schema(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "properties": {
            "conversationId": { "type": "string" },
            "title": { "type": "string" },
            "starred": { "type": "boolean" },
            "createTime": { "type": "string" },
            "modifyTime": { "type": "string" },
            "systemPromptName": { "type": "string" },
            "temporary": { "type": "boolean" }
        },
        "required": [
            "conversationId",
            "title",
            "starred",
            "createTime",
            "modifyTime",
            "systemPromptName",
            "temporary"
        ],
        "additionalProperties": {}
    })
}

/// JSON Schema for an optional `Vec<ResponseNode>` field without bool-valued subschemas.
pub fn option_response_nodes_schema(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": ["array", "null"],
        "items": {
            "type": "object",
            "properties": {
                "responseId": { "type": "string" },
                "sender": { "type": "string" },
                "parentResponseId": { "type": ["string", "null"] }
            },
            "required": ["responseId", "sender"],
            "additionalProperties": {}
        }
    })
}

/// JSON Schema for an optional `Vec<Value>` of loaded response objects.
pub fn option_loaded_responses_schema(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": ["array", "null"],
        "items": {
            "type": "object",
            "properties": {
                "responseId": { "type": "string" },
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "array",
                                "items": { "type": "string" }
                            },
                            "tags": {
                                "type": "array",
                                "items": { "type": "string" }
                            },
                            "rolloutId": { "type": ["string", "null"] },
                            "messageStepId": { "type": ["integer", "null"], "minimum": 0 },
                            "webSearchResults": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "url": { "type": "string" },
                                        "title": { "type": "string" },
                                        "preview": { "type": "string" }
                                    },
                                    "required": ["url", "title", "preview"],
                                    "additionalProperties": {}
                                }
                            },
                            "toolUsageCards": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "toolUsageCardId": { "type": "string" }
                                    },
                                    "required": ["toolUsageCardId"],
                                    "additionalProperties": {}
                                }
                            },
                            "toolUsageResults": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "additionalProperties": {}
                                }
                            }
                        },
                        "required": ["text"],
                        "additionalProperties": {}
                    }
                }
            },
            "required": ["responseId", "steps"],
            "additionalProperties": {}
        }
    })
}
