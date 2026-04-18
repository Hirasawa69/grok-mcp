//! `POST /rest/app-chat/upload-file` and `GET /rest/assets/<id>` payloads.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::common::FileMetadataId;

/// Body for `POST /rest/app-chat/upload-file`. The `content` field is the
/// raw file bytes encoded as standard base64 (no URL-safe variant).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UploadRequest {
    #[serde(rename = "fileName")]
    pub file_name: String,
    #[serde(rename = "fileMimeType")]
    pub file_mime_type: String,
    pub content: String,
}

/// Response body from `POST /rest/app-chat/upload-file`. The `fileMetadataId`
/// is the attachment handle used later in chat requests.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UploadResponse {
    #[serde(rename = "fileMetadataId")]
    pub file_metadata_id: FileMetadataId,
    #[serde(rename = "fileMimeType")]
    pub file_mime_type: String,
    #[serde(rename = "fileName")]
    pub file_name: String,
    #[serde(rename = "fileUri")]
    pub file_uri: String,
    #[serde(default, rename = "parsedFileUri")]
    pub parsed_file_uri: String,
    #[serde(rename = "createTime")]
    pub create_time: String,
    #[serde(default, rename = "fileSource")]
    pub file_source: String,
}

/// Response body from `GET /rest/assets/<file_metadata_id>`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AssetMetadata {
    #[serde(rename = "assetId")]
    pub asset_id: FileMetadataId,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub name: String,
    #[serde(rename = "sizeBytes")]
    #[schemars(schema_with = "crate::server::schema_helpers::u64_schema")]
    pub size_bytes: u64,
    #[serde(rename = "createTime")]
    pub create_time: String,
    #[serde(default, rename = "lastUseTime")]
    pub last_use_time: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub key: String,
    #[serde(default, rename = "isDeleted")]
    pub is_deleted: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
