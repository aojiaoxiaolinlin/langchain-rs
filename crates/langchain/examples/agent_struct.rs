use langchain::ReactAgent;
use langchain_core::message::Message;
use langchain_openai::ChatOpenAIBuilder;
use langgraph::checkpoint::MemorySaver;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{env, sync::Arc};
use tracing_subscriber::EnvFilter;

const BASE_URL: &str = "https://api.siliconflow.cn/v1";
const MODEL: &str = "deepseek-ai/DeepSeek-V3.2";

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct NameResult {
    name: String,
    age: u8,
}

#[tokio::main]
async fn main() {
    let filter = EnvFilter::new("agent_struct=DEBUG,langchain=DEBUG,langgraph=DEBUG");
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_env_filter(filter)
        .pretty()
        .init();

    let api_key = env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let model = ChatOpenAIBuilder::from_base(MODEL, BASE_URL, api_key.as_str()).build();

    let checkpointer = Arc::new(MemorySaver::new());

    let agent = ReactAgent::builder(model)
        .with_checkpointer(checkpointer)
        .with_system_prompt(r#"分析用户的问题，提取出用户的姓名和年龄，使用json格式返回 如 {"name": "张三", "age": 18}"#)
        .build();

    let result = agent
        .invoke_structured::<NameResult>(Message::user("我叫哈基米，我今年25岁"), None)
        .await
        .unwrap();

    println!("{:?}", result.struct_output);
}
