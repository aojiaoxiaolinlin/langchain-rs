//! 细粒度错误处理体系
//!
//! 提供类型安全的错误层次结构，支持程序化错误处理和自动重试逻辑。

use std::error::Error;
use thiserror::Error;

/// 错误类别，用于程序化错误处理
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// 临时性网络/IO问题（可重试）
    Transient,
    /// 无效输入或配置（不可重试）
    Validation,
    /// 认证/授权失败（不可重试）
    Authentication,
    /// 速率限制（可重试）
    RateLimit,
    /// 内部bug（不可重试）
    Internal,
    /// 外部服务错误
    External,
}

/// 基础错误trait，所有langchain错误都应该实现
pub trait LangChainError: std::error::Error + Send + Sync + 'static {
    /// 获取错误类别
    fn category(&self) -> ErrorCategory;

    /// 判断是否可重试
    fn is_retryable(&self) -> bool {
        matches!(
            self.category(),
            ErrorCategory::Transient | ErrorCategory::RateLimit
        )
    }

    /// 建议的重试延迟（毫秒）
    fn retry_delay_ms(&self) -> Option<u64> {
        None
    }
}

/// 模型相关错误
#[derive(Debug, Error)]
pub enum ModelError {
    #[error("API request failed: {0}")]
    RequestFailed(#[source] reqwest::Error),

    #[error("Rate limited: retry after {0} seconds")]
    RateLimited(u32),

    #[error("Invalid API key")]
    InvalidApiKey,

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Timeout after {0}ms")]
    Timeout(u64),

    #[error("Parse error: {0}")]
    ParseError(#[source] serde_json::Error),

    #[error("Response error: {0}")]
    ResponseError(String),

    #[error("Other error: {0}")]
    Other(#[source] Box<dyn Error + Send + Sync>),
}

impl LangChainError for ModelError {
    fn category(&self) -> ErrorCategory {
        match self {
            ModelError::RequestFailed(_) => ErrorCategory::Transient,
            ModelError::RateLimited(_) => ErrorCategory::RateLimit,
            ModelError::InvalidApiKey => ErrorCategory::Authentication,
            ModelError::ModelNotFound(_) => ErrorCategory::Validation,
            ModelError::Timeout(_) => ErrorCategory::Transient,
            ModelError::ParseError(_) => ErrorCategory::Internal,
            ModelError::ResponseError(_) => ErrorCategory::External,
            ModelError::Other(_) => ErrorCategory::Internal,
        }
    }

    fn retry_delay_ms(&self) -> Option<u64> {
        match self {
            ModelError::RateLimited(seconds) => Some((*seconds as u64) * 1000),
            ModelError::Timeout(_) => Some(1000),
            ModelError::RequestFailed(_) => Some(2000),
            _ => None,
        }
    }
}

/// 工具执行错误（保持向后兼容）
#[derive(Debug, Error)]
pub enum ToolError {
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("tool call error: {0}")]
    ToolCall(#[source] Box<dyn Error + Send + Sync>),

    // 新增变体
    #[error("Tool not found: {0}")]
    NotFound(String),

    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("Tool execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Tool timeout: {0}")]
    Timeout(String),

    #[error("Tool error: {0}")]
    ToolError(String),
}

impl ToolError {
    pub fn tool_call<E>(e: E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        Self::ToolCall(Box::new(e))
    }
}

impl LangChainError for ToolError {
    fn category(&self) -> ErrorCategory {
        match self {
            ToolError::Json(_) => ErrorCategory::Internal,
            ToolError::ToolCall(_) => ErrorCategory::External,
            ToolError::NotFound(_) => ErrorCategory::Validation,
            ToolError::InvalidArguments(_) => ErrorCategory::Validation,
            ToolError::ExecutionFailed(_) => ErrorCategory::External,
            ToolError::Timeout(_) => ErrorCategory::Transient,
            ToolError::ToolError(_) => ErrorCategory::Internal,
        }
    }

    fn retry_delay_ms(&self) -> Option<u64> {
        match self {
            ToolError::Timeout(_) => Some(1000),
            ToolError::ExecutionFailed(_) => Some(2000),
            _ => None,
        }
    }
}

/// 图执行错误
#[derive(Debug, Error)]
pub enum GraphError {
    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Edge not found: {0}")]
    EdgeNotFound(String),

    #[error("Cycle detected in graph")]
    CycleDetected,

    #[error("Invalid state transition")]
    InvalidTransition,

    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Max steps exceeded")]
    MaxStepsExceeded,
}

impl LangChainError for GraphError {
    fn category(&self) -> ErrorCategory {
        match self {
            GraphError::NodeNotFound(_) => ErrorCategory::Validation,
            GraphError::EdgeNotFound(_) => ErrorCategory::Validation,
            GraphError::CycleDetected => ErrorCategory::Validation,
            GraphError::InvalidTransition => ErrorCategory::Validation,
            GraphError::ExecutionFailed(_) => ErrorCategory::Internal,
            GraphError::MaxStepsExceeded => ErrorCategory::Validation,
        }
    }
}

/// 存储错误
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Deserialization error: {0}")]
    Deserialization(String),

    #[error("Storage backend error: {0}")]
    Backend(String),

    #[error("Key not found: {0}")]
    KeyNotFound(String),

    #[error("Storage full")]
    StorageFull,
}

impl LangChainError for StorageError {
    fn category(&self) -> ErrorCategory {
        match self {
            StorageError::Io(_) => ErrorCategory::Transient,
            StorageError::Serialization(_) => ErrorCategory::Internal,
            StorageError::Deserialization(_) => ErrorCategory::Internal,
            StorageError::Backend(_) => ErrorCategory::External,
            StorageError::KeyNotFound(_) => ErrorCategory::Validation,
            StorageError::StorageFull => ErrorCategory::Transient,
        }
    }

