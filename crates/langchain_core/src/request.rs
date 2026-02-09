//! 暂时使用的是OpenAI的标准，后面对接其他标准再进行合并配置

use std::{collections::HashMap, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::message::Message;

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct RequestBody {
    /// 模型名称
    pub model: String,

    /// 聊天消息列表
    pub messages: Vec<Arc<Message>>,

    /// 采样温度，范围为0到2或者更高，默认值为1.0
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// 是否开启流式响应，默认值为false
    #[serde(skip_serializing_if = "is_false")]
    pub stream: bool,

    /// 核采样参数，范围为0到1，默认值为1.0
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    /// 限制一次请求中模型生成 completion 的最大 token 数。输入 token 和输出 token 的总长度受模型的上下文长度的限制。
    /// 默认值由模型决定
    /// # DeepSeek..etc
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// 最大完成令牌数，完成生成的令牌数量的上限，包括可见的输出令牌和推理令牌。
    /// # OpenAI
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,

    /// 指定响应格式
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,

    /// 工具列表，指定允许模型调用的工具
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSpec>>,

    /// 控制模型调用 tool 的行为。
    /// - `none` 意味着模型不会调用任何 tool，而是生成一条消息。
    /// - `auto` 意味着模型可以选择生成一条消息或调用一个或多个 tool。
    /// - `required` 意味着模型必须调用一个或多个 tool。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,

    /// 是否返回所输出 token 的对数概率。
    /// 如果为 true，则在 `message` 的 `content` 中返回每个输出 token 的对数概率。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<u32>,

    /// 一个介于 0 到 20 之间的整数 N，指定每个输出位置返回输出概率 top N 的 token，且返回这些 token 的对数概率。指定此参数时，logprobs 必须为 true。
    /// - `OpenAI`解释为：一个介于0和20之间的整数，指定在每个标记位置返回的最有可能的标记数量，每个标记都有一个相关的对数概率。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_logprobs: Option<u32>,

    /// 流式选项，只有在 stream 参数为 true 时，才可设置此参数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,

    /// 停止生成的标记序列
    /// - OpenAI: 最多 4 个序列，API 将停止生成后续标记。返回的文本将不包含停止序列。
    /// - DeepSeek: 一个 string 或最多包含 16 个 string 的 list，在遇到这些词时，API 将停止生成更多的 token。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<String>,

    /// 频率惩罚参数，范围为-2.0到2.0，默认值为0.0
    /// 如果该值为正，那么新 token 会根据其在已有文本中的出现频率受到相应的惩罚，降低模型重复相同内容的可能性。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,

    /// 存在惩罚参数，范围为-2.0到2.0，默认值为0.0
    /// 如果该值为正，那么新 token 会根据其是否已在已有文本中出现受到相应的惩罚，从而增加模型谈论新主题的可能性。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,

    /// 额外参数，当本结构体中没有包含特定的参数时，使用此参数传递额外的参数。
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl RequestBody {
    /// 创建一个新的请求体，指定模型名称。
    ///
    /// # 示例
    /// ```
    /// use langchain_core::request::RequestBody;
    /// let req = RequestBody::from_model("gpt-3.5-turbo");
    /// ```
    pub fn from_model<T: Into<String>>(model: T) -> Self {
        Self {
            model: model.into(),
            ..Default::default()
        }
    }

    /// 添加消息列表，用于指定模型消息的输入。
    ///
    /// # 示例
    /// ```
    /// use std::sync::Arc;
    /// use langchain_core::{request::RequestBody, message::Message};
    /// let req = RequestBody::from_model("gpt-3.5-turbo")
    ///     .with_messages(vec![Arc::new(Message::user("你好"))]);
    /// ```
    pub fn with_messages(mut self, messages: Vec<Arc<Message>>) -> Self {
        self.messages.extend(messages);
        self
    }

    /// 添加工具列表，用于指定模型调用的工具。
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        if !tools.is_empty() {
            self.tools = Some(tools);
        }
        self
    }

    /// 添加响应格式，用于指定模型响应的格式。
    ///
    /// # 示例
    /// ```
    /// use langchain_core::request::{RequestBody, ResponseFormat, FormatType};
    /// let req = RequestBody::from_model("gpt-3.5-turbo")
    ///     .with_response_format(ResponseFormat::json_object());
    ///
    /// assert_eq!(req.response_format, Some(ResponseFormat::json_object()));
    /// ```
    pub fn with_response_format(mut self, format: ResponseFormat) -> Self {
        self.response_format = Some(format);
        self
    }

    /// 可用于运行时检查冲突参数，确保不会与核心字段冲突。
    #[expect(unused)]
    const CORE_FIELDS: &'static [&'static str] = &[
        "model",
        "messages",
        "temperature",
        "top_p",
        "stream",
        "max_tokens",
    ];

    /// 添加额外的参数，当本结构体中没有包含特定的参数时，使用此参数传递额外的参数。
    /// **不要用此方法设置已存在的字段，否则会导致序列化时字段重复，行为未定义。**
    /// # 示例
    /// ```
    /// use langchain_core::request::RequestBody;
    /// use serde_json::Value;
    ///
    /// let req = RequestBody::from_model("gpt-4o")
    ///     .with_extra_param("xxxxx", 42);
    ///
    /// // 序列化为 JSON
    /// let json_str = serde_json::to_string(&req).unwrap();
    /// let parsed: Value = serde_json::from_str(&json_str).unwrap();
    ///
    /// assert_eq!(parsed.get("xxxxx").and_then(|v| v.as_i64()), Some(42));
    /// ```
    pub fn with_extra_param<T: Into<String>, U: Into<Value>>(mut self, key: T, value: U) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }

    /// 添加多个额外的参数，当本结构体中没有包含特定的参数时，使用此参数一次性传递多个额外的参数。
    ///
    /// # 示例
    /// ```
    /// use langchain_core::request::RequestBody;
    /// use serde_json::Value;
    ///
    /// let req = RequestBody::from_model("gpt-3.5-turbo")
    ///     .with_extra_params(vec![("temperature", 0.7), ("top_p", 0.9)]);
    ///
    /// // 序列化为 JSON
    /// let json_str = serde_json::to_string(&req).unwrap();
    /// let parsed: Value = serde_json::from_str(&json_str).unwrap();
    ///
    /// assert_eq!(parsed.get("temperature").and_then(|v| v.as_f64()), Some(0.7));
    /// assert_eq!(parsed.get("top_p").and_then(|v| v.as_f64()), Some(0.9));
    /// ```
    pub fn with_extra_params<I, K, V>(mut self, params: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<Value>,
    {
        for (k, v) in params {
            self.extra.insert(k.into(), v.into());
        }
        self
    }
}

