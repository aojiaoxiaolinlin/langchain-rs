use serde::{Deserialize, Serialize};

use crate::message::Message;

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct RequestBody {
    /// 模型名称
    pub model: String,

    /// 聊天消息列表
    pub messages: Vec<Message>,

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

    /// 指定响应格式TODO:
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,

    /// 工具列表，指定允许模型调用的工具TODO:
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,

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
}

impl RequestBody {
    pub fn from_model<T: Into<String>>(model: T) -> Self {
        Self {
            model: model.into(),
            ..Default::default()
        }
    }

    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages.extend(messages);
        self
    }

    pub fn with_response_format(mut self, format: ResponseFormat) -> Self {
        self.response_format = Some(format);
        self
    }
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub format_type: FormatType,

    /// 可选的 JSON Schema 字符串，用于定义响应格式的结构，
    /// 仅在 format_type 为 `json_schema` 时使用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<String>,
}

impl ResponseFormat {
    pub fn json_object() -> Self {
        Self {
            format_type: FormatType::JsonObject,
            ..Default::default()
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

#[derive(Serialize, Deserialize, Debug, Default)]
pub enum FormatType {
    #[serde(rename = "json_object")]
    JsonObject,
    #[serde(rename = "json_schema")]
    JsonSchema,
    #[default]
    #[serde(rename = "text")]
    Text,
}

fn is_false(value: &bool) -> bool {
    !*value
}
