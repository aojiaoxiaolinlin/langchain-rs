//! 日志记录中间件示例
//! 
//! 演示如何实现一个完整的日志记录中间件，包括：
//! - 节点执行前后的日志
//! - 流式事件日志
//! - 错误日志
//! - 结构化日志支持

use async_trait::async_trait;
use std::fmt::Debug;
use std::time::Instant;
use tracing::{debug, error, info, warn};

/// 中间件上下文
pub struct MiddlewareContext {
    pub request_id: String,
    pub timestamp: std::time::SystemTime,
    pub metadata: std::collections::HashMap<String, String>,
}

/// 节点中间件 trait
#[async_trait]
pub trait NodeMiddleware<I, O, E, Ev>: Send + Sync + 'static {
    async fn before_run(
        &self,
        input: &I,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<(), E>;
    
    async fn after_run(
        &self,
        input: &I,
        output: &O,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<(), E>;
    
    async fn on_error(
        &self,
        input: &I,
        error: &E,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<(), E>;
    
    async fn on_stream_event(
        &self,
        event: &Ev,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<Option<Ev>, E>;
}

/// 日志记录中间件
pub struct LoggingMiddleware {
    /// 日志级别
    level: tracing::Level,
    /// 是否包含输入
    include_input: bool,
    /// 是否包含输出
    include_output: bool,
    /// 是否记录性能指标
    track_performance: bool,
}

impl LoggingMiddleware {
    pub fn new() -> Self {
        Self {
            level: tracing::Level::INFO,
            include_input: true,
            include_output: true,
            track_performance: true,
        }
    }
    
    pub fn with_level(mut self, level: tracing::Level) -> Self {
        self.level = level;
        self
    }
    
    pub fn with_input(mut self, include: bool) -> Self {
        self.include_input = include;
        self
    }
    
    pub fn with_output(mut self, include: bool) -> Self {
        self.include_output = include;
        self
    }
    
    pub fn with_performance_tracking(mut self, track: bool) -> Self {
        self.track_performance = track;
        self
    }
}

impl Default for LoggingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<I, O, E, Ev> NodeMiddleware<I, O, E, Ev> for LoggingMiddleware
where
    I: Debug + Send + Sync + 'static,
    O: Debug + Send + Sync + 'static,
    E: Debug + Send + Sync + 'static,
    Ev: Debug + Send + Sync + 'static,
{
    async fn before_run(
        &self,
        input: &I,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<(), E> {
        if self.track_performance {
            // 存储开始时间用于性能追踪
            let start = Instant::now();
            context.metadata.insert(
                "start_time_nanos".to_string(),
                start.elapsed().as_nanos().to_string(),
            );
        }
        
        if self.include_input {
            info!(
                request_id = %context.request_id,
                node = node_label,
                input = ?input,
                "Node execution started"
            );
        } else {
            info!(
                request_id = %context.request_id,
                node = node_label,
                "Node execution started"
            );
        }
        
        Ok(())
    }
    
    async fn after_run(
        &self,
        _input: &I,
        output: &O,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<(), E> {
        let duration = if self.track_performance {
            if let Some(start_nanos) = context.metadata.get("start_time_nanos") {
                if let Ok(start) = start_nanos.parse::<u128>() {
                    let current = Instant::now().elapsed().as_nanos();
                    Some((current - start) as f64 / 1_000_000.0) // 转换为毫秒
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        
        if let Some(duration_ms) = duration {
            if self.include_output {
                info!(
                    request_id = %context.request_id,
                    node = node_label,
                    duration_ms = duration_ms,
                    output = ?output,
                    "Node execution completed"
                );
            } else {
                info!(
                    request_id = %context.request_id,
                    node = node_label,
                    duration_ms = duration_ms,
                    "Node execution completed"
                );
            }
        } else {
            if self.include_output {
                info!(
                    request_id = %context.request_id,
                    node = node_label,
                    output = ?output,
                    "Node execution completed"
                );
            } else {
                info!(
                    request_id = %context.request_id,
                    node = node_label,
                    "Node execution completed"
                );
            }
        }
        
        Ok(())
    }
    
    async fn on_error(
        &self,
        _input: &I,
        error: &E,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<(), E> {
        error!(
            request_id = %context.request_id,
            node = node_label,
            error = ?error,
            "Node execution failed"
        );
        
        Ok(())
    }
    
    async fn on_stream_event(
        &self,
        event: &Ev,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<Option<Ev>, E> {
        debug!(
            request_id = %context.request_id,
            node = node_label,
            event = ?event,
            "Stream event emitted"
        );
        
        // 不修改事件，只记录
        Ok(None)
    }
}

/// 使用示例
#[cfg(test)]
mod tests {
    use super::*;
    
    #[derive(Debug, Clone)]
    struct TestInput {
        value: String,
    }
    
    #[derive(Debug, Clone)]
    struct TestOutput {
        result: String,
    }
    
    #[derive(Debug)]
    struct TestError {
        message: String,
    }
    
    #[derive(Debug)]
    enum TestEvent {
        Progress(u32),
        Message(String),
    }
    
    #[tokio::test]
    async fn test_logging_middleware() {
        // 初始化日志
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init()
            .ok();
        
        let middleware = LoggingMiddleware::new()
            .with_level(tracing::Level::INFO)
            .with_performance_tracking(true);
        
        let context = MiddlewareContext {
            request_id: "test-123".to_string(),
            timestamp: std::time::SystemTime::now(),
            metadata: std::collections::HashMap::new(),
        };
        
        let input = TestInput {
            value: "test input".to_string(),
        };
        
        // 测试 before_run
        let result = middleware
            .before_run(&input, "test_node", &context)
            .await;
        assert!(result.is_ok());
        
        // 测试 after_run
        let output = TestOutput {
            result: "test output".to_string(),
        };
        let result = middleware
            .after_run(&input, &output, "test_node", &context)
            .await;
        assert!(result.is_ok());
        
        // 测试 on_error
        let error = TestError {
            message: "test error".to_string(),
        };
        let result = middleware
            .on_error(&input, &error, "test_node", &context)
            .await;
        assert!(result.is_ok());
        
        // 测试 on_stream_event
        let event = TestEvent::Message("streaming...".to_string());
        let result = middleware
            .on_stream_event(&event, "test_node", &context)
            .await;
        assert!(result.is_ok());
    }
}
