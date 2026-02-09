//! # OpenAI 标准实现
//! 该模块实现了 OpenAI 标准的模型和工具调用，提供了与 OpenAI API 交互的功能。

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use langchain_core::{
    message::Message,
    request::RequestBody,
    response::ResponseBody,
    response::Usage,
    state::{ChatCompletion, ChatModel, ChatStreamEvent, InvokeOptions, StandardChatStream},
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};

use crate::error::OpenAIError;

mod error;

pub const CHAT_COMPLETIONS: &str = "/chat/completions";

pub struct ChatOpenAI {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
    default_temperature: Option<f32>,
    default_max_tokens: Option<u32>,
    default_top_p: Option<f32>,
}

#[async_trait::async_trait]
impl ChatModel for ChatOpenAI {
    async fn invoke(
        &self,
        messages: &[Arc<Message>],
        options: &InvokeOptions<'_>,
    ) -> Result<ChatCompletion, Box<dyn std::error::Error + Send + Sync>> {
        let tools = options.tools.unwrap_or(&[]).to_vec();

        let mut request = RequestBody::from_model(&self.model).with_messages(messages.to_vec());

        // 应用配置选项
        if let Some(temperature) = options.temperature.or(self.default_temperature) {
            request.temperature = Some(temperature);
        }
        if let Some(max_tokens) = options.max_tokens.or(self.default_max_tokens) {
            request.max_tokens = Some(max_tokens);
        }
        if let Some(top_p) = options.top_p.or(self.default_top_p) {
            request.top_p = Some(top_p);
        }
        // OpenAI API 的 stop 参数是 String，我们取第一个或合并
        if let Some(stop) = options.stop
            && stop.is_empty()
        {
            request.stop = Some(stop.join(", "));
        }

        if let Some(format) = options.response_format {
            request.response_format = Some(format.clone());
        }

        if !tools.is_empty() {
            request = request.with_tools(tools);
        }

        if let Some(tool_choice) = &options.tool_choice {
            request.tool_choice = Some(tool_choice.clone());
        }

        tracing::debug!(
            "OpenAI API request: {}",
            serde_json::to_string_pretty(&request).unwrap()
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                .map_err(|e| OpenAIError::InvalidHeaderValue(e.to_string()))?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let response = self
            .client
            .post(format!("{}{CHAT_COMPLETIONS}", self.base_url))
            .headers(headers)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    OpenAIError::Timeout
                } else {
                    OpenAIError::Http(e)
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|e| format!("failed to read error body: {e}"));
            tracing::error!("OpenAI API error: status = {status}, body = {body}");
            return Err(match status.as_u16() {
                401 => {
                    Box::new(OpenAIError::InvalidApiKey) as Box<dyn std::error::Error + Send + Sync>
                }
                404 => {
                    Box::new(OpenAIError::ModelNotFound) as Box<dyn std::error::Error + Send + Sync>
                }
                _ => Box::new(OpenAIError::Other(format!(
                    "status: {status}, body: {body}"
                ))) as Box<dyn std::error::Error + Send + Sync>,
            });
        }

        let response: ResponseBody = response
            .json::<ResponseBody>()
            .await
            .map_err(OpenAIError::ResponseBodyParse)?;

        tracing::debug!("OpenAI API response: {:?}", response);

        let messages = response
            .choices
            .iter()
            .map(|c| Arc::new(c.message.clone()))
            .collect::<Vec<_>>();

        if messages.is_empty() {
            return Err(OpenAIError::Other("no choices in response".to_owned()).into());
        }

