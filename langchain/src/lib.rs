use futures::future::join_all;
use langchain_core::{
    message::Message,
    request::ToolSpec,
    state::{MessageDiff, MessageState},
};
use langgraph::{
    graph::StateGraph,
    graph_runner::{AgentGraphRunner, GraphRunnerError},
    node::{BaseAgentLabel, GraphLabel, InternedGraphLabel, Node},
};
use std::sync::Arc;

pub use langchain_macros::{tool, tools_from_fns};
pub use langgraph::node::NodeRunError;

#[async_trait::async_trait]
pub trait LlmModel: Clone + Send + Sync {
    async fn invoke(&self, state: &MessageState) -> Result<MessageDiff, NodeRunError>;

    fn bind_tools(&mut self, tools: &[ToolSpec]);
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    type Output: serde::Serialize;
    fn spec(&self) -> ToolSpec;
    async fn invoke(
        &self,
        state: &MessageState,
        args: serde_json::Value,
    ) -> Result<Self::Output, NodeRunError>;
}

pub type DynTool = Arc<dyn Tool<Output = serde_json::Value> + Send + Sync>;

pub trait IntoDynTool {
    fn into_dyn_tool(self) -> DynTool;
}

impl<T> IntoDynTool for T
where
    T: Tool<Output = serde_json::Value> + Send + Sync + 'static,
{
    fn into_dyn_tool(self) -> DynTool {
        Arc::new(self)
    }
}

impl IntoDynTool for DynTool {
    fn into_dyn_tool(self) -> DynTool {
        self
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, GraphLabel)]
pub enum AgentLabel {
    CallModel,
    ToolExecutor,
}

pub struct LlmNode<M> {
    pub model: M,
}

#[async_trait::async_trait]
impl<M> Node<MessageState> for LlmNode<M>
where
    M: LlmModel + Send + Sync + 'static,
{
    async fn run(&self, state: &MessageState) -> Result<MessageDiff, NodeRunError> {
        self.model.invoke(state).await
    }
}

pub struct ToolNode {
    tools: Vec<DynTool>,
}

#[async_trait::async_trait]
impl Node<MessageState> for ToolNode {
    async fn run(&self, state: &MessageState) -> Result<MessageDiff, NodeRunError> {
        if let Some(last_message) = state.messages.last() {
            if let Message::Assistant { tool_calls, .. } = last_message
                && tool_calls.is_some()
            {
                let tool_calls = tool_calls.as_ref().unwrap();

                let mut futures = Vec::new();

                println!("工具调用个数: {}", tool_calls.len());

                for call in tool_calls {
                    let tool_name = call.function_name().to_string();
                    let args = call.arguments();
                    let id = call.id().to_string();

                    let tool = self
                        .tools
                        .iter()
                        .find(|t| t.spec().function_name() == tool_name)
                        .ok_or_else(|| {
                            NodeRunError::ToolRunError(format!("tool {} not found", tool_name))
                        })?
                        .clone();

                    let state_ref = state;
                    let fut = async move {
                        let content = tool.invoke(state_ref, args).await?;
                        Ok::<Message, NodeRunError>(Message::tool(content.to_string(), id))
                    };

                    futures.push(fut);
                }

                let results = join_all(futures).await;
                let mut new_messages = Vec::new();

                for res in results {
                    let msg = res?;
                    new_messages.push(msg);
                }

                return Ok(MessageDiff {
                    new_messages,
                    llm_calls_delta: 0,
                });
            }
        }

        Err(NodeRunError::ToolRunError("no tool call".to_string()))
    }
}

// TODO: 实现反思节点
pub struct ReflectNode;

#[async_trait::async_trait]
impl Node<MessageState> for ReflectNode {
    async fn run(&self, _: &MessageState) -> Result<MessageDiff, NodeRunError> {
        Ok(MessageDiff {
            new_messages: vec![],
            llm_calls_delta: 0,
        })
    }
}

pub struct EndNode;

#[async_trait::async_trait]
impl Node<MessageState> for EndNode {
    async fn run(&self, _: &MessageState) -> Result<MessageDiff, NodeRunError> {
        Ok(MessageDiff {
            new_messages: Vec::new(),
            llm_calls_delta: 0,
        })
    }
}

