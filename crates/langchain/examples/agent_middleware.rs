use langchain::{
    ReactAgent, define_middleware_label,
    node::middleware::{AgentHook, AgentMiddleware},
};
use langchain_core::{message::Message, state::MessagesState, tool};
use langchain_openai::ChatOpenAIBuilder;
use langgraph::node::NodeContext;
use std::{env, sync::Arc};
use tracing_subscriber::EnvFilter;

const BASE_URL: &str = "https://api.siliconflow.cn/v1";
const MODEL: &str = "deepseek-ai/DeepSeek-V3.2";

#[tool(
    description = "add two numbers",
    args(a = "first number", b = "second number")
)]
async fn add(a: i32, b: i32) -> i32 {
    a + b
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
            branches: vec![],
        })
        .with_before_model(AgentHook {
            handler: Arc::new(|state: &MessagesState, _: &NodeContext| {
                tracing::info!("before_model: {:?}", state);
                Box::pin(async move { Ok(MessagesState::default()) })
            }),
            target: None,
            branches: vec![],
        })
        .with_after_model(AgentHook {
            handler: Arc::new(|state: &MessagesState, _: &NodeContext| {
                tracing::info!("after_model: {:?}", state);
                Box::pin(async move { Ok(MessagesState::default()) })
            }),
            target: None,
            branches: vec![],
        })
        .with_after_agent(AgentHook {
            handler: Arc::new(|state: &MessagesState, _: &NodeContext| {
                tracing::info!("after_agent: {:?}", state);
                Box::pin(async move { Ok(MessagesState::default()) })
            }),
            target: None,
            branches: vec![],
        });

    let agent = ReactAgent::builder(model)
        .with_system_prompt(r#"你是一个智能助手，你可以使用提供的工具来回答用户的问题。如果问题之间没有依赖关系，你可以并行执行多个工具。"#)
        .with_middlewares([middleware])
        .with_tool_middleware(Arc::new(Box::new(
            |state: &MessagesState, _ctx: &NodeContext, name, args, handler| {
                let state = state.clone();
                let name = name.to_string();
                Box::pin(async move {
                    tracing::info!("wrap_tool before: {} {:?}", name, args);
                    tracing::info!("Current state msg count: {}", state.messages.len());
                    let result = handler(args).await;
                    tracing::info!("wrap_tool after: {:?}", result);
                    result
                })
            },
        )))
        .with_tools([add_tool()])
        .build();

    let result = agent
        .invoke(Message::user("计算100和200的和"), None)
        .await
        .unwrap();

    println!("{:?}", result.last_message().unwrap().to_pretty());
}
