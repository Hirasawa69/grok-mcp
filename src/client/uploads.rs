//! Upload-oriented client API.

use std::path::PathBuf;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use bon::Builder;

use crate::{
    client::GrokClient,
    error::{ConfigError, Error, Result},
    models::{UploadRequest, UploadResponse},
};

/// Resource entry point for upload endpoints.
#[derive(Clone, Copy)]
pub struct Uploads<'a>(&'a GrokClient);

/// Data assembled by the generated `UploadFileBuilder`.
#[derive(Builder)]
pub struct UploadFile<'a> {
    #[builder(start_fn)]
    client: &'a GrokClient,
    #[builder(into)]
    file_name: Option<String>,
    #[builder(into)]
    mime_type: Option<String>,
    #[builder(into)]
    content_base64: Option<String>,
    #[builder(into)]
    local_path: Option<PathBuf>,
}

impl<'a> Uploads<'a> {
    pub fn upload(self) -> UploadFileBuilder<'a> {
        UploadFile::builder(self.0)
    }
}

impl<'a, S> UploadFileBuilder<'a, S>
where
    S: upload_file_builder::State,
{
    pub async fn send(self) -> Result<UploadResponse> {
        self.build().send().await
    }
}

impl<'a> UploadFile<'a> {
    async fn send(self) -> Result<UploadResponse> {
        let file_name = self
            .file_name
            .ok_or_else(|| Error::Config(ConfigError::MissingRequired("file_name")))?;
        let mime_type = self
            .mime_type
            .ok_or_else(|| Error::Config(ConfigError::MissingRequired("mime_type")))?;
        let content = match (self.content_base64, self.local_path) {
            (Some(content), None) => content,
            (None, Some(path)) => {
                let bytes = tokio::fs::read(&path).await?;
                BASE64_STANDARD.encode(bytes)
            }
            (Some(_), Some(_)) | (None, None) => {
                return Err(Error::Config(ConfigError::MissingRequired(
                    "content_base64 or local_path",
                )));
            }
        };

        let request = UploadRequest {
            file_name,
            file_mime_type: mime_type,
            content,
        };
        self.client
            .post_json("/rest/app-chat/upload-file", &request)
            .await
    }
}

impl GrokClient {
    #[must_use]
    pub fn uploads(&self) -> Uploads<'_> {
        Uploads(self)
    }
}
