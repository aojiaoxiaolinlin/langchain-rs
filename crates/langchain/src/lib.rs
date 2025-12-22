use std::{collections::HashMap, error::Error, marker::PhantomData};

use async_trait::async_trait;
use futures::future::try_join_all;
use langchain_core::{
    message::Message,
    request::ToolSpec,
    state::{ChatCompletion, ChatModel, MessagesState, RegisteredTool, ToolFn},
};
use langgraph::{
    graph::GraphError,
    graph::GraphStepError,
    label::{BaseGraphLabel, GraphLabel},
};
use langgraph::{node::Node, state_graph::StateGraph};
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
impl<M, LE> Node<MessagesState, MessagesState, ReActAgentError> for LlmNode<M, LE>
where
    M: ChatModel<Error = LE> + Send + Sync + 'static,
    LE: Error + Send + Sync + 'static,
{
    async fn run(&self, input: &MessagesState) -> Result<MessagesState, ReActAgentError> {
        let mut next = input.clone();
        let completion: ChatCompletion = self
            .model
            .invoke(&next.messages, &self.tools)
            .await
            .map_err(|e| ReActAgentError::Model(Box::new(e)))?;
        tracing::debug!("LLM completion: {:?}", completion);
        next.extend_messages(completion.messages);
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
impl<E> Node<MessagesState, MessagesState, E> for IdentityNode<E>
where
    E: Send + Sync + 'static,
{
    async fn run(&self, input: &MessagesState) -> Result<MessagesState, E> {
        Ok(input.clone())
    }
}

#[async_trait]
impl<E> Node<MessagesState, MessagesState, ReActAgentError> for ToolNode<E>
where
    E: Error + Send + Sync + 'static,
{
    async fn run(&self, input: &MessagesState) -> Result<MessagesState, ReActAgentError> {
        let mut next = input.clone();
        if let Some(calls) = input.last_tool_calls() {
            let mut futures = Vec::new();
            let mut ids = Vec::new();
            tracing::debug!("Tool calls count: {}", calls.len());
            for call in calls {
                if let Some(handler) = self.tools.get(call.function_name()) {
                    ids.push(call.id().to_string());
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
    graph: StateGraph<MessagesState, ReActAgentError, ReactAgentBranch>,
    system_prompt: Option<String>,
}

impl ReactAgent {
    pub fn create_agent<M>(model: M, tools: Vec<RegisteredTool<ToolError>>) -> Self
    where
        M: ChatModel + Send + Sync + 'static,
        M::Error: Error + Send + Sync + 'static,
    {
        let (tool_specs, tools) = parse_tool(tools);

        let mut graph: StateGraph<MessagesState, ReActAgentError, ReactAgentBranch> =
            StateGraph::from_entry(BaseGraphLabel::Start);

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
        let (state, _) = self.graph.run_until_stuck(state, max_steps).await?;
        Ok(state)
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
            _messages: &[Message],
            _tools: &[ToolSpec],
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
            _messages: &[Message],
        ) -> Result<langchain_core::state::ChatStream<Self::Error>, Self::Error> {
            unimplemented!()
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
        let mut sg: StateGraph<MessagesState, ReActAgentError, TestBranch> =
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
