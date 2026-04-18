//! `POST /rest/skills` payload.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SkillsResponse {
    pub skills: Vec<Skill>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Skill {
    #[schemars(schema_with = "crate::server::schema_helpers::u32_schema")]
    pub index: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default, rename = "displayName")]
    pub display_name: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
