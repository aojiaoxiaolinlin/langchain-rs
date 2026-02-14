use langchain::{
    ReactAgent, define_middleware_label,
    node::middleware::{AgentHook, AgentMiddleware},
};
use langchain_core::{message::Message, state::MessagesState};
use langchain_openai::ChatOpenAIBuilder;
use langgraph::node::NodeContext;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smallvec::smallvec;
use std::{env, sync::Arc};
use tracing_subscriber::EnvFilter;

const BASE_URL: &str = "https://api.siliconflow.cn/v1";
const MODEL: &str = "deepseek-ai/DeepSeek-V3.2";

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct NameResult {
    #[schemars(description = "用户的姓名")]
    name: String,
    #[schemars(description = "用户的年龄")]
    age: u8,
}

#[tokio::main]
async fn main() {
    let filter = EnvFilter::new("agent_middleware=DEBUG,langchain=DEBUG,langgraph=DEBUG");
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_env_filter(filter)
        .pretty()
        .init();

    let api_key = env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let model = ChatOpenAIBuilder::from_base(MODEL, BASE_URL, api_key.as_str()).build();

    let middleware = AgentMiddleware::from_label(define_middleware_label!(TestMiddleware))
        .with_before_agent(AgentHook {
            handler: Arc::new(|state: &MessagesState, _: &NodeContext| {
                tracing::info!("before_agent: {:?}", state);
                Box::pin(async move { Ok(MessagesState::default()) })
            }),
            target: None,
            branches: smallvec![],
        })
        .with_before_model(AgentHook {
            handler: Arc::new(|state: &MessagesState, _: &NodeContext| {
                tracing::info!("before_model: {:?}", state);
                Box::pin(async move { Ok(MessagesState::default()) })
            }),
            target: None,
            branches: smallvec![],
        })
        .with_after_model(AgentHook {
            handler: Arc::new(|state: &MessagesState, _: &NodeContext| {
                tracing::info!("after_model: {:?}", state);
                Box::pin(async move { Ok(MessagesState::default()) })
            }),
            target: None,
            branches: smallvec![],
        })
        .with_after_agent(AgentHook {
            handler: Arc::new(|state: &MessagesState, _: &NodeContext| {
                tracing::info!("after_agent: {:?}", state);
                Box::pin(async move { Ok(MessagesState::default()) })
            }),
            target: None,
            branches: smallvec![],
        });

    let agent = ReactAgent::builder(model)
        .with_system_prompt(r#"分析用户的问题，提取出用户的姓名和年龄，使用json格式返回 如 {"name": "张三", "age": 18}"#)
        .with_middlewares([middleware])
        .build();

    let result = agent
        .invoke_structured::<NameResult>(Message::user("我叫哈基米，我今年25岁"), None)
        .await
        .unwrap();

    println!("{:?}", result.struct_output);
}
