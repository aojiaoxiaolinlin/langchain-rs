use async_trait::async_trait;
use langchain_core::state::{ChatStreamEvent, MessagesState};
use langgraph::node::{EventSink, Node, NodeContext};
use std::marker::PhantomData;

pub struct IdentityNode<E> {
    pub _marker: PhantomData<E>,
}

#[async_trait]
impl<E> Node<MessagesState, MessagesState, E, ChatStreamEvent> for IdentityNode<E>
where
    E: Send + Sync + 'static,
{
    async fn run_sync(
        &self,
        _input: &MessagesState,
        _context: NodeContext<'_>,
    ) -> Result<MessagesState, E> {
        Ok(MessagesState::default())
    }

    async fn run_stream(
        &self,
        input: &MessagesState,
        _sink: &dyn EventSink<ChatStreamEvent>,
        context: NodeContext<'_>,
    ) -> Result<MessagesState, E> {
        self.run_sync(input, context).await
    }
}
