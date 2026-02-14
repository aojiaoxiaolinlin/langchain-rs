use langchain::ReactAgent;
use langchain_core::{message::Message, tool};
use langchain_openai::ChatOpenAIBuilder;
use std::env;

const BASE_URL: &str = "https://api.siliconflow.cn/v1";
const MODEL: &str = "deepseek-ai/DeepSeek-V3.2";

#[tool(
    description = "add two numbers",
    args(a = "first number", b = "second number")
)]
async fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[tool(
    description = "subtract two numbers",
    args(a = "first number", b = "second number")
)]
async fn subtract(a: i32, b: i32) -> i32 {
    a - b
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let api_key = env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let model = ChatOpenAIBuilder::from_base(MODEL, BASE_URL, api_key.as_str()).build();

    let agent = ReactAgent::builder(model)
        .with_tools([add_tool(), subtract_tool()])
        .with_system_prompt(
            "你是一个智能助手，你可以使用提供的工具来回答用户的问题。如果问题之间没有依赖关系，你可以并行执行多个工具。",
        )
        .build();

    let state = agent
        .invoke(
            Message::user("计算100和200的和，同时计算999减去800的差"),
            None,
        )
        .await
        .unwrap();

    if let Some(msg) = state.messages.last() {
        println!("{}", msg.to_pretty());
    }
}
