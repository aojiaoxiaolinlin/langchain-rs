use std::{collections::HashMap, error::Error, marker::PhantomData, mem};

use async_trait::async_trait;
use futures::{Stream, StreamExt, future::try_join_all};
use langchain_core::{
    message::{FunctionCall, Message, ToolCall},
    request::ToolSpec,
    state::{ChatCompletion, ChatModel, ChatStreamEvent, MessagesState, RegisteredTool, ToolFn},
};
use langgraph::{
    graph::{GraphError, GraphStepError},
    label::{BaseGraphLabel, GraphLabel},
    state_graph::RunStrategy,
};
use langgraph::{
    node::{EventSink, Node},
    state_graph::StateGraph,
};
use serde_json::Value;
use thiserror::Error;

pub struct LlmNode<M, E>
where
    M: ChatModel<Error = E> + 'static,
    E: Send + Sync + 'static,
{
    pub model: M,
    pub tools: Vec<ToolSpec>,
}

impl<M, E> LlmNode<M, E>
where
    M: ChatModel<Error = E> + 'static,
    E: Send + Sync + 'static,
{
    pub fn new(model: M, tools: Vec<ToolSpec>) -> Self {
        Self { model, tools }
    }
}

#[async_trait]
impl<M, LE> Node<MessagesState, MessagesState, ReActAgentError, ChatStreamEvent> for LlmNode<M, LE>
where
    M: ChatModel<Error = LE> + Send + Sync + 'static,
    LE: Error + Send + Sync + 'static,
{
    async fn run_sync(&self, input: &MessagesState) -> Result<MessagesState, ReActAgentError> {
        let mut next = input.clone();
        let completion: ChatCompletion = self
            .model
            .invoke(next.messages.iter().cloned().collect(), self.tools.clone())
            .await
            .map_err(|e| ReActAgentError::Model(Box::new(e)))?;
        tracing::debug!("LLM completion: {:?}", completion);
        next.extend_messages(completion.messages);
        next.increment_llm_calls();
        Ok(next)
    }

    async fn run_stream(
        &self,
        input: &MessagesState,
        sink: &mut dyn EventSink<ChatStreamEvent>,
    ) -> Result<MessagesState, ReActAgentError> {
        let mut next = input.clone();
        let mut completion_stream = self
            .model
            .stream(next.messages.iter().cloned().collect(), self.tools.clone())
            .await
            .map_err(|e| ReActAgentError::Model(Box::new(e)))?;

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        let mut raw_args = String::new();

        while let Some(event) = completion_stream.next().await {
            let event = event.map_err(|e| ReActAgentError::Model(Box::new(e)))?;
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
            next.push_message(assistant);
        }

        next.increment_llm_calls();
        Ok(next)
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
    async fn run_sync(&self, input: &MessagesState) -> Result<MessagesState, E> {
        Ok(input.clone())
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
impl<E> Node<MessagesState, MessagesState, ReActAgentError, ChatStreamEvent> for ToolNode<E>
where
    E: Error + Send + Sync + 'static,
{
    async fn run_sync(&self, input: &MessagesState) -> Result<MessagesState, ReActAgentError> {
        let mut next = input.clone();
        if let Some(calls) = input.last_tool_calls() {
            let mut futures = Vec::new();
            let mut ids = Vec::new();
            tracing::debug!("Tool calls count: {}", calls.len());
            for call in calls {
                if let Some(handler) = self.tools.get(call.function_name()) {
                    ids.push(call.id().to_string());
                    tracing::debug!("Tool call: {:?}", call.function);
                    futures.push((handler)(call.arguments()));
                }
            }
            let results: Vec<Value> = try_join_all(futures)
                .await
                .map_err(|e| ReActAgentError::Tool(Box::new(e)))?;
            for (id, value) in ids.into_iter().zip(results.into_iter()) {
                tracing::debug!("Tool call result: {}", value);
                next.push_message(Message::tool(value.to_string(), id));
            }
        }
        Ok(next)
    }

    async fn run_stream(
        &self,
        input: &MessagesState,
        _sink: &mut dyn EventSink<ChatStreamEvent>,
    ) -> Result<MessagesState, ReActAgentError> {
        self.run_sync(input).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, GraphLabel)]
enum ReactAgentLabel {
    Llm,
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ReactAgentBranch {
    Tool,
    End,
}

pub use langchain_core::ToolError;

#[derive(Debug, Error)]
pub enum ReActAgentError {
    #[error("model error")]
    Model(#[source] Box<dyn Error + Send + Sync>),
    #[error("tool error")]
    Tool(#[source] Box<dyn Error + Send + Sync>),
    #[error(transparent)]
    Graph(#[from] GraphError),
}

impl From<GraphStepError<ReActAgentError>> for ReActAgentError {
    fn from(value: GraphStepError<ReActAgentError>) -> Self {
        match value {
            GraphStepError::Graph(e) => Self::Graph(e),
            GraphStepError::Node(e) => e,
        }
    }
}

pub struct ReactAgent {
    graph: StateGraph<MessagesState, ReActAgentError, ReactAgentBranch, ChatStreamEvent>,
    system_prompt: Option<String>,
}

impl ReactAgent {
    pub fn create_agent<M>(model: M, tools: Vec<RegisteredTool<ToolError>>) -> Self
    where
        M: ChatModel + Send + Sync + 'static,
        M::Error: Error + Send + Sync + 'static,
    {
        let (tool_specs, tools) = parse_tool(tools);

        let mut graph: StateGraph<
            MessagesState,
            ReActAgentError,
            ReactAgentBranch,
            ChatStreamEvent,
        > = StateGraph::from_entry(BaseGraphLabel::Start);

        graph.add_node(
            BaseGraphLabel::Start,
            IdentityNode::<ReActAgentError> {
                _marker: PhantomData,
            },
        );
        graph.add_node(
            BaseGraphLabel::End,
            IdentityNode::<ReActAgentError> {
                _marker: PhantomData,
            },
        );
        graph.add_node(ReactAgentLabel::Llm, LlmNode::new(model, tool_specs));
        graph.add_node(ReactAgentLabel::Tool, ToolNode::new(tools));

        let mut branches = HashMap::new();
        branches.insert(ReactAgentBranch::Tool, ReactAgentLabel::Tool.intern());
        branches.insert(ReactAgentBranch::End, BaseGraphLabel::End.intern());
        graph.add_edge(BaseGraphLabel::Start, ReactAgentLabel::Llm);
        graph.add_condition_edge(ReactAgentLabel::Llm, branches, |state: &MessagesState| {
            if state.last_tool_calls().is_some() {
                vec![ReactAgentBranch::Tool]
            } else {
                vec![ReactAgentBranch::End]
            }
        });

        graph.add_edge(ReactAgentLabel::Tool, ReactAgentLabel::Llm);

        Self {
            graph,
            system_prompt: None,
        }
    }

    pub fn with_system_prompt(mut self, system_prompt: String) -> Self {
        self.system_prompt = Some(system_prompt);
        self
    }

    pub async fn invoke(&self, message: Message) -> Result<MessagesState, ReActAgentError> {
        let mut state = MessagesState::default();
        if let Some(system_prompt) = &self.system_prompt {
            state.push_message(Message::system(system_prompt.clone()));
        }
        state.push_message(message);
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
    ) -> Result<impl Stream<Item = ChatStreamEvent> + '_, ReActAgentError> {
        let mut state = MessagesState::default();
        if let Some(system_prompt) = &self.system_prompt {
            state.push_message(Message::system(system_prompt.clone()));
        }
        state.push_message(message);
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
    }

    #[derive(Debug)]
    struct TestModel;

    #[async_trait]
    impl ChatModel for TestModel {
        type Error = TestError;

        async fn invoke(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolSpec>,
        ) -> Result<ChatCompletion, Self::Error> {
            let call = ToolCall {
                id: "call1".to_string(),
                type_name: "function".to_string(),
                function: FunctionCall {
                    name: "test_tool".to_string(),
                    arguments: serde_json::json!({}),
                },
            };
            let msg = Message::Assistant {
                content: "assistant".to_string(),
                tool_calls: Some(vec![call]),
                name: None,
            };

            let usage = Usage::default();
            Ok(ChatCompletion {
                messages: vec![msg],
                usage,
            })
        }

        async fn stream(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolSpec>,
        ) -> Result<langchain_core::state::ChatStream<Self::Error>, Self::Error> {
            use async_stream::try_stream;

            let stream = try_stream! {
                yield ChatStreamEvent::Content("assistant".to_string());
                yield ChatStreamEvent::ToolCallDelta {
                    index: 0,
                    id: Some("call1".to_string()),
                    type_name: Some("function".to_string()),
                    name: Some("test_tool".to_string()),
                    arguments: Some("{}".to_string()),
                };
                yield ChatStreamEvent::Done {
                    finish_reason: Some("stop".to_string()),
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
        Ok("tool_result".to_string())
    }

    #[derive(Debug, Clone, PartialEq, Eq, Hash, GraphLabel)]
    enum TestLabel {
        Llm,
        Tool,
    }

    #[expect(unused)]
    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    enum TestBranch {
        Default,
    }

    #[tokio::test]
    async fn llm_and_tool_nodes_work_in_state_graph() {
        let mut sg: StateGraph<MessagesState, ReActAgentError, TestBranch, ChatStreamEvent> =
            StateGraph::from_entry(TestLabel::Llm);

        let tool = test_tool_tool();

        let (tool_specs, tools) = parse_tool(vec![tool]);

        let llm_node = LlmNode::new(TestModel, tool_specs);

        let tool_node = ToolNode::new(tools);

        sg.add_node(TestLabel::Llm, llm_node);
        sg.add_node(TestLabel::Tool, tool_node);

        sg.add_edge(TestLabel::Llm, TestLabel::Tool);

        let initial = MessagesState::new(vec![Message::user("hello")]);

        let (final_state, final_label) = sg.run_until_stuck(initial, 10).await.unwrap();

        assert_eq!(final_label, TestLabel::Tool.intern());
        assert_eq!(final_state.llm_calls, 1);
        assert_eq!(final_state.messages.len(), 3);

        match &final_state.messages[0] {
            Message::User { .. } => {}
            _ => panic!("first message must be user"),
        }

        match &final_state.messages[1] {
            Message::Assistant { tool_calls, .. } => {
                assert!(tool_calls.is_some());
                assert_eq!(tool_calls.as_ref().unwrap().len(), 1);
            }
            _ => panic!("second message must be assistant"),
        }

        match &final_state.messages[2] {
            Message::Tool { .. } => {}
            _ => panic!("third message must be tool"),
        }
    }
}
