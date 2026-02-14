use std::sync::Arc;

use futures::future::BoxFuture;
use langchain_core::state::{ChatStreamEvent, MessagesState};
use langgraph::{
    label::InternedGraphLabel,
    node::{Node, NodeContext},
};
use smallvec::SmallVec;

use crate::AgentError;

pub type MiddlewareHandler<S> =
    Arc<dyn Fn(&S, &NodeContext) -> BoxFuture<'static, Result<S, AgentError>> + Send + Sync>;

#[derive(Clone)]
pub struct AgentMiddleware<S: Default> {
    /// 中间件标签
    pub label: MiddlewareLabel,
    /// 代理启动前（每个调用只执行一次）
    pub before_agent: Option<AgentHook<S>>,
    /// 每次模型调用前执行
    pub before_model: Option<AgentHook<S>>,
    /// 每次模型调用后执行
    pub after_model: Option<AgentHook<S>>,
    /// 每次代理完成（每个调用一次）
    pub after_agent: Option<AgentHook<S>>,
}

impl<S: Default> AgentMiddleware<S> {
    pub fn from_label(label: MiddlewareLabel) -> Self {
        Self {
            label,
            before_agent: None,
            before_model: None,
            after_model: None,
            after_agent: None,
        }
    }

    pub fn with_before_agent(mut self, before_agent_hook: AgentHook<S>) -> Self {
        self.before_agent = Some(before_agent_hook);
        self
    }

    pub fn with_before_model(mut self, handler: AgentHook<S>) -> Self {
        self.before_model = Some(handler);
        self
    }

    pub fn with_after_model(mut self, handler: AgentHook<S>) -> Self {
        self.after_model = Some(handler);
        self
    }

    pub fn with_after_agent(mut self, handler: AgentHook<S>) -> Self {
        self.after_agent = Some(handler);
        self
    }
}

#[derive(Clone, Copy)]
pub struct MiddlewareLabel {
    pub before_agent: InternedGraphLabel,
    pub before_model: InternedGraphLabel,
    pub after_model: InternedGraphLabel,
    pub after_agent: InternedGraphLabel,
}

#[derive(Clone)]
pub struct AgentHook<S: Default> {
    pub handler: MiddlewareHandler<S>,
    pub target: Option<InternedGraphLabel>,
    pub branches: SmallVec<[InternedGraphLabel; 2]>,
}

#[macro_export]
macro_rules! define_middleware_label {
    ($name:ident) => {{
        use langchain::node::middleware::MiddlewareLabel;
        use langgraph::label::GraphLabel;

        #[derive(Debug, Clone, PartialEq, Eq, Hash, GraphLabel)]
        pub enum $name {
            BeforeAgent,
            BeforeModel,
            AfterModel,
            AfterAgent,
        }

        let middleware_label = MiddlewareLabel {
            before_agent: $name::BeforeAgent.intern(),
            before_model: $name::BeforeModel.intern(),
            after_model: $name::AfterModel.intern(),
            after_agent: $name::AfterAgent.intern(),
        };

        middleware_label
    }};
}

pub struct AgentMiddlewareNode {
    pub inner: MiddlewareHandler<MessagesState>,
}

impl AgentMiddlewareNode {
    pub fn new(inner: MiddlewareHandler<MessagesState>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl Node<MessagesState, MessagesState, AgentError, ChatStreamEvent> for AgentMiddlewareNode {
    async fn run_sync(
        &self,
        input: &MessagesState,
        context: NodeContext<'_>,
    ) -> Result<MessagesState, AgentError> {
        (self.inner)(input, &context).await
    }

    async fn run_stream(
        &self,
        input: &MessagesState,
        _sink: &dyn langgraph::node::EventSink<ChatStreamEvent>,
        context: langgraph::node::NodeContext<'_>,
    ) -> Result<MessagesState, AgentError> {
        self.run_sync(input, context).await
    }
}
