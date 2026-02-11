use std::mem;

use async_trait::async_trait;
use futures::StreamExt;
use langchain_core::{
    message::{FunctionCall, Message, ToolCall},
    request::ToolSpec,
    state::{ChatCompletion, ChatModel, ChatStreamEvent, InvokeOptions, MessagesState},
};
use langgraph::node::{EventSink, Node, NodeContext};

use crate::AgentError;

pub struct LlmNode<M>
where
    M: ChatModel + 'static,
{
    pub model: M,
    pub tools: Vec<ToolSpec>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl<M> LlmNode<M>
where
    M: ChatModel + 'static,
{
    pub fn new(model: M, tools: Vec<ToolSpec>) -> Self {
        Self {
            model,
            tools,
            temperature: None,
            max_tokens: None,
        }
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }
}

#[async_trait]
impl<M> Node<MessagesState, MessagesState, AgentError, ChatStreamEvent> for LlmNode<M>
where
    M: ChatModel + Send + Sync + 'static,
{
    async fn run_sync(
        &self,
        input: &MessagesState,
        context: NodeContext<'_>,
    ) -> Result<MessagesState, AgentError> {
        let messages: Vec<_> = input.messages.iter().cloned().collect();
        let options = InvokeOptions {
            tools: if self.tools.is_empty() {
                None
            } else {
                Some(&self.tools)
            },
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            response_format: context.config.response_format.as_ref(),
            ..Default::default()
        };
        let completion: ChatCompletion = self
            .model
            .invoke(&messages, &options)
            .await
            .map_err(AgentError::Model)?;
        tracing::debug!("LLM completion: {:?}", completion);

        let mut delta = MessagesState::default();
        delta.extend_messages(completion.messages);
        delta.increment_llm_calls();
        Ok(delta)
    }

    async fn run_stream(
        &self,
        input: &MessagesState,
        sink: &mut dyn EventSink<ChatStreamEvent>,
        _context: NodeContext<'_>,
    ) -> Result<MessagesState, AgentError> {
        let messages: Vec<_> = input.messages.iter().cloned().collect();

        let options = InvokeOptions {
            tools: if self.tools.is_empty() {
                None
            } else {
                Some(&self.tools)
            },
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            ..Default::default()
        };

        let mut completion_stream = self
            .model
            .stream(&messages, &options)
            .await
            .map_err(AgentError::Model)?;

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        let mut raw_args = String::new();

        while let Some(event) = completion_stream.next().await {
            let event = event.map_err(AgentError::Model)?;
            sink.emit(event.clone()).await;

            match event {
                ChatStreamEvent::Content(chunk) => {
                    content.push_str(&chunk);
                }
                ChatStreamEvent::ToolCallDelta {
                    index,
                    id,
                    type_name,
                    name,
                    arguments,
                } => {
                    if tool_calls.len() <= index {
                        tool_calls.resize_with(index + 1, || ToolCall {
                            id: String::new(),
                            type_name: String::new(),
                            function: FunctionCall {
                                name: String::new(),
                                arguments: serde_json::Value::Null,
                            },
                        });

                        if index > 0 {
                            let call = &mut tool_calls[index - 1];
                            call.function.arguments =
                                serde_json::Value::String(mem::take(&mut raw_args));
                        }
                    }

                    let call = &mut tool_calls[index];

                    if let Some(id) = id {
                        call.id = id;
                    }
                    if let Some(tn) = type_name {
                        call.type_name = tn;
                    }
                    if let Some(name) = name {
                        call.function.name = name;
                    }
                    if let Some(args) = arguments {
                        raw_args.push_str(&args);
                    }
                }
                ChatStreamEvent::Done { .. } => {}
            }
        }

        let mut delta = MessagesState::default();

        if !content.is_empty() || !tool_calls.is_empty() {
            let assistant = Message::Assistant {
                content,
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    let len = tool_calls.len();
                    tool_calls[len - 1].function.arguments = serde_json::Value::String(raw_args);
                    Some(tool_calls)
                },
                name: None,
            };
            delta.push_message_owned(assistant);
        }

        delta.increment_llm_calls();
        Ok(delta)
    }
}
