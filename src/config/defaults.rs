//! Default chat flags applied to every `grok_research` request unless overridden.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::common::Mode;

/// Per-call defaults applied when the `grok_research` tool omits overrides.
///
/// Field meanings mirror the grok.com web client wire flags one-for-one.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ChatDefaults {
    pub mode: Mode,
    pub disable_search: bool,
    pub force_concise: bool,
    pub disable_memory: bool,
    pub enable_image_generation: bool,
    #[schemars(schema_with = "crate::server::schema_helpers::u32_schema")]
    pub image_generation_count: u32,
    pub enable_side_by_side: bool,
    pub disable_text_follow_ups: bool,
}

impl Default for ChatDefaults {
    fn default() -> Self {
        Self {
            // Matches the `grok_research` hero-tool default, so
            // `grok_rate_limits` without an explicit mode reports the quota
            // the hero tool actually spends.
            mode: Mode::Expert,
            disable_search: false,
            force_concise: false,
            disable_memory: false,
            enable_image_generation: false,
            image_generation_count: 0,
            enable_side_by_side: false,
            disable_text_follow_ups: false,
        }
    }
}
