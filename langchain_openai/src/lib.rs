use langchain::{LlmModel, NodeRunError};
use langchain_core::{
    request::{RequestBody, ToolSpec},
    response::ResponseBody,
    state::{MessageDiff, MessageState},
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};

pub mod message;
pub mod request;
pub mod response;

pub const CHAT_COMPLETIONS: &str = "/chat/completions";

#[derive(Debug, Clone)]
pub struct ChatOpenAiModel {
    base_url: String,
    api_key: String,
    model: String,
    tools: Vec<ToolSpec>,
}

impl ChatOpenAiModel {
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        Self {
            base_url,
            api_key,
            model,
            tools: vec![],
        }
    }
}

#[async_trait::async_trait]
impl LlmModel for ChatOpenAiModel {
    async fn invoke(&self, state: &MessageState) -> Result<MessageDiff, NodeRunError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                .map_err(|_| NodeRunError::LlmRunError("invalid api key".to_string()))?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let body = RequestBody::from_model(&self.model)
            .with_messages(state.messages.clone())
            .with_tools(self.tools.clone());
        println!(
            "请求Body:\n{}",
            serde_json::to_string_pretty(&body)
                .map_err(|_| NodeRunError::LlmRunError("invalid request body".to_string()))?
        );

        let client = reqwest::Client::new();

        let response = client
            .post(format!("{}{CHAT_COMPLETIONS}", self.base_url))
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|_| NodeRunError::LlmRunError("request failed".to_string()))?
            .json::<ResponseBody>()
            .await
            .map_err(|e| NodeRunError::LlmRunError(format!("request failed: {:?}", e)))?;
        println!("回复Body:\n{:?}", response);

        let message = response
            .choices
            .into_iter()
            .map(|choice| choice.message)
            .collect::<Vec<_>>();

        Ok(MessageDiff {
            new_messages: message,
            llm_calls_delta: 1,
        })
    }

    fn bind_tools(&mut self, tools: &[ToolSpec]) {
        self.tools = tools.to_vec();
    }
}
