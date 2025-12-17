use langchain::{LlmModel, ReactAgent, Tool, tool, tools_from_fns};
use langchain_core::{
    message::Message,
    request::ToolSpec,
    state::{MessageDiff, MessageState},
};
use langgraph::node::NodeRunError;

#[derive(Clone, Default)]
struct FakeModel {
    tools: Vec<ToolSpec>,
}

#[async_trait::async_trait]
impl LlmModel for FakeModel {
    async fn invoke(&self, state: &MessageState) -> Result<MessageDiff, NodeRunError> {
        let last = state.messages.last().cloned().unwrap();
        match last {
            Message::User { .. } => {
                let tool_args = serde_json::json!({
                    "a": 1.0_f64,
                    "b": 2.0_f64,
                })
                .to_string();
                let function = langchain_core::message::FunctionCall {
                    name: "add".to_string(),
                    arguments: serde_json::Value::String(tool_args),
                };
                let tool_call = langchain_core::message::ToolCall {
                    id: "call-add-1".to_string(),
                    type_name: "function".to_string(),
                    function,
                };
                let assistant = Message::Assistant {
                    content: "call tool".to_string(),
                    tool_calls: Some(vec![tool_call]),
                    name: None,
                };
                Ok(MessageDiff {
                    new_messages: vec![assistant],
                    llm_calls_delta: 1,
                })
            }
            _ => Ok(MessageDiff {
                new_messages: vec![Message::assistant("done")],
                llm_calls_delta: 1,
            }),
        }
    }

    fn bind_tools(&mut self, tools: &[ToolSpec]) {
        self.tools = tools.to_vec();
    }
}

#[tool(description = "计算两个数的和", args(a = "第一个数", b = "第二个数"))]
async fn add(a: f64, b: f64) -> Result<f64, NodeRunError> {
    Ok(a + b)
}

#[tokio::test]
async fn react_agent_executes_tool_and_updates_state() {
    let model = FakeModel::default();
    let tools = tools_from_fns!(add);
    let mut agent = ReactAgent::create_agent_with_tools(model, tools);

    let state = agent
        .invoke(Message::user("calculate 1+2"))
        .await
        .expect("agent should run");

    assert!(!state.messages.is_empty());
}
