//! #OpenAI 标准实现
//! 该模块实现了 OpenAI 标准的模型和工具调用，提供了与 OpenAI API 交互的功能。

use std::time::Duration;

use langchain_core::{
    message::Message,
    request::RequestBody,
    response::ResponseBody,
    state::{ChatCompletion, ChatModel, ChatStream},
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};

use crate::error::OpenAIError;

mod error;

pub struct ChatOpenAI {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
    #[expect(unused)]
    temperature: Option<f32>,
    #[expect(unused)]
    max_tokens: Option<usize>,
    #[expect(unused)]
    timeout: Option<Duration>,
}

#[async_trait::async_trait]
impl ChatModel for ChatOpenAI {
    type Error = OpenAIError;

    async fn invoke(&self, messages: &[Message]) -> Result<ChatCompletion, Self::Error> {
        let request = RequestBody::from_model(&self.model).with_messages(messages.to_vec());

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                .map_err(|e| OpenAIError::InvalidHeaderValue(e.to_string()))?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let response = self
            .client
            .post(&self.base_url)
            .headers(headers)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    OpenAIError::Timeout
                } else {
                    OpenAIError::HTTP(e)
                }
            })?;

        let response = response.error_for_status().map_err(|e| {
            tracing::error!("OpenAI API error: {:?}", e);
            if let Some(status) = e.status() {
                match status.as_u16() {
                    401 => OpenAIError::InvalidApiKey,
                    404 => OpenAIError::ModelNotFound,
                    _ => OpenAIError::Other(e.to_string()),
                }
            } else if e.is_timeout() {
                OpenAIError::Timeout
            } else {
                OpenAIError::HTTP(e)
            }
        })?;

        let response: ResponseBody = response
            .json::<ResponseBody>()
            .await
            .map_err(OpenAIError::ResponseBodyParse)?;

        let messages = response
            .choices
            .iter()
            .map(|c| c.message.clone())
            .collect::<Vec<_>>();

        if messages.is_empty() {
            return Err(OpenAIError::Other("no choices in response".to_string()));
        }

        Ok(ChatCompletion {
            messages,
            usage: response.usage,
        })
    }

    async fn stream(&self, _messages: &[Message]) -> Result<ChatStream<Self::Error>, Self::Error> {
        unimplemented!("stream is not implemented yet")
    }
}

pub struct ChatOpenAIBuilder {
    base_url: String,
    model: String,
    api_key: String,
    temperature: Option<f32>,
    max_tokens: Option<usize>,
    timeout: Option<Duration>,
}

impl ChatOpenAIBuilder {
    pub fn from_base<T: Into<String>>(model: T, base_url: T, api_key: T) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            api_key: api_key.into(),
            temperature: None,
            max_tokens: None,
            timeout: None,
        }
    }

    pub fn base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }
    pub fn model(mut self, model: String) -> Self {
        self.model = model;
        self
    }
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }
    pub fn max_tokens(mut self, max_tokens: usize) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
    pub fn build(self) -> ChatOpenAI {
        let timeout = self.timeout.unwrap_or_else(|| Duration::from_secs(600));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("failed to build reqwest client");
        ChatOpenAI {
            client,
            base_url: self.base_url,
            model: self.model,
            api_key: self.api_key,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            timeout: Some(timeout),
        }
    }
}
