pub mod node;

use std::{collections::HashMap, error::Error, marker::PhantomData, sync::Arc};

use futures::{Stream, StreamExt};
use langchain_core::{ModelError, ToolError};
use langchain_core::{
    message::Message,
    request::{FormatType, ResponseFormat, ToolSpec},
    state::{AgentState, ChatModel, ChatStreamEvent, MessagesState, RegisteredTool, ToolFn},
    store::BaseStore,
};
use langgraph::label::InternedGraphLabel;
use langgraph::{
    checkpoint::{
        Configuration, {Checkpoint, Checkpointer},
    },
    graph::GraphError,
    label::{BaseGraphLabel, GraphLabel},
    state_graph::{GraphSpec, RunStrategy, StateGraph},
};
use node::identity::IdentityNode;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use smallvec::{SmallVec, smallvec};
use thiserror::Error;
use tracing::debug;

pub use node::llm::LlmNode;
pub use node::tool::ToolNode;

use crate::node::middleware::{AgentMiddleware, AgentMiddlewareNode};

/// Specification for the React Agent Graph
pub struct ReactAgentSpec;

impl GraphSpec for ReactAgentSpec {
    type State = MessagesState;
    type Update = MessagesState;
    type Error = AgentError;
    type Event = ChatStreamEvent;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GraphLabel)]
