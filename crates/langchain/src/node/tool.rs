use std::error::Error;
use std::pin::Pin;
use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use futures::Future;
use futures::future::join_all;
use langchain_core::{
    message::Message,
    state::{ChatStreamEvent, MessagesState, ToolFn, ToolFuture},
};
use langgraph::node::{EventSink, Node, NodeContext};
use serde_json::Value;

use crate::AgentError;

pub type ToolHandler<E> = Box<dyn FnOnce(Value) -> ToolFuture<E> + Send + 'static>;

pub type ToolMiddleware<E> = Box<
    dyn Fn(&MessagesState, &NodeContext, &str, Value, ToolHandler<E>) -> ToolFuture<E>
        + Send
        + Sync,
>;

pub struct ToolNode<E>
where
    E: Send + Sync + 'static,
{
    pub middleware: Option<Arc<ToolMiddleware<E>>>,
    pub tools: HashMap<String, Arc<ToolFn<E>>>,
}

impl<E> ToolNode<E>
where
    E: Send + Sync + 'static,
{
    pub fn new(tools: HashMap<String, Arc<ToolFn<E>>>) -> Self {
        Self {
            tools,
            middleware: None,
        }
    }

    pub fn wrap_tool<F>(mut self, f: F) -> Self
    where
        F: Fn(&MessagesState, &NodeContext, &str, Value, ToolHandler<E>) -> ToolFuture<E>
            + Send
            + Sync
            + 'static,
    {
        self.middleware = Some(Arc::new(Box::new(f)));
        self
    }
}

#[async_trait]
impl<E> Node<MessagesState, MessagesState, AgentError, ChatStreamEvent> for ToolNode<E>
where
    E: Error + Send + Sync + 'static,
{
    async fn run_sync(
        &self,
        input: &MessagesState,
        context: NodeContext<'_>,
    ) -> Result<MessagesState, AgentError> {
        let mut delta = MessagesState::default();
        if let Some(calls) = input.last_tool_calls() {
            let mut futures = Vec::new();
            let mut ids = Vec::new();
            tracing::debug!("Tool calls count: {}", calls.len());
            for call in calls {
                if let Some(handler) = self.tools.get(call.function_name()) {
                    ids.push(call.id().to_owned());
                    tracing::debug!("Tool call: {:?}", call.function);

                    let fut: Pin<Box<dyn Future<Output = String> + Send>> = match call.arguments() {
                        Ok(args) => {
                            let handler = handler.clone();
                            let fut = if let Some(middleware) = &self.middleware {
                                let handler: ToolHandler<E> = Box::new(move |args| (handler)(args));
                                (middleware)(input, &context, call.function_name(), args, handler)
                            } else {
                                (handler)(args)
                            };

                            Box::pin(async move {
                                match fut.await {
                                    Ok(value) => {
                                        tracing::debug!("Tool call result: {}", value);
                                        value.to_string()
                                    }
                                    Err(e) => {
                                        tracing::error!("Tool call failed: {}", e);
                                        format!("Error: {}", e)
                                    }
                                }
                            })
                        }
                        Err(e) => {
                            let msg = format!("Error: Failed to parse arguments: {}", e);
                            tracing::error!("{}", msg);
                            Box::pin(async move { msg })
                        }
                    };

                    futures.push(fut);
                }
            }
            let results = join_all(futures).await;
            for (id, content) in ids.into_iter().zip(results.into_iter()) {
                delta.push_message_owned(Message::tool(content, id));
            }
        }
        Ok(delta)
    }

    async fn run_stream(
        &self,
        input: &MessagesState,
        _sink: &dyn EventSink<ChatStreamEvent>,
        context: NodeContext<'_>,
    ) -> Result<MessagesState, AgentError> {
        self.run_sync(input, context).await
    }
}
