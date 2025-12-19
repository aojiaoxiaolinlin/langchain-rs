use std::sync::Arc;

use langchain::{
    AgentMiddleware, DynAgentMiddleware, ReactAgentBuilder, Tool, tool, tools_from_fns,
};
use langchain_core::{message::Message, request::ToolSpec, state::MessageState};
use langchain_openai::ChatOpenAi;
use langgraph::node::NodeRunError;

const BASE_URL: &str = "https://api.siliconflow.cn/v1";
const API_KEY: &str = "";
const MODEL: &str = "deepseek-ai/DeepSeek-V3.2";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let model = ChatOpenAi::new(BASE_URL, API_KEY, MODEL);
    let agent = ReactAgentBuilder::new(model)
        .with_tools(tools_from_fns!(get_weather, add, sub))
        .with_system_prompt(
            "你是一个智能助手，你可以调用工具来完成任务,如果有多个任务它们毫无依赖关系,你可以并行调用多个工具"
        )
        .with_middleware(logging_middleware())
        .build();

    let result = agent
        .invoke(Message::user(
            "查询成都天气情况,并且计算100+200,并且计算100-200",
        ))
        .await?;

    let message = result
        .messages
        .last()
        .ok_or(anyhow::anyhow!("No message found"))?;
    tracing::info!("运行结束{:?}", message);

    Ok(())
}

#[tool(
    description = "获取指定城市的天气信息",
    args(location = "城市名称，例如 '成都'")
)]
async fn get_weather(location: String) -> Result<String, NodeRunError> {
    Ok(format!("Weather in {} is sunny", location))
}

#[tool(description = "计算两个数的和", args(a = "第一个数", b = "第二个数"))]
async fn add(a: f64, b: f64) -> Result<f64, NodeRunError> {
    Ok(a + b)
}

#[tool(description = "计算两个数的差", args(a = "第一个数", b = "第二个数"))]
async fn sub(state: &MessageState, a: f64, b: f64) -> Result<f64, NodeRunError> {
    tracing::info!("当前消息长度: {:?}", state.messages.len());
    Ok(a - b)
}

struct LoggingMiddleware;

impl AgentMiddleware for LoggingMiddleware {
    fn before_run(&self, state: &MessageState) {
        println!("before_run, messages: {}", state.messages.len());
    }

    fn after_run(&self, state: &MessageState) {
        println!("after_run, messages: {}", state.messages.len());
    }

    fn before_model(&self, _state: &MessageState, tools: &[ToolSpec]) {
        println!("before_model, tools: {}", tools.len());
    }

    fn before_tool(&self, _state: &MessageState, tool_name: &str) {
        println!("before_tool: {tool_name}");
    }

    fn after_tool(&self, _state: &MessageState, tool_name: &str) {
        println!("after_tool: {tool_name}");
    }
}

fn logging_middleware() -> DynAgentMiddleware {
    Arc::new(LoggingMiddleware)
}
