use async_stream::try_stream;
use bytes::Bytes;
use reqwest::header::{HeaderMap, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_stream::Stream;
use tokio_stream::StreamExt;

use crate::types::{fallback_text_for_content_blocks, ContentBlock};

pub(crate) fn build_data_url(mime_type: &str, base64: &str) -> String {
    format!("data:{mime_type};base64,{base64}")
}

pub(crate) fn parse_data_url_content_block(url: &str) -> Option<ContentBlock> {
    let payload = url.strip_prefix("data:")?;
    let (meta, base64) = payload.split_once(',')?;
    let mime_type = meta.strip_suffix(";base64")?;
    Some(ContentBlock::image_base64(mime_type, base64))
}

pub(crate) fn parse_image_content_block(url: &str) -> ContentBlock {
    parse_data_url_content_block(url).unwrap_or_else(|| ContentBlock::image_url(url))
}

pub(crate) fn finalize_assistant_text(
    text: String,
    content_blocks: &[ContentBlock],
    saw_multimodal_content: bool,
) -> String {
    if !text.is_empty() {
        return text;
    }
    if let Some(fallback) = fallback_text_for_content_blocks(content_blocks) {
        return fallback;
    }
    if saw_multimodal_content {
        return "(assistant returned multimodal content)".to_string();
    }
    String::new()
}

pub(crate) async fn send_openai_compatible_request<T>(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    request: &T,
    stream: bool,
    extra_headers: HeaderMap,
    error_prefix: &str,
) -> anyhow::Result<reqwest::Response>
where
    T: Serialize + ?Sized,
{
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut builder = client
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .header(
            ACCEPT,
            if stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .headers(extra_headers)
        .json(request);

    if let Some(api_key) = api_key {
        builder = builder.header(AUTHORIZATION, format!("Bearer {api_key}"));
    }

    let response = builder.send().await?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response.text().await.unwrap_or_default();
    Err(anyhow::anyhow!("{error_prefix}: {} {}", status, body))
}

pub(crate) fn parse_sse_json_response<T>(
    response: reqwest::Response,
) -> impl Stream<Item = anyhow::Result<T>> + Send + 'static
where
    T: DeserializeOwned + Send + 'static,
{
    parse_sse_json_bytes_stream(response.bytes_stream())
}

pub(crate) fn parse_sse_json_bytes_stream<S, E, T>(
    stream: S,
) -> impl Stream<Item = anyhow::Result<T>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
    E: std::error::Error + Send + Sync + 'static,
    T: DeserializeOwned + Send + 'static,
{
    try_stream! {
        let mut stream = stream;
        let mut buffer: Vec<u8> = Vec::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(anyhow::Error::from)?;
            buffer.extend_from_slice(&chunk);

            while let Some((idx, delim_len)) = find_sse_frame_delimiter(&buffer) {
                let frame_bytes = buffer[..idx].to_vec();
                buffer.drain(..idx + delim_len);

                let mut frame = String::from_utf8(frame_bytes)?;
                if frame.contains("\r\n") {
                    frame = frame.replace("\r\n", "\n");
                }

                let Some(data) = extract_sse_data(&frame) else {
                    continue;
                };
                if data == "[DONE]" {
                    return;
                }
                yield serde_json::from_str::<T>(&data)?;
            }
        }

        if !buffer.is_empty() {
            let mut frame = String::from_utf8(buffer)?;
            if frame.contains("\r\n") {
                frame = frame.replace("\r\n", "\n");
            }
            if let Some(data) = extract_sse_data(&frame) {
                if data != "[DONE]" {
                    yield serde_json::from_str::<T>(&data)?;
                }
            }
        }
    }
}

fn find_sse_frame_delimiter(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = buf
        .windows(2)
        .position(|w| w == b"\n\n")
        .map(|idx| (idx, 2));
    let crlf = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|idx| (idx, 4));

    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn extract_sse_data(frame: &str) -> Option<String> {
    let mut lines = Vec::new();
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            lines.push(rest.trim_start().to_string());
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}
