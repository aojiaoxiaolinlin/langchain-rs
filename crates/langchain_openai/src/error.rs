use thiserror::Error;

/// OpenAI API 错误
#[derive(Debug, Error)]
pub enum OpenAIError {
    /// 无效的 API 密钥
    #[error("无效的 API 密钥")]
    InvalidApiKey,
    /// 无效的请求头
    #[error("无效的请求头值: {0}")]
    InvalidHeaderValue(String),
    /// 模型不存在
    #[error("模型不存在")]
    ModelNotFound,
    /// 超时错误    
    #[error("超时错误")]
    Timeout,
    /// 网络错误
    #[error("网络错误")]
    Http(reqwest::Error),
    /// 响应体解析错误
    #[error("响应体解析错误")]
    ResponseBodyParse(reqwest::Error),
    /// 其他错误
    #[error("其他错误: {0}")]
    Other(String),
}
