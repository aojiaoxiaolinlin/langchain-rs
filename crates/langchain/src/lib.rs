use std::collections::HashMap;

use async_trait::async_trait;
use futures::future::try_join_all;
use langchain_core::{
    message::Message,
    state::{ChatCompletion, ChatModel, MessagesState, RegisteredTool},
};
use langgraph::node::Node;
use serde_json::Value;

pub struct LlmNode<M, E>
where
    M: ChatModel<Error = E> + 'static,
    E: Send + Sync + 'static,
{
    pub model: M,
}

impl<M, E> LlmNode<M, E>
where
    M: ChatModel<Error = E> + 'static,
    E: Send + Sync + 'static,
{
    pub fn new(model: M) -> Self {
        Self { model }
    }
}

#[async_trait]
impl<M, E> Node<MessagesState, MessagesState, E> for LlmNode<M, E>
where
    M: ChatModel<Error = E> + Send + Sync + 'static,
    E: Send + Sync + 'static,
{
    async fn run(&self, input: &MessagesState) -> Result<MessagesState, E> {
        let mut next = input.clone();
        let completion: ChatCompletion = self.model.invoke(&next.messages).await?;
        next.extend_messages(completion.messages);
        next.increment_llm_calls();
        Ok(next)
    }
}

pub struct ToolNode<E>
where
    E: Send + Sync + 'static,
{
    pub tools: HashMap<String, RegisteredTool<E>>,
}

impl<E> ToolNode<E>
where
    E: Send + Sync + 'static,
{
    pub fn new(tools: HashMap<String, RegisteredTool<E>>) -> Self {
        Self { tools }
    }
}

#[async_trait]
impl<E> Node<MessagesState, MessagesState, E> for ToolNode<E>
where
    E: Send + Sync + 'static,
{
    async fn run(&self, input: &MessagesState) -> Result<MessagesState, E> {
        let mut next = input.clone();
        if let Some(calls) = input.last_tool_calls() {
            let mut futures = Vec::new();
            let mut ids = Vec::new();
            for call in calls {
                if let Some(tool) = self.tools.get(call.function_name()) {
                    ids.push(call.id().to_string());
                    futures.push((tool.handler)(call.function.arguments.clone()));
                }
            }
            let results: Vec<Value> = try_join_all(futures).await?;
            for (id, value) in ids.into_iter().zip(results.into_iter()) {
                next.push_message(Message::tool(value.to_string(), id));
            }
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use langchain_core::{
        message::{FunctionCall, Message, ToolCall},
        request::ToolFunction,
        response::{TokensDetails, Usage},
        state::{RegisteredTool, ToolFuture},
    };
    use langgraph::{graph::Graph, label::GraphLabel, state_graph::StateGraph};

    #[derive(Debug)]
    enum TestError {
        #[expect(unused)]
        Model,
        #[expect(unused)]
        Tool,
    }

    #[derive(Debug)]
    struct TestModel;

    #[async_trait]
    impl ChatModel for TestModel {
        type Error = TestError;

        async fn invoke(&self, _messages: &[Message]) -> Result<ChatCompletion, Self::Error> {
            let call = ToolCall {
                id: "call1".to_string(),
                type_name: "function".to_string(),
                function: FunctionCall {
                    name: "test_tool".to_string(),
                    arguments: serde_json::Value::Null,
                },
            };
            let msg = Message::Assistant {
                content: "assistant".to_string(),
                tool_calls: Some(vec![call]),
                name: None,
            };

            let usage = Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                completion_tokens_details: TokensDetails {
                    reasoning_tokens: 0,
                },
            };
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

    async fn test_tool_fn(_args: Value) -> Result<Value, TestError> {
        Ok(Value::String("tool_result".to_string()))
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
        let mut graph: Graph<MessagesState, MessagesState, TestError, TestBranch> = Graph {
            nodes: HashMap::new(),
        };

        let llm_node = LlmNode::new(TestModel);

        let mut tools_map: HashMap<String, RegisteredTool<TestError>> = HashMap::new();
        let function = ToolFunction {
            name: "test_tool".to_string(),
            description: "test tool".to_string(),
            parameters: serde_json::json!({}),
        };
        let handler =
            Box::new(|args: Value| -> ToolFuture<TestError> { Box::pin(test_tool_fn(args)) });
        let tool = RegisteredTool { function, handler };
        tools_map.insert("test_tool".to_string(), tool);
        let tool_node = ToolNode::new(tools_map);

        graph.add_node(TestLabel::Llm, llm_node);
        graph.add_node(TestLabel::Tool, tool_node);

        graph.add_node_edge(TestLabel::Llm, TestLabel::Tool);

        let entry = TestLabel::Llm.intern();
        let sg: StateGraph<MessagesState, TestError, TestBranch> = StateGraph::new(entry, graph);

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
