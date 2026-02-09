use std::{collections::HashMap, error::Error, marker::PhantomData, mem, sync::Arc};

use async_trait::async_trait;
use futures::{Stream, StreamExt, future::join_all};
use langchain_core::{
    message::{FunctionCall, Message, ToolCall},
    request::{FormatType, ResponseFormat, ToolSpec},
    state::{
        AgentState, ChatCompletion, ChatModel, ChatStreamEvent, InvokeOptions, MessagesState,
        RegisteredTool, ToolFn,
    },
    store::BaseStore,
};
use langgraph::{
    checkpoint::{Checkpoint, Checkpointer, RunnableConfig},
    graph::GraphError,
    label::{BaseGraphLabel, GraphLabel},
    node::{EventSink, Node, NodeContext},
    state_graph::RunStrategy,
    state_graph::StateGraph,
};
use schemars::JsonSchema;
use thiserror::Error;

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

pub struct ToolNode<E>
where
    E: Send + Sync + 'static,
{
    pub tools: HashMap<String, Box<ToolFn<E>>>,
}

impl<E> ToolNode<E>
where
    E: Send + Sync + 'static,
{
    pub fn new(tools: HashMap<String, Box<ToolFn<E>>>) -> Self {
        Self { tools }
    }
}

struct IdentityNode<E> {
    _marker: PhantomData<E>,
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
        _sink: &mut dyn EventSink<ChatStreamEvent>,
        context: NodeContext<'_>,
    ) -> Result<MessagesState, E> {
        self.run_sync(input, context).await
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
        _context: NodeContext<'_>,
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
                    futures.push((handler)(call.arguments()));
                }
            }
            let results = join_all(futures).await;
            for (id, result) in ids.into_iter().zip(results.into_iter()) {
                let content = match result {
                    Ok(value) => {
                        tracing::debug!("Tool call result: {}", value);
                        value.to_string()
                    }
                    Err(e) => {
                        tracing::error!("Tool call failed: {}", e);
                        format!("Error: {}", e)
                    }
                };
                delta.push_message_owned(Message::tool(content, id));
            }
        }
        Ok(delta)
    }

    async fn run_stream(
        &self,
        input: &MessagesState,
        _sink: &mut dyn EventSink<ChatStreamEvent>,
        context: NodeContext<'_>,
    ) -> Result<MessagesState, AgentError> {
        self.run_sync(input, context).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GraphLabel)]
enum ReactAgentLabel {
    Llm,
    Tool,
}

pub use langchain_core::ToolError;
use tracing::debug;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("model error: {0}")]
    Model(#[source] Box<dyn Error + Send + Sync>),
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
        }
    }

    pub fn with_system_prompt<Str: Into<String>>(mut self, system_prompt: Str) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    pub fn with_tools(mut self, tools: Vec<RegisteredTool<ToolError>>) -> Self {
        self.tools = tools;
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

    /// Transforms this builder into a structured agent builder
    pub fn build(self) -> ReactAgent {
        let (tool_specs, tools) = parse_tool(self.tools);

        let mut graph: StateGraph<MessagesState, MessagesState, AgentError, ChatStreamEvent> =
            StateGraph::new(
                BaseGraphLabel::Start,
                |mut old: MessagesState, update: MessagesState| {
                    old.extend_messages(update.messages);
                    old.llm_calls += update.llm_calls;
                    old
                },
            );

        if let Some(store) = self.store {
            graph = graph.with_shared_store(store);
        }

        if let Some(checkpointer) = self.checkpointer {
            graph = graph.with_shared_checkpointer(checkpointer);
        }

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

        let mut branches = HashMap::new();
        branches.insert(
            ReactAgentLabel::Tool.intern(),
            ReactAgentLabel::Tool.intern(),
        );
        branches.insert(BaseGraphLabel::End.intern(), BaseGraphLabel::End.intern());

        graph.add_edge(BaseGraphLabel::Start, ReactAgentLabel::Llm);
        graph.add_condition_edge(ReactAgentLabel::Llm, branches, |state: &MessagesState| {
            if state.last_tool_calls().is_some() {
                vec![ReactAgentLabel::Tool.intern()]
            } else {
                vec![BaseGraphLabel::End.intern()]
            }
        });

        graph.add_edge(ReactAgentLabel::Tool, ReactAgentLabel::Llm);

        ReactAgent {
            graph,
            system_prompt: self.system_prompt,
        }
    }
}

