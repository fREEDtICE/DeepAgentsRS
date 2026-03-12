use std::sync::Arc;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue};

use crate::llm::common::{parse_sse_json_response, send_openai_compatible_request};
use crate::llm::openai_compatible::{
    MockOpenAiTransport, OpenAiChatRequest, OpenAiChatResponse, OpenAiChunkStream,
    OpenAiCompatibleConfig, OpenAiCompatibleProvider, OpenAiCompatibleTransport,
};
use crate::llm::{
    ChatRequest, ChatResponse, LlmEventStream, LlmProvider, LlmProviderCapabilities,
    MultimodalInputRoles, ToolSpec, ToolsPayload,
};

const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

#[derive(Debug, Clone)]
pub struct OpenRouterConfig {
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub site_url: Option<String>,
    pub app_name: Option<String>,
    pub multimodal_input_roles: MultimodalInputRoles,
}

impl OpenRouterConfig {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            base_url: DEFAULT_OPENROUTER_BASE_URL.to_string(),
            api_key: None,
            site_url: None,
            app_name: None,
            multimodal_input_roles: MultimodalInputRoles::user_only(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn with_site_url(mut self, site_url: impl Into<String>) -> Self {
        self.site_url = Some(site_url.into());
        self
    }

    pub fn with_app_name(mut self, app_name: impl Into<String>) -> Self {
        self.app_name = Some(app_name.into());
        self
    }

    pub fn with_multimodal_input_roles(mut self, roles: MultimodalInputRoles) -> Self {
        self.multimodal_input_roles = roles;
        self
    }

    fn to_openai_compatible_config(&self) -> OpenAiCompatibleConfig {
        let mut config = OpenAiCompatibleConfig::new(self.model.clone())
            .with_base_url(self.base_url.clone())
            .with_multimodal_input_roles(self.multimodal_input_roles);
        if let Some(api_key) = &self.api_key {
            config = config.with_api_key(api_key.clone());
        }
        config
    }
}

pub struct OpenRouterProvider {
    inner: OpenAiCompatibleProvider,
}

impl OpenRouterProvider {
    pub fn new(config: OpenRouterConfig) -> Self {
        let mut transport = ReqwestOpenRouterTransport::new();
        if let Some(site_url) = &config.site_url {
            transport = transport.with_site_url(site_url.clone());
        }
        if let Some(app_name) = &config.app_name {
            transport = transport.with_app_name(app_name.clone());
        }
        Self::with_transport(config, Arc::new(transport))
    }

    pub fn with_transport(
        config: OpenRouterConfig,
        transport: Arc<dyn OpenAiCompatibleTransport>,
    ) -> Self {
        Self {
            inner: OpenAiCompatibleProvider::new(config.to_openai_compatible_config(), transport),
        }
    }
}

#[async_trait]
impl LlmProvider for OpenRouterProvider {
    fn capabilities(&self) -> LlmProviderCapabilities {
        self.inner.capabilities()
    }

    fn convert_tools(&self, tool_specs: &[ToolSpec]) -> anyhow::Result<ToolsPayload> {
        self.inner.convert_tools(tool_specs)
    }

    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatResponse> {
        self.inner.chat(req).await
    }

    async fn stream_chat(&self, req: ChatRequest) -> anyhow::Result<LlmEventStream> {
        self.inner.stream_chat(req).await
    }
}

pub type MockOpenRouterTransport = MockOpenAiTransport;

#[derive(Clone, Default)]
pub struct ReqwestOpenRouterTransport {
    client: reqwest::Client,
    site_url: Option<String>,
    app_name: Option<String>,
}

impl ReqwestOpenRouterTransport {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            site_url: None,
            app_name: None,
        }
    }

    pub fn with_client(client: reqwest::Client) -> Self {
        Self {
            client,
            site_url: None,
            app_name: None,
        }
    }

    pub fn with_site_url(mut self, site_url: impl Into<String>) -> Self {
        self.site_url = Some(site_url.into());
        self
    }

    pub fn with_app_name(mut self, app_name: impl Into<String>) -> Self {
        self.app_name = Some(app_name.into());
        self
    }

    async fn send_request(
        &self,
        config: &OpenAiCompatibleConfig,
        request: OpenAiChatRequest,
        stream: bool,
    ) -> anyhow::Result<reqwest::Response> {
        let mut headers = HeaderMap::new();
        if let Some(site_url) = &self.site_url {
            headers.insert("HTTP-Referer", HeaderValue::from_str(site_url)?);
        }
        if let Some(app_name) = &self.app_name {
            headers.insert("X-Title", HeaderValue::from_str(app_name)?);
        }
        send_openai_compatible_request(
            &self.client,
            &config.base_url,
            config.api_key.as_deref(),
            &request,
            stream,
            headers,
            "openrouter_http_error",
        )
        .await
    }
}

#[async_trait]
impl OpenAiCompatibleTransport for ReqwestOpenRouterTransport {
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
        Ok(Box::pin(parse_sse_json_response(response)))
    }
}
