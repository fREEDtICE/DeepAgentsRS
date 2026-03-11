use std::pin::Pin;

use async_stream::try_stream;
use async_trait::async_trait;
use bytes::Bytes;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use tokio_stream::Stream;
use tokio_stream::StreamExt;

use super::provider::OpenAiCompatibleConfig;
use super::wire::{OpenAiChatChunk, OpenAiChatRequest, OpenAiChatResponse};

pub type OpenAiChunkStream =
    Pin<Box<dyn Stream<Item = anyhow::Result<OpenAiChatChunk>> + Send + 'static>>;

#[async_trait]
pub trait OpenAiCompatibleTransport: Send + Sync {
    async fn create_chat_completion(
        &self,
        config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChatResponse>;

    async fn stream_chat_completion(
        &self,
        config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChunkStream>;
}

#[derive(Clone)]
pub struct MockOpenAiTransport {
    response: Option<OpenAiChatResponse>,
    chunks: Vec<OpenAiChatChunk>,
}

impl MockOpenAiTransport {
    pub fn for_response(response: OpenAiChatResponse) -> Self {
        Self {
            response: Some(response),
            chunks: Vec::new(),
        }
    }

    pub fn for_chunks(chunks: Vec<OpenAiChatChunk>) -> Self {
        Self {
            response: None,
            chunks,
        }
    }
}

#[async_trait]
impl OpenAiCompatibleTransport for MockOpenAiTransport {
    async fn create_chat_completion(
        &self,
        _config: &OpenAiCompatibleConfig,
        _request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChatResponse> {
        self.response
            .clone()
            .ok_or_else(|| anyhow::anyhow!("mock_openai_missing_response"))
    }

    async fn stream_chat_completion(
        &self,
        _config: &OpenAiCompatibleConfig,
        _request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChunkStream> {
        Ok(Box::pin(tokio_stream::iter(
            self.chunks.clone().into_iter().map(Ok::<_, anyhow::Error>),
        )))
    }
}

#[derive(Clone, Default)]
pub struct ReqwestOpenAiTransport {
    client: reqwest::Client,
}

impl ReqwestOpenAiTransport {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }

    async fn send_request(
        &self,
        config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
        stream: bool,
    ) -> anyhow::Result<reqwest::Response> {
        let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
        let mut builder = self
            .client
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
            .json(&request);

        if let Some(api_key) = &config.api_key {
            builder = builder.header(AUTHORIZATION, format!("Bearer {api_key}"));
        }

        let response = builder.send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        let body = response.text().await.unwrap_or_default();
        Err(anyhow::anyhow!("openai_http_error: {} {}", status, body))
    }
}

#[async_trait]
impl OpenAiCompatibleTransport for ReqwestOpenAiTransport {
    async fn create_chat_completion(
        &self,
        config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChatResponse> {
        let response = self.send_request(config, request, false).await?;
        Ok(response.json::<OpenAiChatResponse>().await?)
    }

    async fn stream_chat_completion(
        &self,
        config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
    ) -> anyhow::Result<OpenAiChunkStream> {
        let response = self.send_request(config, request, true).await?;
        Ok(Box::pin(parse_sse_response(response)))
    }
}

fn parse_sse_response(
    response: reqwest::Response,
) -> impl Stream<Item = anyhow::Result<OpenAiChatChunk>> + Send + 'static {
    parse_sse_bytes_stream(response.bytes_stream())
}

fn parse_sse_bytes_stream<S, E>(
    stream: S,
) -> impl Stream<Item = anyhow::Result<OpenAiChatChunk>> + Send + 'static
where
    S: Stream<Item = Result<Bytes, E>> + Send + Unpin + 'static,
    E: std::error::Error + Send + Sync + 'static,
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
                yield serde_json::from_str::<OpenAiChatChunk>(&data)?;
            }
        }

        if !buffer.is_empty() {
            let mut frame = String::from_utf8(buffer)?;
            if frame.contains("\r\n") {
                frame = frame.replace("\r\n", "\n");
            }
            if let Some(data) = extract_sse_data(&frame) {
                if data != "[DONE]" {
                    yield serde_json::from_str::<OpenAiChatChunk>(&data)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::openai_compatible::OpenAiMessageContent;
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn parse_sse_bytes_stream_handles_utf8_split_across_chunks() {
        let json = serde_json::json!({
            "choices": [{
                "delta": { "content": "你" },
                "finish_reason": null
            }]
        });
        let frame = format!("data: {}\n\n", json.to_string());
        let bytes = frame.into_bytes();

        let needle = "你".as_bytes();
        let pos = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("needle present");
        let split = pos + 1;

        let parts: Vec<Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from(bytes[..split].to_vec())),
            Ok(Bytes::from(bytes[split..].to_vec())),
            Ok(Bytes::from_static(b"data: [DONE]\n\n")),
        ];

        let stream = parse_sse_bytes_stream(tokio_stream::iter(parts));
        tokio::pin!(stream);
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            out.push(chunk.unwrap());
        }

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].choices.len(), 1);
        assert_eq!(
            out[0].choices[0].delta.content,
            Some(OpenAiMessageContent::from("你"))
        );
    }
}