enum ReactAgentLabel {
    Llm,
    Tool,
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("model error: {0}")]
    Model(#[from] ModelError),
    #[error("tool error: {0}")]
    Tool(#[source] Box<dyn Error + Send + Sync>),
    #[error("graph execution error: {0}")]
    Graph(String),
    #[error("agent error: {0}")]
    Agent(String),
    #[error("structured output error: {0}")]
    StructuredOutput(String),
}

impl From<GraphError<AgentError>> for AgentError {
    fn from(value: GraphError<AgentError>) -> Self {
        match value {
            GraphError::NodeRunError(e) => e,
            _ => Self::Graph(value.to_string()),
        }
    }
}

/// Unified React Agent Builder
pub struct ReactAgentBuilder<M> {
    model: M,
    tools: Vec<RegisteredTool<ToolError>>,
    system_prompt: Option<String>,
    store: Option<Arc<dyn BaseStore>>,
    checkpointer: Option<Arc<dyn Checkpointer<MessagesState>>>,
    middlewares: SmallVec<[AgentMiddleware<MessagesState>; 4]>,
}

impl<M> ReactAgentBuilder<M>
where
    M: ChatModel + Send + Sync + 'static,
{
    pub fn new(model: M) -> Self {
        Self {
            model,
            tools: Vec::new(),
            system_prompt: None,
            store: None,
            checkpointer: None,
            middlewares: SmallVec::new(),
        }
    }

    pub fn with_system_prompt<Str: Into<String>>(mut self, system_prompt: Str) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    pub fn with_tools<I>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = RegisteredTool<ToolError>>,
    {
        self.tools = tools.into_iter().collect();
        self
    }

    pub fn bind_tool(mut self, tool: RegisteredTool<ToolError>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn with_shared_store(mut self, store: Arc<dyn BaseStore>) -> Self {
        self.store = Some(store);
        self
    }

    pub fn with_checkpointer(mut self, checkpointer: Arc<dyn Checkpointer<MessagesState>>) -> Self {
        self.checkpointer = Some(checkpointer);
        self
    }

    pub fn with_middlewares<I>(mut self, middlewares: I) -> Self
    where
        I: IntoIterator<Item = AgentMiddleware<MessagesState>>,
    {
        self.middlewares = middlewares.into_iter().collect();
        self
    }

    /// Transforms this builder into a structured agent builder
    pub fn build(self) -> ReactAgent {
        let (tool_specs, tools) = parse_tool(self.tools);

        let mut graph: StateGraph<ReactAgentSpec> = StateGraph::new(
            BaseGraphLabel::Start,
            |old: &mut MessagesState, update: MessagesState| {
                for msg in update.messages {
                    let exists = old.messages.iter().any(|m| Arc::ptr_eq(m, &msg));
                    if !exists {
                        old.push_message(msg);
                    }
                }
                old.llm_calls += update.llm_calls;
            },
        );

        if let Some(store) = self.store {
            graph = graph.with_shared_store(store);
        }

        if let Some(checkpointer) = self.checkpointer {
            graph = graph.with_shared_checkpointer(checkpointer);
        }

        let mut before_agent_nodes: SmallVec<[_; 4]> = smallvec![];
        let mut before_model_nodes: SmallVec<[_; 4]> = smallvec![];
        let mut after_model_nodes: SmallVec<[_; 4]> = smallvec![];
        let mut after_agent_nodes: SmallVec<[_; 4]> = smallvec![];

        self.middlewares.into_iter().for_each(|middleware| {
            if let Some(before_agent) = middleware.before_agent {
                let before_agent_node = AgentMiddlewareNode::new(before_agent.handler);
                let label = middleware.label.before_agent.intern();

                before_agent_nodes.push(AgentMiddlewareEdge {
                    label,
                    target: before_agent.target,
                    branches: before_agent.branches,
                });
                graph.add_node(label, before_agent_node);
            }

            if let Some(before_model) = middleware.before_model {
                let before_model_node = AgentMiddlewareNode::new(before_model.handler);
                let label = middleware.label.before_model.intern();

                before_model_nodes.push(AgentMiddlewareEdge {
                    label,
                    target: before_model.target,
                    branches: before_model.branches,
                });
                graph.add_node(label, before_model_node);
            }

            if let Some(after_model) = middleware.after_model {
                let after_model_node = AgentMiddlewareNode::new(after_model.handler);
                let label = middleware.label.after_model.intern();

                after_model_nodes.push(AgentMiddlewareEdge {
                    label,
                    target: after_model.target,
                    branches: after_model.branches,
                });
                graph.add_node(label, after_model_node);
            }

            if let Some(after_agent) = middleware.after_agent {
                let after_agent_node = AgentMiddlewareNode::new(after_agent.handler);
                let label = middleware.label.after_agent.intern();

                after_agent_nodes.push(AgentMiddlewareEdge {
                    label,
                    target: after_agent.target,
                    branches: after_agent.branches,
                });
                graph.add_node(label, after_agent_node);
            }
        });

        graph.add_node(
            BaseGraphLabel::Start,
            IdentityNode::<AgentError> {
                _marker: PhantomData,
            },
        );
        graph.add_node(
            BaseGraphLabel::End,
            IdentityNode::<AgentError> {
                _marker: PhantomData,
            },
        );
        graph.add_node(ReactAgentLabel::Llm, LlmNode::new(self.model, tool_specs));
        graph.add_node(ReactAgentLabel::Tool, ToolNode::new(tools));

        let after_agent_entry = apply_middleware_chain(
            &mut graph,
            &after_agent_nodes,
            BaseGraphLabel::End.intern(),
            true,
            false,
        );

        let after_model_entry = apply_middleware_chain(
            &mut graph,
            &after_model_nodes,
            after_agent_entry,
            true,
            true,
        );

        if !after_model_nodes.is_empty() {
            graph.add_edge(ReactAgentLabel::Llm, after_model_entry);
        } else {
            let mut branches = HashMap::new();
            branches.insert(after_agent_entry, after_agent_entry);
            branches.insert(
                ReactAgentLabel::Tool.intern(),
                ReactAgentLabel::Tool.intern(),
            );

            graph.add_condition_edge(
                ReactAgentLabel::Llm,
                branches,
                move |state: &MessagesState| {
                    if state.last_tool_calls().is_some() {
                        smallvec![ReactAgentLabel::Tool.intern()]
                    } else {
                        smallvec![after_agent_entry]
                    }
                },
            );
        }

        let before_model_entry = apply_middleware_chain(
            &mut graph,
            &before_model_nodes,
            ReactAgentLabel::Llm.intern(),
            false,
            false,
        );

        let before_agent_entry = apply_middleware_chain(
            &mut graph,
            &before_agent_nodes,
            before_model_entry,
            false,
            false,
        );

        graph.add_edge(BaseGraphLabel::Start, before_agent_entry);
        graph.add_edge(ReactAgentLabel::Tool, ReactAgentLabel::Llm);

        ReactAgent {
            graph,
            system_prompt: self.system_prompt,
        }
    }
}

struct AgentMiddlewareEdge {
    label: InternedGraphLabel,
    target: Option<InternedGraphLabel>,
    branches: SmallVec<[InternedGraphLabel; 2]>,
}

fn apply_middleware_chain(
    graph: &mut StateGraph<ReactAgentSpec>,
    nodes: &[AgentMiddlewareEdge],
    next_label: InternedGraphLabel,
    reverse: bool,
    check_tool_calls: bool,
) -> InternedGraphLabel {
    if nodes.is_empty() {
        return next_label;
    }

    let execution_sequence: Vec<&AgentMiddlewareEdge> = if reverse {
        nodes.iter().rev().collect()
    } else {
        nodes.iter().collect()
    };

    for (i, node) in execution_sequence.iter().enumerate() {
        let current_label = node.label;
        let target = node.target;

        let is_last = i == execution_sequence.len() - 1;
        let next = if is_last {
            next_label
        } else {
            execution_sequence[i + 1].label
        };

        let mut branches = node
            .branches
            .iter()
            .map(|&l| (l, l))
            .collect::<HashMap<_, _>>();
        branches.insert(next, next);
        if check_tool_calls && is_last {
            branches.insert(
                ReactAgentLabel::Tool.intern(),
                ReactAgentLabel::Tool.intern(),
            );
        }

        graph.add_condition_edge(current_label, branches, move |state: &MessagesState| {
            if let Some(target) = target {
                smallvec![target]
            } else {
                if check_tool_calls && is_last && state.last_tool_calls().is_some() {
                    smallvec![ReactAgentLabel::Tool.intern()]
                } else {
                    smallvec![next]
                }
            }
        });
    }

    execution_sequence[0].label
}

pub struct ReactAgent {
    pub graph: StateGraph<ReactAgentSpec>,
    pub system_prompt: Option<String>,
}

impl ReactAgent {
    pub fn builder<M>(model: M) -> ReactAgentBuilder<M>
    where
        M: ChatModel + Send + Sync + 'static,
    {
        ReactAgentBuilder::new(model)
    }

    pub fn create_agent<M>(model: M, tools: Vec<RegisteredTool<ToolError>>) -> Self
    where
        M: ChatModel + Send + Sync + 'static,
    {
        Self::builder(model).with_tools(tools).build()
    }

    pub async fn invoke(
        &self,
        message: Message,
        thread_id: Option<&str>,
    ) -> Result<MessagesState, AgentError> {
        let config = thread_id.map_or(
            Configuration {
                thread_id: None,
                response_format: None,
            },
            |thread_id| Configuration {
                thread_id: Some(thread_id.to_owned()),
                response_format: None,
            },
        );

        let (mut state, resume_from) = self.get_state(&config).await;
        state.push_message_owned(message.clone());
        let max_steps = 25;

        let (state, _) = self
            .graph
            .run(
                state,
                &config,
                max_steps,
                RunStrategy::StopAtNonLinear,
                resume_from,
            )
            .await?;

        Ok(state)
    }

    pub async fn invoke_structured<S>(
        &self,
        message: Message,
        thread_id: Option<&str>,
    ) -> Result<AgentState<MessagesState, S>, AgentError>
    where
        S: DeserializeOwned + JsonSchema,
    {
        let mode = FormatType::JsonObject;

        let response_format = match mode {
            FormatType::JsonSchema => {
                let schema = serde_json::to_string(&schemars::schema_for!(S)).map_err(|e| {
                    AgentError::StructuredOutput(format!("Failed to serialize schema: {}", e))
                })?;
                Some(ResponseFormat {
                    format_type: FormatType::JsonSchema,
                    json_schema: Some(schema),
                })
            }
            FormatType::JsonObject => Some(ResponseFormat {
                format_type: FormatType::JsonObject,
                json_schema: None,
            }),
            _ => None,
        };

        let config = thread_id.map_or(
            Configuration {
                thread_id: None,
                response_format: response_format.clone(),
            },
            |thread_id| Configuration {
                thread_id: Some(thread_id.to_owned()),
                response_format,
            },
        );

        let (mut state, resume_from) = self.get_state(&config).await;
        state.push_message_owned(message.clone());
        let max_steps = 25;

        let (state, _) = self
            .graph
            .run(
                state,
                &config,
                max_steps,
                RunStrategy::StopAtNonLinear,
                resume_from,
            )
            .await?;

        let content = state
            .last_assistant()
            .ok_or_else(|| AgentError::Agent("No assistant message in state".to_owned()))?
            .content();

        let output: S = serde_json::from_str(content).map_err(|e| {
            AgentError::StructuredOutput(format!("Failed to parse structured output: {}", e))
        })?;

        Ok(AgentState {
            state,
            struct_output: Some(output),
        })
    }

    pub async fn stream<'a>(
        &'a self,
        message: Message,
        thread_id: Option<&str>,
    ) -> Result<impl Stream<Item = ChatStreamEvent> + 'a, AgentError> {
        let graph = &self.graph;

        let config = thread_id.map_or(
            Configuration {
                thread_id: None,
                response_format: None,
            },
            |thread_id| Configuration {
                thread_id: Some(thread_id.to_owned()),
                response_format: None,
            },
        );

        let (mut state, resume_from) = self.get_state(&config).await;

        state.push_message_owned(message.clone());
        let max_steps = 25;

        let stream = async_stream::stream! {
            let mut inner_stream = graph.stream(
                state,
                &config,
                max_steps,
                RunStrategy::StopAtNonLinear,
                resume_from,
            );

            while let Some(item) = inner_stream.next().await {
                yield item;
            }
        };

        Ok(stream)
    }

    async fn get_state(
        &self,
        config: &Configuration,
    ) -> (MessagesState, Option<SmallVec<[String; 4]>>) {
        if let Some(checkpointer) = &self.graph.checkpointer
            && let Some(thread_id) = &config.thread_id
        {
            debug!("有checkpointer，尝试从checkpointer获取状态");
            if let Ok(Some(checkpoint)) = checkpointer.get(thread_id).await {
                debug!("从checkpointer获取状态成功");
                (checkpoint.state, Some(checkpoint.next_nodes))
            } else {
                debug!("从checkpointer获取状态失败，初始化新状态");
                let mut state = MessagesState::default();
                if let Some(system_prompt) = &self.system_prompt {
                    state.push_message_owned(Message::system(system_prompt.clone()));
                }
                let checkpoint = Checkpoint::new_auto(state.clone(), thread_id.clone(), 0, None);
                if let Err(e) = checkpointer.put(&checkpoint).await {
                    tracing::error!("Failed to save checkpoint: {:?}", e);
                }
                (state, None)
            }
        } else {
            let mut state = MessagesState::default();
            if let Some(system_prompt) = &self.system_prompt {
                state.push_message_owned(Message::system(system_prompt.clone()));
            }
            (state, None)
        }
    }
}

