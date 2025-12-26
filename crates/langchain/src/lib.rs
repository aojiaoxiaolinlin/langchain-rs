use std::{collections::HashMap, error::Error, marker::PhantomData, mem};

use async_trait::async_trait;
use futures::{Stream, StreamExt, future::join_all};
use langchain_core::{
    message::{FunctionCall, Message, ToolCall},
    request::ToolSpec,
    state::{
        ChatCompletion, ChatModel, ChatStreamEvent, InvokeOptions, MessagesState, RegisteredTool,
        ToolFn,
    },
};
use langgraph::{
    graph::GraphError,
    label::{BaseGraphLabel, GraphLabel},
    state_graph::RunStrategy,
};
use langgraph::{
    node::{EventSink, Node},
    state_graph::StateGraph,
};
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
    async fn run_sync(&self, input: &MessagesState) -> Result<MessagesState, AgentError> {
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
                            // 此时上一个tool_call的arguments参数才是完整的
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
                    // 最后一个tool_call的arguments参数需要在这里处理
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
    async fn run_sync(&self, _input: &MessagesState) -> Result<MessagesState, E> {
        Ok(MessagesState::default())
    }

    async fn run_stream(
        &self,
        input: &MessagesState,
        _sink: &mut dyn EventSink<ChatStreamEvent>,
    ) -> Result<MessagesState, E> {
        self.run_sync(input).await
    }
}

#[async_trait]
impl<E> Node<MessagesState, MessagesState, AgentError, ChatStreamEvent> for ToolNode<E>
where
    E: Error + Send + Sync + 'static,
{
    async fn run_sync(&self, input: &MessagesState) -> Result<MessagesState, AgentError> {
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
                        // 错误信息也返回给LLM、让它决定是否重试
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
    ) -> Result<MessagesState, AgentError> {
        self.run_sync(input).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GraphLabel)]
enum ReactAgentLabel {
    Llm,
    Tool,
}

pub use langchain_core::ToolError;

