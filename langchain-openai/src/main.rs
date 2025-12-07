use langchain_openai::{message::Message, request::RequestBody};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};

const BASE_URL: &str = "https://api.siliconflow.cn/v1";
const API_KEY: &str = "sk-omjozmoodyqubrrqqnssxwgvmqgkqwwlwdncbohytkvogegl";
const MODEL: &str = "deepseek-ai/DeepSeek-V3.2";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {API_KEY}"))?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let body = RequestBody::from_model(MODEL).with_messages(vec![Message::user("您好")]);
    println!("请求Body:\n{}", serde_json::to_string_pretty(&body)?);

    let client = reqwest::Client::new();

    let response = client
        .post(format!("{BASE_URL}/chat/completions"))
        .headers(headers)
        .json(&body)
        .send()
        .await?
        // .json::<ResponseBody>()
        .text()
        .await?;

    println!("Body:\n{:?}", response);
    Ok(())
}
