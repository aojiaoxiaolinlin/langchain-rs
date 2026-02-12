use langchain::ReactAgent;
use langchain_core::message::Message;
use langchain_openai::ChatOpenAIBuilder;
use langgraph::checkpoint::checkpoint_struct_api::{RedisSaver, RedisSaverConfig};
use std::{env, sync::Arc};
use tracing_subscriber::EnvFilter;

const BASE_URL: &str = "https://api.siliconflow.cn/v1";
const MODEL: &str = "deepseek-ai/DeepSeek-V3.2";

#[tokio::main]
async fn main() {
    let filter = EnvFilter::new("agent_struct=DEBUG,langchain=DEBUG,langgraph=DEBUG");
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_env_filter(filter)
        .pretty()
        .init();

    let api_key = env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
// 注意，运行此示例会在 database_url 的数据中创建名为 langchain_rs_checkpoint 的键，如果冲突则会失败
    let database_url = env::var("LANGCHAIN_RS_REDIS_DATABASE_URL")
        .expect("LANGCHAIN_RS_REDIS_DATABASE_URL must be set");

    let model = ChatOpenAIBuilder::from_base(MODEL, BASE_URL, api_key.as_str()).build();

    let checkpointer = Arc::new(
        RedisSaver::new(RedisSaverConfig::new(database_url))
            .await
            .unwrap(),
    );

    let agent = ReactAgent::builder(model)
        .with_checkpointer(checkpointer)
        .with_system_prompt("你是一个聊天AI助手，回答要简洁有趣，要遵循 user 命令")
        .build();

    let result = agent
        .invoke(Message::user("我给你取名叫老大！"), Some("redis001"))
        .await
        .unwrap();

    println!("{}", result.last_message().unwrap().to_pretty());

    let result = agent
        .invoke(Message::user("你叫什么名字？"), Some("redis001"))
        .await
        .unwrap();

    println!("{}", result.last_message().unwrap().to_pretty());
}