#[derive(Debug, Error)]
pub enum AgentError {
    /// 模型执行时发生错误
    #[error("model error: {0}")]
    Model(#[source] Box<dyn Error + Send + Sync>),
    /// 工具执行时发生错误
    #[error("tool error: {0}")]
    Tool(#[source] Box<dyn Error + Send + Sync>),
    /// 用户通常不需要在代码中处理这些系统错误，只需记录日志
    #[error("graph execution error: {0}")]
    Graph(String),
}

impl From<GraphError<AgentError>> for AgentError {
    fn from(value: GraphError<AgentError>) -> Self {
        match value {
            GraphError::NodeRunError(e) => e,
            _ => Self::Graph(value.to_string()),
        }
    }
}

pub struct ReactAgent {
    graph: StateGraph<MessagesState, MessagesState, AgentError, ChatStreamEvent>,
    system_prompt: Option<String>,
}

impl ReactAgent {
    pub fn create_agent<M>(model: M, tools: Vec<RegisteredTool<ToolError>>) -> Self
    where
        M: ChatModel + Send + Sync + 'static,
    {
        let (tool_specs, tools) = parse_tool(tools);

        // 使用 MessagesState 作为更新类型（与输入相同），reducer 负责合并
        let mut graph: StateGraph<MessagesState, MessagesState, AgentError, ChatStreamEvent> =
            StateGraph::new(
                BaseGraphLabel::Start,
                |mut old: MessagesState, update: MessagesState| {
                    // 合并逻辑：将 update 中的消息追加到 old 中，并累加 llm_calls
                    old.extend_messages(update.messages);
                    old.llm_calls += update.llm_calls;
                    old
                },
            );

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
        graph.add_node(ReactAgentLabel::Llm, LlmNode::new(model, tool_specs));
        graph.add_node(ReactAgentLabel::Tool, ToolNode::new(tools));

        let mut branches = HashMap::new();
        // 使用 Label 作为分支键
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

        Self {
            graph,
            system_prompt: None,
        }
    }

    pub fn with_system_prompt<T: Into<String>>(mut self, system_prompt: T) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    pub async fn invoke(&self, message: Message) -> Result<MessagesState, AgentError> {
        let mut state = MessagesState::default();
        if let Some(system_prompt) = &self.system_prompt {
            state.push_message_owned(Message::system(system_prompt.clone()));
        }
        state.push_message_owned(message);
        let max_steps = 25;
        let (state, _) = self
            .graph
            .run(state, max_steps, RunStrategy::StopAtNonLinear)
            .await?;
        Ok(state)
    }

    pub async fn stream(
        &self,
        message: Message,
    ) -> Result<impl Stream<Item = ChatStreamEvent> + '_, AgentError> {
        let mut state = MessagesState::default();
        if let Some(system_prompt) = &self.system_prompt {
            state.push_message_owned(Message::system(system_prompt.clone()));
        }
        state.push_message_owned(message);
        let max_steps = 25;
        let stream = self
            .graph
            .stream(state, max_steps, RunStrategy::StopAtNonLinear);

        Ok(stream)
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
    use langgraph::{label::GraphLabel, state_graph::StateGraph};

    #[derive(Debug, Error)]
    enum TestError {
        #[error(transparent)]
        Json(#[from] serde_json::Error),
        #[error("custom error")]
        Custom,
    }

    #[derive(Debug)]
    struct TestModel;

    #[async_trait]
    impl ChatModel for TestModel {
        async fn invoke(
            &self,
            _messages: &[std::sync::Arc<Message>],
            _options: &langchain_core::state::InvokeOptions<'_>,
        ) -> Result<ChatCompletion, Box<dyn std::error::Error + Send + Sync>> {
            let call = ToolCall {
                id: "call1".to_owned(),
                type_name: "function".to_owned(),
                function: FunctionCall {
                    name: "test_tool".to_owned(),
                    arguments: serde_json::json!({}),
                },
            };
            let msg = Message::Assistant {
                content: "assistant".to_owned(),
                tool_calls: Some(vec![call]),
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

    #[tool(description = "fail tool")]
    async fn fail_tool() -> Result<String, TestError> {
        Err(TestError::Custom)
    }

    #[derive(Debug, Clone, PartialEq, Eq, Hash, GraphLabel)]
    enum TestLabel {
        Llm,
        Tool,
    }

    #[tokio::test]
    async fn llm_and_tool_nodes_work_in_state_graph() {
        let mut sg: StateGraph<MessagesState, MessagesState, AgentError, ChatStreamEvent> =
            StateGraph::new(
                TestLabel::Llm,
                |mut old: MessagesState, update: MessagesState| {
                    old.extend_messages(update.messages);
                    old.llm_calls += update.llm_calls;
                    old
                },
            );

        let tool = test_tool_tool();

        let (tool_specs, tools) = parse_tool(vec![tool]);

        let llm_node = LlmNode::new(TestModel, tool_specs);

        let tool_node = ToolNode::new(tools);

        sg.add_node(TestLabel::Llm, llm_node);
        sg.add_node(TestLabel::Tool, tool_node);

        sg.add_edge(TestLabel::Llm, TestLabel::Tool);

        let initial = MessagesState::new(vec![Message::user("hello")]);

        let (final_state, final_label) = sg.run_until_stuck(initial, 10).await.unwrap();

        assert_eq!(final_label, vec![TestLabel::Tool.intern()]);
        assert_eq!(final_state.llm_calls, 1);
        assert_eq!(final_state.messages.len(), 3);

        match final_state.messages[0].as_ref() {
            Message::User { .. } => {}
            _ => panic!("first message must be user"),
        }

        match final_state.messages[1].as_ref() {
            Message::Assistant { tool_calls, .. } => {
                assert!(tool_calls.is_some());
                assert_eq!(tool_calls.as_ref().unwrap().len(), 1);
            }
            _ => panic!("second message must be assistant"),
        }

        match final_state.messages[2].as_ref() {
            Message::Tool { .. } => {}
            _ => panic!("third message must be tool"),
        }
    }

    #[tokio::test]
    async fn tool_node_captures_error() {
        let tool = fail_tool_tool();
        let (_, tools) = parse_tool(vec![tool]);
        let tool_node = ToolNode::new(tools);

        // Construct input state with a tool call
        let mut input = MessagesState::default();
        let call = ToolCall {
            id: "call1".to_owned(),
            type_name: "function".to_owned(),
            function: FunctionCall {
                name: "fail_tool".to_owned(),
                arguments: serde_json::json!({}),
            },
        };
        input.push_message_owned(Message::Assistant {
            content: "".to_owned(),
            tool_calls: Some(vec![call]),
            name: None,
        });

        // Run tool node
        let result = tool_node.run_sync(&input).await;

        assert!(result.is_ok());
        let delta = result.unwrap();
        assert_eq!(delta.messages.len(), 1);

        match delta.messages[0].as_ref() {
            Message::Tool { content, .. } => {
                assert!(content.contains("custom error"));
            }
            _ => panic!("expected tool message"),
        }
    }
}