/// 响应格式，用于指定模型响应的格式。
#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub format_type: FormatType,

    /// 可选的 JSON Schema 字符串，用于定义响应格式的结构，
    /// 仅在 format_type 为 `json_schema` 时使用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<String>,
}

impl ResponseFormat {
    /// 创建一个 JSON 对象格式的响应格式。
    ///
    /// # 示例
    /// ```
    /// use langchain_core::request::ResponseFormat;
    /// let format = ResponseFormat::json_object();
    /// ```
    pub fn json_object() -> Self {
        Self {
            format_type: FormatType::JsonObject,
            ..Default::default()
        }
    }

    /// 创建一个 JSON Schema 格式的响应格式。
    ///
    /// # 示例
    /// ```
    /// use langchain_core::request::ResponseFormat;
    /// let format = ResponseFormat::json_schema("{}".to_string());
    /// ```
    pub fn json_schema(schema: String) -> Self {
        Self {
            format_type: FormatType::JsonSchema,
            json_schema: Some(schema),
        }
    }
}

/// 流式选项，只有在 stream 参数为 true 时，才可设置此参数。
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct StreamOptions {
    /// 如果设置为 true，在流式消息最后的 data: [DONE] 之前将会传输一个额外的块。
    /// 此块上的 usage 字段显示整个请求的 token 使用统计信息，而 choices 字段将始终是一个空数组。
    /// 所有其他块也将包含一个 usage 字段，但其值为 null。
    pub include_usage: bool,

    /// 当为真时，将启用流混淆。
    /// 流混淆在流增量事件上添加随机字符到混淆字段，以规范化有效负载大小，作为对某些侧信道攻击的一种缓解措施。
    /// 这些混淆字段默认包含，但会增加数据流的少量开销。
    /// 如果您信任应用程序与 OpenAI API 之间的网络链接，您可以将 include_obfuscation 设置为 false，以优化带宽。
    pub include_obfuscation: bool,
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq, Hash, Clone, Copy)]
pub enum FormatType {
    #[serde(rename = "json_object")]
    JsonObject,
    #[serde(rename = "json_schema")]
    JsonSchema,
    #[default]
    #[serde(rename = "text")]
    Text,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ToolSpec {
    #[serde(rename = "function")]
    Function { function: ToolFunction },
}

impl ToolSpec {
    pub fn function_name(&self) -> &str {
        match self {
            ToolSpec::Function { function } => &function.name,
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

mod test {

    #[test]
    fn test_with_extra_param() {
        use super::*;
        let req = RequestBody::from_model("gpt-4o").with_extra_param("xxxxx", 42);

        let json_str = serde_json::to_string(&req).unwrap();
        println!("{}", json_str);
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        println!("{:?}", parsed);
        assert_eq!(parsed.get("xxxxx").and_then(|v| v.as_i64()), Some(42));
    }
}