pub struct ReactAgent {
    pub graph: StateGraph<MessagesState, MessagesState, AgentError, ChatStreamEvent>,
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
        thread_id: Option<String>,
    ) -> Result<MessagesState, AgentError> {
        let config = thread_id.map_or(
            RunnableConfig {
                thread_id: None,
                response_format: None,
            },
            |thread_id| RunnableConfig {
                thread_id: Some(thread_id),
                response_format: None,
            },
        );

        let mut state = self.get_state(&config).await;
        state.push_message_owned(message.clone());
        let max_steps = 25;

        let (state, _) = self
            .graph
            .run(state, &config, max_steps, RunStrategy::StopAtNonLinear)
            .await?;

        Ok(state)
    }

    pub async fn invoke_structured<S>(
        &self,
        message: Message,
        thread_id: Option<String>,
    ) -> Result<AgentState<MessagesState, S>, AgentError>
    where
        S: serde::de::DeserializeOwned + JsonSchema,
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
            RunnableConfig {
                thread_id: None,
                response_format: response_format.clone(),
            },
            |thread_id| RunnableConfig {
                thread_id: Some(thread_id),
                response_format,
            },
        );

        let mut state = self.get_state(&config).await;
        state.push_message_owned(message.clone());
        let max_steps = 25;

        let (state, _) = self
            .graph
            .run(state, &config, max_steps, RunStrategy::StopAtNonLinear)
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
        thread_id: Option<String>,
    ) -> Result<impl Stream<Item = ChatStreamEvent> + 'a, AgentError> {
        let graph = &self.graph;

        let config = thread_id.map_or(
            RunnableConfig {
                thread_id: None,
                response_format: None,
            },
            |thread_id| RunnableConfig {
                thread_id: Some(thread_id),
                response_format: None,
            },
        );

        let mut state = self.get_state(&config).await;

        state.push_message_owned(message.clone());
        let max_steps = 25;

        let stream = async_stream::stream! {
            let mut inner_stream = graph.stream(
                state,
                &config,
                max_steps,
                RunStrategy::StopAtNonLinear,
            );

            while let Some(item) = inner_stream.next().await {
                yield item;
            }
        };

        Ok(stream)
    }

    async fn get_state(&self, config: &RunnableConfig) -> MessagesState {
        if let Some(checkpointer) = &self.graph.checkpointer
            && let Some(thread_id) = &config.thread_id
        {
            debug!("有checkpointer，尝试从checkpointer获取状态");
            if let Ok(Some(checkpoint)) = checkpointer.get(thread_id).await {
                debug!("从checkpointer获取状态成功");
                checkpoint.state
            } else {
                debug!("从checkpointer获取状态失败，初始化新状态");
                let mut state = MessagesState::default();
                if let Some(system_prompt) = &self.system_prompt {
                    state.push_message_owned(Message::system(system_prompt.clone()));
                }
                let checkpoint = Checkpoint {
                    state: state.clone(),
                    next_nodes: Vec::new(),
                    pending_interrupt: None,
                };
                checkpointer.put(thread_id, &checkpoint).await.unwrap();
                state
            }
        } else {
            let mut state = MessagesState::default();
            if let Some(system_prompt) = &self.system_prompt {
                state.push_message_owned(Message::system(system_prompt.clone()));
            }
            state
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
    use langchain_core::state::ChatStreamEvent;
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
        ) -> Result<ChatCompletion, Box<dyn std::error::Error + Send + Sync>> {
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
        ) -> Result<
            langchain_core::state::StandardChatStream,
            Box<dyn std::error::Error + Send + Sync>,
        > {
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
        let thread_id = "thread-1".to_owned();
        let state1 = agent
            .invoke(Message::user("hello"), Some(thread_id.clone()))
            .await
            .unwrap();

        // 验证状态
        assert_eq!(state1.messages.len(), 2); // user + assistant

        // 第二次调用，使用相同的 thread_id
        let state2 = agent
            .invoke(Message::user("world"), Some(thread_id.clone()))
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

        let thread_id = "thread-2".to_owned();

        // 第一次调用
        let state1 = agent
            .invoke(Message::user("hello"), Some(thread_id.clone()))
            .await
            .unwrap();
        assert_eq!(state1.messages.len(), 2);

        // 第二次调用，即使传入 thread_id，由于没有 checkpointer，状态也不会累积
        let state2 = agent
            .invoke(Message::user("world"), Some(thread_id.clone()))
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
