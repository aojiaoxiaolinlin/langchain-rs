use async_trait::async_trait;
use futures_core::Stream;
use std::pin::Pin;

use crate::{
    message::{Message, ToolCall},
    response::Usage,
};

#[derive(Debug, Clone, Default)]
pub struct MessagesState {
    pub messages: Vec<Message>,
    pub llm_calls: u32,
}

impl MessagesState {
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            llm_calls: 0,
        }
    }

    pub fn push_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn extend_messages(&mut self, messages: Vec<Message>) {
        self.messages.extend(messages);
    }

    pub fn increment_llm_calls(&mut self) {
        self.llm_calls = self.llm_calls.saturating_add(1);
    }

    pub fn last_message(&self) -> Option<&Message> {
        self.messages.last()
    }

    pub fn last_assistant(&self) -> Option<&Message> {
        self.messages
            .iter()
            .rev()
            .find(|m| matches!(m, Message::Assistant { .. }))
    }

    pub fn last_tool_calls(&self) -> Option<&[ToolCall]> {
        match self.last_assistant() {
            Some(Message::Assistant {
                tool_calls: Some(calls),
                ..
            }) => Some(calls.as_slice()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatCompletion {
    pub messages: Vec<Message>,
    pub usage: Usage,
}

pub type ChatStream<E> = Pin<Box<dyn Stream<Item = Result<Message, E>> + Send>>;

#[async_trait]
pub trait ChatModel: Send + Sync {
    type Error: Send + Sync + 'static;

    async fn invoke(&self, messages: &[Message]) -> Result<ChatCompletion, Self::Error>;

    async fn stream(&self, messages: &[Message]) -> Result<ChatStream<Self::Error>, Self::Error>;
}
