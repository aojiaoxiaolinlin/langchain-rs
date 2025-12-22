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
async fn add(a: i32, b: i32) -> Result<i32, serde_json::Error> {
    Ok(a + b)
}

#[tool(
    description = "subtract two numbers",
    args(a = "first number", b = "second number")
)]
async fn subtract(a: i32, b: i32) -> Result<i32, serde_json::Error> {
    Ok(a - b)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let api_key = env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let model = ChatOpenAIBuilder::from_base(MODEL, BASE_URL, api_key.as_str()).build();

    let agent = ReactAgent::create_agent(model, vec![add_tool(), subtract_tool()]);

    let state = agent
        .invoke(Message::user("计算100和200的和，并且计算999减去800的差"))
        .await
        .unwrap();

    println!("{:?}", state.messages.last().unwrap());
}