    fn retry_delay_ms(&self) -> Option<u64> {
        match self {
            StorageError::Io(_) => Some(1000),
            StorageError::StorageFull => Some(5000),
            _ => None,
        }
    }
}

/// 验证错误
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    #[error("Out of range: {0}")]
    OutOfRange(String),
}

impl LangChainError for ValidationError {
    fn category(&self) -> ErrorCategory {
        ErrorCategory::Validation
    }
}

/// 重试配置
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: usize,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 10000,
            backoff_multiplier: 2.0,
        }
    }
}

/// 简单的重试逻辑
pub async fn retry_with_backoff<F, T, E, Fut>(
    mut operation: F,
    error_category: impl Fn(&E) -> ErrorCategory,
    config: &RetryConfig,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    let mut delay = std::time::Duration::from_millis(config.initial_delay_ms);

    for attempt in 0..=config.max_retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                let category = error_category(&e);

                if !matches!(
                    category,
                    ErrorCategory::Transient | ErrorCategory::RateLimit
                ) {
                    return Err(e);
                }

                if attempt == config.max_retries {
                    return Err(e);
                }

                // 使用建议的延迟或指数退避
                let sleep_duration = delay;
                tokio::time::sleep(sleep_duration).await;

                // 计算下一次延迟
                delay = std::time::Duration::from_millis(
                    (delay.as_millis() as f32 * config.backoff_multiplier) as u64,
                );
                delay = std::cmp::min(delay, std::time::Duration::from_millis(config.max_delay_ms));
            }
        }
    }

    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_error_categories() {
        let rate_limited = ModelError::RateLimited(60);
        assert_eq!(rate_limited.category(), ErrorCategory::RateLimit);
        assert!(rate_limited.is_retryable());
        assert_eq!(rate_limited.retry_delay_ms(), Some(60000));

        let invalid_key = ModelError::InvalidApiKey;
        assert_eq!(invalid_key.category(), ErrorCategory::Authentication);
        assert!(!invalid_key.is_retryable());

        let timeout = ModelError::Timeout(5000);
        assert_eq!(timeout.category(), ErrorCategory::Transient);
        assert!(timeout.is_retryable());
        assert_eq!(timeout.retry_delay_ms(), Some(1000));
    }

    #[test]
    fn test_tool_error_categories() {
        let not_found = ToolError::NotFound("test_tool".to_owned());
        assert_eq!(not_found.category(), ErrorCategory::Validation);
        assert!(!not_found.is_retryable());

        let timeout = ToolError::Timeout("test_tool".to_owned());
        assert_eq!(timeout.category(), ErrorCategory::Transient);
        assert!(timeout.is_retryable());
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 10000);
        assert_eq!(config.backoff_multiplier, 2.0);
    }
}
