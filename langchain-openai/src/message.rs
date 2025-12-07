use serde::{Deserialize, Serialize};

/// 聊天消息，表示不同角色的消息类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum Message {
    /// 用户消息
    #[serde(rename = "user")]
    User {
        /// 消息内容
        content: String,
        /// 可选填的参与者的名称，为模型提供信息以区分相同角色的参与者
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// AI助手消息
    #[serde(rename = "assistant")]
    Assistant {
        /// 消息内容
        content: String,
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
        content: String,
        tool_call_id: String,
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
            content: content.into(),
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
            content: content.into(),
            name: Some(name.into()),
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
}