        Ok(ChatCompletion {
            messages,
            usage: response.usage,
        })
    }

    async fn stream(
        &self,
        messages: &[Arc<Message>],
        options: &InvokeOptions<'_>,
    ) -> Result<StandardChatStream, Box<dyn std::error::Error + Send + Sync>> {
        let tools = options.tools.unwrap_or(&[]).to_vec();

        let mut request = RequestBody::from_model(&self.model).with_messages(messages.to_vec());

        // 应用配置选项
        if let Some(temperature) = options.temperature.or(self.default_temperature) {
            request.temperature = Some(temperature);
        }
        if let Some(max_tokens) = options.max_tokens.or(self.default_max_tokens) {
            request.max_tokens = Some(max_tokens);
        }
        if let Some(top_p) = options.top_p.or(self.default_top_p) {
            request.top_p = Some(top_p);
        }
        // OpenAI API 的 stop 参数是 String，我们取第一个或合并
        if let Some(stop) = options.stop
            && stop.is_empty()
        {
            request.stop = Some(stop.join(", "));
        }

        if !tools.is_empty() {
            request = request.with_tools(tools);
        }

        request.stream = true;

        tracing::debug!(
            "OpenAI API request: {}",
            serde_json::to_string_pretty(&request).unwrap()
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                .map_err(|e| OpenAIError::InvalidHeaderValue(e.to_string()))?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let response = self
            .client
            .post(format!("{}{CHAT_COMPLETIONS}", self.base_url))
            .headers(headers)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    OpenAIError::Timeout
                } else {
                    OpenAIError::Http(e)
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|e| format!("failed to read error body: {e}"));
            tracing::error!("OpenAI API error: status = {status}, body = {body}");
            return Err(match status.as_u16() {
                401 => {
                    Box::new(OpenAIError::InvalidApiKey) as Box<dyn std::error::Error + Send + Sync>
                }
                404 => {
                    Box::new(OpenAIError::ModelNotFound) as Box<dyn std::error::Error + Send + Sync>
                }
                _ => Box::new(OpenAIError::Other(format!(
                    "status: {status}, body: {body}"
                ))) as Box<dyn std::error::Error + Send + Sync>,
            });
        }

        let stream = async_stream::try_stream! {
            let mut buffer = String::new();
            let mut done_emitted = false;
            let mut bytes_stream = response.bytes_stream();

            while let Some(chunk) = bytes_stream.next().await {
                let chunk = chunk.map_err(OpenAIError::Http)?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some((event, rest)) = split_sse_event(&buffer) {
                    buffer = rest;
                    let data = extract_sse_data(&event);
                    if data.is_empty() {
                        continue;
                    }
                    if data.trim() == "[DONE]" {
                        if !done_emitted {
                            yield ChatStreamEvent::Done { finish_reason: None, usage: None };
                        }
                        return;
                    }

                    let value: serde_json::Value =
                        serde_json::from_str(&data).map_err(|e| OpenAIError::Other(e.to_string()))?;

                    let usage = value
                        .get("usage")
                        .and_then(|u| if u.is_null() { None } else { Some(u) })
                        .and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok());

                    let choices = value
                        .get("choices")
                        .and_then(|c| c.as_array())
                        .cloned()
                        .unwrap_or_default();

                    if choices.is_empty() {
                        if let Some(usage) = usage
                            && !done_emitted
                        {
                            done_emitted = true;
                            yield ChatStreamEvent::Done { finish_reason: None, usage: Some(usage) };
                        }
                        continue;
                    }

                    for choice in choices {
                        let finish_reason = choice
                            .get("finish_reason")
                            .and_then(|r| r.as_str())
                            .map(|s| s.to_owned());

                        if let Some(delta) = choice.get("delta") {
                            if let Some(content) = delta.get("content").and_then(|c| c.as_str())
                                && !content.is_empty()
                            {
                                yield ChatStreamEvent::Content(content.to_owned());
                            }

                            if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                                for call in tool_calls {
                                    let index = call
                                        .get("index")
                                        .and_then(|i| i.as_u64())
                                        .unwrap_or(0) as usize;
                                    let id = call.get("id").and_then(|v| v.as_str()).map(|s| s.to_owned());
                                    let type_name = call.get("type").and_then(|v| v.as_str()).map(|s| s.to_owned());
                                    let function = call.get("function");
                                    let name = function
                                        .and_then(|f| f.get("name"))
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_owned());
                                    let arguments = function
                                        .and_then(|f| f.get("arguments"))
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_owned());

                                    yield ChatStreamEvent::ToolCallDelta {
                                        index,
                                        id,
                                        type_name,
                                        name,
                                        arguments,
                                    };
                                }
                            }
                        }

                        if let Some(finish_reason) = finish_reason
                            && !done_emitted
                        {
                            done_emitted = true;
                            yield ChatStreamEvent::Done { finish_reason: Some(finish_reason), usage: usage.clone() };
                        }
                    }
                }
            }

            if !done_emitted {
                yield ChatStreamEvent::Done { finish_reason: None, usage: None };
            }
        };

        Ok(Box::pin(stream))
    }
}

fn split_sse_event(buffer: &str) -> Option<(String, String)> {
    let idx = buffer.find("\n\n")?;
    let (event, rest) = buffer.split_at(idx);
    let rest = rest.get(2..).unwrap_or("").to_owned();
    Some((event.to_owned(), rest))
}

fn extract_sse_data(event: &str) -> String {
    let mut out = String::new();
    for line in event.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            let data = rest.trim_start();
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(data);
        }
    }
    out
}

pub struct ChatOpenAIBuilder {
    base_url: String,
    model: String,
    api_key: String,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    top_p: Option<f32>,
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
            top_p: None,
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

    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
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
            default_temperature: self.temperature,
            default_max_tokens: self.max_tokens,
            default_top_p: self.top_p,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use langchain_core::message::Message;
    use langchain_core::state::InvokeOptions;
    use std::sync::Arc;

    #[tokio::test]
    #[ignore]
    async fn invoke_with_real_openai() {
        let model = "deepseek-ai/DeepSeek-V3";
        let base_url = "https://api.siliconflow.cn/v1";
        let api_key = "";

        let client = ChatOpenAIBuilder::from_base(model, base_url, api_key).build();
        let messages = vec![Arc::new(Message::user("hello"))];
        let options = InvokeOptions::default();

        let result = client.invoke(&messages, &options).await;

        match result {
            Ok(completion) => {
                println!("{:?}", completion);
                assert!(!completion.messages.is_empty());
            }
            Err(e) => {
                panic!("ChatOpenAI invoke failed: {:?}", e);
            }
        }
    }
}
