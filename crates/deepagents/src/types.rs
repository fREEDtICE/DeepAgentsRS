use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    #[serde(alias = "tool_call_id", alias = "call_id", alias = "id")]
    pub id: String,
    #[serde(alias = "tool_name")]
    pub name: String,
    #[serde(default, alias = "args", alias = "input", alias = "arguments")]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageBase64BlockRef<'a> {
    pub mime_type: &'a str,
    pub base64: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageUrlBlockRef<'a> {
    pub url: &'a str,
    pub mime_type: Option<&'a str>,
}

impl ContentBlock {
    pub fn image_base64(mime_type: impl Into<String>, base64: impl Into<String>) -> Self {
        Self {
            block_type: "image_base64".to_string(),
            mime_type: Some(mime_type.into()),
            base64: Some(base64.into()),
            url: None,
        }
    }

    pub fn as_image_base64(&self) -> Option<ImageBase64BlockRef<'_>> {
        if self.block_type != "image_base64" {
            return None;
        }
        Some(ImageBase64BlockRef {
            mime_type: self.mime_type.as_deref()?,
            base64: self.base64.as_deref()?,
        })
    }

    pub fn image_url(url: impl Into<String>) -> Self {
        Self {
            block_type: "image_url".to_string(),
            mime_type: None,
            base64: None,
            url: Some(url.into()),
        }
    }

    pub fn image_url_with_mime(mime_type: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            block_type: "image_url".to_string(),
            mime_type: Some(mime_type.into()),
            base64: None,
            url: Some(url.into()),
        }
    }

    pub fn as_image_url(&self) -> Option<ImageUrlBlockRef<'_>> {
        if self.block_type != "image_url" {
            return None;
        }
        Some(ImageUrlBlockRef {
            url: self.url.as_deref()?,
            mime_type: self.mime_type.as_deref(),
        })
    }

    pub fn fallback_text(&self) -> String {
        if let Some(image) = self.as_image_base64() {
            return format!("(image content: {})", image.mime_type);
        }
        if let Some(image) = self.as_image_url() {
            if let Some(mime_type) = image.mime_type {
                return format!("(image content: {})", mime_type);
            }
            return "(image content)".to_string();
        }
        format!("({} content)", self.block_type)
    }
}

pub fn fallback_text_for_content_blocks(blocks: &[ContentBlock]) -> Option<String> {
    if blocks.is_empty() {
        return None;
    }
    if blocks.len() == 1 {
        return Some(blocks[0].fallback_text());
    }
    Some(format!("({} content blocks)", blocks.len()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_blocks: Option<Vec<ContentBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        alias = "call_id",
        alias = "toolUseId",
        alias = "tool_use_id"
    )]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub output_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_dir: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    pub path: String,
    pub line: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrences: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub exit_code: i32,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}