fn route(state: &MessageState) -> InternedGraphLabel {
    if let Some(last_message) = state.messages.last() {
        if let Message::Assistant { tool_calls, .. } = last_message
            && tool_calls.is_some()
        {
            return AgentLabel::ToolExecutor.intern();
        } else {
            return BaseAgentLabel::End.intern();
        };
    }
    BaseAgentLabel::End.intern()
}

async fn run_message_agent<M>(
    initial: MessageState,
    model: M,
    tools: Vec<DynTool>,
) -> Result<MessageState, GraphRunnerError>
where
    M: LlmModel + Clone + Send + Sync + 'static,
{
    let mut graph = StateGraph::<MessageState>::default();

    graph.add_node(AgentLabel::CallModel, LlmNode { model });
    graph.add_node(AgentLabel::ToolExecutor, ToolNode { tools });
    graph.add_node(BaseAgentLabel::End, EndNode);

    graph.set_start(BaseAgentLabel::Start);
    graph.set_end(BaseAgentLabel::End);

    graph.add_node_edge(BaseAgentLabel::Start, AgentLabel::CallModel);
    graph.add_condition_edge(AgentLabel::CallModel, route);
    graph.add_node_edge(AgentLabel::ToolExecutor, AgentLabel::CallModel);

    AgentGraphRunner::run(&graph, initial).await
}

pub struct ReactAgent<M> {
    model: M,
    tools: Vec<DynTool>,
    system_prompt: Option<String>,
}

impl<M> ReactAgent<M>
where
    M: LlmModel + Clone + Send + Sync + 'static,
{
    pub fn create_agent(model: M) -> Self {
        Self {
            model,
            tools: vec![],
            system_prompt: None,
        }
    }

    pub fn create_agent_with_tools<T, I>(model: M, tools: I) -> Self
    where
        T: IntoDynTool,
        I: IntoIterator<Item = T>,
    {
        let tools_vec = tools.into_iter().map(|t| t.into_dyn_tool()).collect();
        Self {
            model,
            tools: tools_vec,
            system_prompt: None,
        }
    }

    pub fn with_tools<T, I>(mut self, tools: I) -> Self
    where
        T: IntoDynTool,
        I: IntoIterator<Item = T>,
    {
        self.tools = tools.into_iter().map(|t| t.into_dyn_tool()).collect();
        self
    }

    pub fn with_tool<T>(mut self, tool: T) -> Self
    where
        T: IntoDynTool,
    {
        self.tools.push(tool.into_dyn_tool());
        self
    }

    pub fn with_system_prompt(mut self, system_prompt: String) -> Self {
        self.system_prompt = Some(system_prompt);
        self
    }

    pub async fn invoke(&mut self, message: Message) -> Result<MessageState, GraphRunnerError> {
        let initial = MessageState::new(vec![message]);
        let specs: Vec<ToolSpec> = self.tools.iter().map(|t| t.spec()).collect();
        self.model.bind_tools(&specs);
        run_message_agent(initial, self.model.clone(), self.tools.clone()).await
    }
}

pub struct ReactAgentBuilder<M> {
    model: M,
    tools: Vec<DynTool>,
    system_prompt: Option<String>,
}

impl<M> ReactAgentBuilder<M>
where
    M: LlmModel + Clone + Send + Sync + 'static,
{
    pub fn new(model: M) -> Self {
        Self {
            model,
            tools: Vec::new(),
            system_prompt: None,
        }
    }

    pub fn with_tools<T, I>(mut self, tools: I) -> Self
    where
        T: IntoDynTool,
        I: IntoIterator<Item = T>,
    {
        self.tools = tools.into_iter().map(|t| t.into_dyn_tool()).collect();
        self
    }

    pub fn with_tool<T>(mut self, tool: T) -> Self
    where
        T: IntoDynTool,
    {
        self.tools.push(tool.into_dyn_tool());
        self
    }

    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    pub fn build(self) -> ReactAgent<M> {
        ReactAgent {
            model: self.model,
            tools: self.tools,
            system_prompt: self.system_prompt,
        }
    }
}