fn parse_tool<E>(tools: Vec<RegisteredTool<E>>) -> (Vec<ToolSpec>, HashMap<String, Box<ToolFn<E>>>)
where
    E: Error + Send + Sync + 'static,
{
    let mut tool_specs = Vec::new();
    let tools: HashMap<String, Box<ToolFn<E>>> = tools
        .into_iter()
        .map(|t| {
            let spec = ToolSpec::Function {
                function: t.function.clone(),
            };
            tool_specs.push(spec);
            (t.function.name, t.handler)
        })
        .collect();
    (tool_specs, tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use langchain_core::state::{ChatCompletion, ChatStreamEvent};
    use langchain_core::tool;
    use langchain_core::{
        message::{FunctionCall, Message, ToolCall},
        response::Usage,
    };

    #[derive(Debug, Error)]
    enum TestError {
        #[error(transparent)]
        Json(#[from] serde_json::Error),
    }

    #[derive(Debug)]
    struct TestModel;

    #[async_trait]
    impl ChatModel for TestModel {
        async fn invoke(
            &self,
            _messages: &[std::sync::Arc<Message>],
            options: &langchain_core::state::InvokeOptions<'_>,
        ) -> Result<ChatCompletion, langchain_core::error::ModelError> {
            let tool_calls = if options.tools.is_some() {
                let call = ToolCall {
                    id: "call1".to_owned(),
                    type_name: "function".to_owned(),
                    function: FunctionCall {
                        name: "test_tool".to_owned(),
                        arguments: serde_json::json!({}),
                    },
                };
                Some(vec![call])
            } else {
                None
            };

            let msg = Message::Assistant {
                content: "assistant".to_owned(),
                tool_calls,
                name: None,
            };

            let usage = Usage::default();
            Ok(ChatCompletion {
                messages: vec![std::sync::Arc::new(msg)],
                usage,
            })
        }

        async fn stream(
            &self,
            _messages: &[std::sync::Arc<Message>],
            _options: &langchain_core::state::InvokeOptions<'_>,
        ) -> Result<langchain_core::state::StandardChatStream, langchain_core::error::ModelError>
        {
            use async_stream::try_stream;

            let stream = try_stream! {
                yield ChatStreamEvent::Content("assistant".to_owned());
                yield ChatStreamEvent::ToolCallDelta {
                    index: 0,
                    id: Some("call1".to_owned()),
                    type_name: Some("function".to_owned()),
                    name: Some("test_tool".to_owned()),
                    arguments: Some("{}".to_owned()),
                };
                yield ChatStreamEvent::Done {
                    finish_reason: Some("stop".to_owned()),
                    usage: Some(Usage::default()),
                };
            };

            Ok(Box::pin(stream))
        }
    }

    #[expect(unused)]
    #[derive(Debug)]
    struct TestTool;

    #[tool(description = "test tool")]
    async fn test_tool() -> Result<String, TestError> {
        Ok("tool_result".to_owned())
    }

    #[tokio::test]
    async fn llm_and_tool_nodes_work_in_state_graph() {
        let tool = test_tool_tool();
        let agent = ReactAgent::builder(TestModel)
            .with_tools(vec![tool])
            .build();

        let _final_state = agent.invoke(Message::user("hello"), None).await.unwrap();
    }

    #[tokio::test]
    async fn test_react_agent_with_checkpointer() {
        use langgraph::checkpoint::MemorySaver;
        use std::sync::Arc;

        let checkpointer = Arc::new(MemorySaver::new());
        // let tool = test_tool_tool();
        let agent = ReactAgent::builder(TestModel)
            // .with_tools(vec![tool])
            .with_checkpointer(checkpointer.clone())
            .build();

        // 第一次调用，使用 thread_id "thread-1"
        let thread_id = "thread-1";
        let state1 = agent
            .invoke(Message::user("hello"), Some(thread_id))
            .await
            .unwrap();

        // 验证状态
        assert_eq!(state1.messages.len(), 2); // user + assistant

        // 第二次调用，使用相同的 thread_id
        let state2 = agent
            .invoke(Message::user("world"), Some(thread_id))
            .await
            .unwrap();

        // 验证状态累积 (user + assistant + user + assistant)
        assert_eq!(state2.messages.len(), 4);

        // 第三次调用，不使用 thread_id
        let state3 = agent
            .invoke(Message::user("no memory"), None)
            .await
            .unwrap();

        // 验证状态未累积 (user + assistant)
        assert_eq!(state3.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_react_agent_without_checkpointer() {
        // let tool = test_tool_tool();
        let agent = ReactAgent::builder(TestModel)
            // .with_tools(vec![tool])
            .build(); // 默认无 checkpointer

        let thread_id = "thread-2";

        // 第一次调用
        let state1 = agent
            .invoke(Message::user("hello"), Some(thread_id))
            .await
            .unwrap();
        assert_eq!(state1.messages.len(), 2);

        // 第二次调用，即使传入 thread_id，由于没有 checkpointer，状态也不会累积
        let state2 = agent
            .invoke(Message::user("world"), Some(thread_id))
            .await
            .unwrap();
        assert_eq!(state2.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_react_agent_system_prompt() {
        let agent = ReactAgent::builder(TestModel)
            .with_system_prompt("You are a helpful assistant")
            .build();

        let state = agent.invoke(Message::user("hello"), None).await.unwrap();

        // Expect: System + User + Assistant
        assert_eq!(state.messages.len(), 3);
        match state.messages[0].as_ref() {
            Message::System { content, .. } => assert_eq!(content, "You are a helpful assistant"),
            _ => panic!("First message should be system"),
        }
    }
}
