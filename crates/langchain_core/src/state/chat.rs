use async_trait::async_trait;
use futures_core::Stream;
use im::Vector;
use std::pin::Pin;
use std::sync::Arc;

use crate::{
    message::{Message, ToolCall},
    request::ToolSpec,
    response::Usage,
};

/// LLM 调用选项
#[derive(Debug, Clone, Default)]
pub struct InvokeOptions<'a> {
    /// 可用工具列表
    pub tools: Option<&'a [ToolSpec]>,
    /// 采样温度
    pub temperature: Option<f32>,
    /// 最大生成 token 数
    pub max_tokens: Option<u32>,
    /// 核采样参数
    pub top_p: Option<f32>,
    /// 停止序列
    pub stop: Option<&'a [String]>,
}

#[derive(Debug, Clone, Default)]
pub struct MessagesState {
    pub messages: Vector<Arc<Message>>,
    pub llm_calls: u32,
}

impl MessagesState {
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages: messages.into_iter().map(Arc::new).collect(),
            llm_calls: 0,
        }
    }

    pub fn push_message(&mut self, message: Arc<Message>) {
        self.messages.push_back(message);
    }

    pub fn extend_messages<I>(&mut self, messages: I)
    where
        I: IntoIterator<Item = Arc<Message>>,
    {
        self.messages.extend(messages);
    }

    /// 便捷方法：自动包装为 Arc
    pub fn push_message_owned(&mut self, message: Message) {
        self.messages.push_back(Arc::new(message));
    }

    /// 便捷方法：自动包装为 Arc
    pub fn extend_messages_owned(&mut self, messages: Vec<Message>) {
        self.messages.extend(messages.into_iter().map(Arc::new));
    }

    pub fn increment_llm_calls(&mut self) {
        self.llm_calls = self.llm_calls.saturating_add(1);
    }

    pub fn last_message(&self) -> Option<&Arc<Message>> {
        let len = self.messages.len();
        if len == 0 {
            None
        } else {
            self.messages.get(len - 1)
        }
    }

    pub fn last_assistant(&self) -> Option<&Arc<Message>> {
        self.messages
            .iter()
            .rev()
            .find(|m| matches!(m.as_ref(), Message::Assistant { .. }))
    }

    pub fn last_tool_calls(&self) -> Option<&[ToolCall]> {
        match self.last_assistant() {
            Some(msg) => match msg.as_ref() {
                Message::Assistant {
                    tool_calls: Some(calls),
                    ..
                } => Some(calls.as_slice()),
                _ => None,
            },
            None => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatCompletion {
    pub messages: Vec<Arc<Message>>,
    pub usage: Usage,
}

#[derive(Debug, Clone)]
pub enum ChatStreamEvent {
    Content(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        type_name: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
    Done {
        finish_reason: Option<String>,
        usage: Option<Usage>,
    },
}

pub type ChatStream<E> = Pin<Box<dyn Stream<Item = Result<ChatStreamEvent, E>> + Send>>;

/// 标准的 ChatStream，使用 Box<dyn Error>
pub type StandardChatStream = Pin<
    Box<
        dyn Stream<Item = Result<ChatStreamEvent, Box<dyn std::error::Error + Send + Sync>>> + Send,
    >,
>;

#[async_trait]
pub trait ChatModel: Send + Sync {
    async fn invoke(
        &self,
        messages: &[Arc<Message>],
        options: &InvokeOptions<'_>,
    ) -> Result<ChatCompletion, Box<dyn std::error::Error + Send + Sync>>;

    async fn stream(
        &self,
        messages: &[Arc<Message>],
        options: &InvokeOptions<'_>,
    ) -> Result<StandardChatStream, Box<dyn std::error::Error + Send + Sync>>;
}
