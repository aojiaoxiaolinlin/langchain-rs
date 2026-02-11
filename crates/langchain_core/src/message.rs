//! 暂时使用的是OpenAI的标准，后面对接其他标准再进行合并配置

use serde::{Deserialize, Serialize};

/// 聊天消息，表示不同角色的消息类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum Message {
    /// 用户消息
    #[serde(rename = "user")]
    User {
        /// 消息内容
        content: Content,
        /// 可选填的参与者的名称，为模型提供信息以区分相同角色的参与者
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// AI助手消息
    #[serde(rename = "assistant")]
    Assistant {
        /// 消息内容
        content: String,
        /// 可选填的工具调用列表
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCall>>,
        /// 可选填的消息名称
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// 系统消息
    #[serde(rename = "system")]
    System {
        /// 消息内容
        content: String,
        /// 可选填的消息名称
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// 开发者消息
    Developer {
        /// 消息内容
        content: String,
        /// 可选填的消息名称
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// 工具调用消息
    #[serde(rename = "tool")]
    Tool {
        // 必须有 id 来对应
        tool_call_id: String,
        content: String,
    },
}

impl Message {
    /// 创建一个用户消息
    /// # Arguments
    /// * `content` - 消息内容
    /// # Returns
    /// * `Message` - 用户消息实例
    pub fn user<S: Into<String>>(content: S) -> Self {
        Self::User {
            content: Content::Text(content.into()),
            name: None,
        }
    }

    /// 创建一个带名称的用户消息
    /// # Arguments
    /// * `content` - 消息内容
    /// * `name` - 参与者名称
    /// # Returns
    /// * `Message` - 用户消息实例
    pub fn user_with_name<S: Into<String>, N: Into<String>>(content: S, name: N) -> Self {
        Self::User {
            content: Content::Text(content.into()),
            name: Some(name.into()),
        }
    }

    /// 创建一个带内容块的用户消息
    /// # Arguments
    /// * `content` - 消息内容块
    /// # Returns
    /// * `Message` - 用户消息实例
    pub fn user_with_content_block(content: ContentBlock) -> Self {
        Self::User {
            content: Content::Mixed(vec![content]),
            name: None,
        }
    }

    /// 创建一个助手消息
    /// # Arguments
    /// * `content` - 消息内容
    /// # Returns
    /// * `Message` - 助手消息实例
    pub fn assistant<S: Into<String>>(content: S) -> Self {
        Self::Assistant {
            content: content.into(),
            tool_calls: None,
            name: None,
        }
    }

    /// 创建一个带名称的助手消息
    /// # Arguments
    /// * `content` - 消息内容
    /// * `name` - 消息名称
    /// # Returns
    /// * `Message` - 助手消息实例
    pub fn assistant_with_name<S: Into<String>, N: Into<String>>(content: S, name: N) -> Self {
        Self::Assistant {
            content: content.into(),
            tool_calls: None,
            name: Some(name.into()),
        }
    }

    /// 创建一个系统消息
    /// # Arguments
    /// * `content` - 消息内容
    /// # Returns
    /// * `Message` - 系统消息实例
    pub fn system<S: Into<String>>(content: S) -> Self {
        Self::System {
            content: content.into(),
            name: None,
        }
    }

    /// 创建一个开发者消息
    ///
    /// 详情：参考 https://platform.openai.com/docs/api-reference/chat/create#chat_create-messages
    /// # Arguments
    /// * `content` - 消息内容
    /// # Returns
    /// * `Message` - 开发者消息实例
    pub fn developer<S: Into<String>>(content: S) -> Self {
        Self::Developer {
            content: content.into(),
            name: None,
        }
    }

    /// 创建一个带名称的开发者消息
    ///
    /// 详情：参考 https://platform.openai.com/docs/api-reference/chat/create#chat_create-messages
    /// # Arguments
    /// * `content` - 消息内容
    /// * `name` - 消息名称
    /// # Returns
    /// * `Message` - 开发者消息实例
    pub fn developer_with_name<S: Into<String>, N: Into<String>>(content: S, name: N) -> Self {
        Self::Developer {
            content: content.into(),
            name: Some(name.into()),
        }
    }

    /// 创建一个工具调用消息
    /// # Arguments
    /// * `content` - 消息内容
    /// * `tool_call_id` - 工具调用ID
    /// # Returns
    /// * `Message` - 工具调用消息实例
    pub fn tool<S: Into<String>, I: Into<String>>(content: S, tool_call_id: I) -> Self {
        Self::Tool {
            content: content.into(),
            tool_call_id: tool_call_id.into(),
        }
    }

    /// 获取消息内容的文本形式
    pub fn content(&self) -> &str {
        match self {
            Message::User { content, .. } => match content {
                Content::Text(s) => s,
                _ => "",
            },
            Message::Assistant { content, .. } => content,
            Message::System { content, .. } => content,
            Message::Developer { content, .. } => content,
            Message::Tool { content, .. } => content,
        }
    }

    pub fn to_pretty(&self) -> String {
        match self {
            Message::User { content, .. } => match content {
                Content::Text(s) => format!(
                    "================================ Human Message =================================\n\n{s}\n\n"
                ),
                _ => "".to_owned(),
            },
            Message::Assistant {
                content,
                tool_calls,
                ..
            } => {
                if let Some(tool_calls) = tool_calls
                    && !tool_calls.is_empty()
                {
                    format!(
                        "================================ Ai Message =================================\nCall Tools:\n{:?}\n\n",
                        tool_calls
                    )
                } else {
                    format!(
                        "================================ Ai Message =================================\n\n{content}\n\n"
                    )
                }
            }
            Message::System { content, .. } => {
                format!(
                    "================================ System Message =================================\n\n{content}\n\n"
                )
            }
            Message::Developer { content, .. } => {
                format!(
                    "================================ Developer Message =================================\n\n{content}\n\n"
                )
            }
            Message::Tool {
                content,
                tool_call_id,
            } => {
                format!(
                    "================================ Tool Message =================================\n\nTool {}: {}\n\n",
                    tool_call_id, content
                )
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub function: FunctionCall,
}

impl ToolCall {
    pub fn function_name(&self) -> &str {
        &self.function.name
    }

    pub fn arguments(&self) -> Result<serde_json::Value, serde_json::Error> {
        match &self.function.arguments {
            serde_json::Value::String(value) => serde_json::from_str(value),
            value => Ok(value.clone()),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    // 将来可以扩展
    Image { url: String },
    // 或者对于 Agent，可以包含 ToolCalls
    Mixed(Vec<ContentBlock>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    // Claude 或 v1 格式的 reasoning block
    #[serde(rename = "reasoning")]
    Reasoning { content: String },
}
