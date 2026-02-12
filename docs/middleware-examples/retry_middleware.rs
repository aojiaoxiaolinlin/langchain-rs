//! 重试中间件示例
//! 
//! 演示如何实现一个智能重试中间件，包括：
//! - 可配置的重试策略（固定延迟、指数退避）
//! - 可重试错误判断
//! - 最大重试次数限制
//! - 重试间隔控制

use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

/// 中间件上下文
pub struct MiddlewareContext {
    pub request_id: String,
    pub timestamp: std::time::SystemTime,
    pub metadata: std::collections::HashMap<String, String>,
}

/// 下一个处理器
pub struct Next<I, O, E> {
    inner: Box<
        dyn Fn(I, MiddlewareContext) -> Pin<Box<dyn Future<Output = Result<O, E>> + Send>>
            + Send
            + Sync,
    >,
}

impl<I, O, E> Next<I, O, E> {
    pub fn new<F, Fut>(f: F) -> Self
    where
        F: Fn(I, MiddlewareContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<O, E>> + Send + 'static,
    {
        Self {
            inner: Box::new(move |input, ctx| Box::pin(f(input, ctx))),
        }
    }
    
    pub async fn call(&self, input: I, context: MiddlewareContext) -> Result<O, E> {
        (self.inner)(input, context).await
    }
}

/// 中间件 trait
#[async_trait]
pub trait Middleware<I, O, E>: Send + Sync + 'static {
    async fn call(
        &self,
        input: I,
        context: MiddlewareContext,
        next: Next<I, O, E>,
    ) -> Result<O, E>;
}

/// 重试策略
#[derive(Debug, Clone)]
pub enum RetryStrategy {
    /// 固定延迟
    Fixed(Duration),
    /// 指数退避
    Exponential {
        /// 基础延迟
        base: Duration,
        /// 最大延迟
        max: Duration,
    },
    /// 自定义策略
    Custom(fn(u32) -> Duration),
}

impl RetryStrategy {
    /// 计算第 N 次重试的延迟时间
    pub fn get_delay(&self, attempt: u32) -> Duration {
        match self {
            RetryStrategy::Fixed(duration) => *duration,
            RetryStrategy::Exponential { base, max } => {
                let delay_ms = base.as_millis() as u64 * 2u64.pow(attempt);
                let max_ms = max.as_millis() as u64;
                Duration::from_millis(delay_ms.min(max_ms))
            }
            RetryStrategy::Custom(f) => f(attempt),
        }
    }
}

/// 重试中间件
pub struct RetryMiddleware<E> {
    /// 最大重试次数
    max_retries: u32,
    /// 重试策略
    strategy: RetryStrategy,
    /// 判断错误是否可重试的函数
    is_retryable: Box<dyn Fn(&E) -> bool + Send + Sync>,
}

impl<E> RetryMiddleware<E> {
    /// 创建新的重试中间件
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            strategy: RetryStrategy::Exponential {
                base: Duration::from_millis(100),
                max: Duration::from_secs(10),
            },
            is_retryable: Box::new(|_| true), // 默认所有错误都重试
        }
    }
    
    /// 设置重试策略
    pub fn with_strategy(mut self, strategy: RetryStrategy) -> Self {
        self.strategy = strategy;
        self
    }
    
    /// 设置可重试错误判断函数
    pub fn with_retryable_check<F>(mut self, check: F) -> Self
    where
        F: Fn(&E) -> bool + Send + Sync + 'static,
    {
        self.is_retryable = Box::new(check);
        self
    }
    
    /// 固定延迟重试
    pub fn with_fixed_delay(max_retries: u32, delay: Duration) -> Self {
        Self {
            max_retries,
            strategy: RetryStrategy::Fixed(delay),
            is_retryable: Box::new(|_| true),
        }
    }
    
    /// 指数退避重试
    pub fn with_exponential_backoff(
        max_retries: u32,
        base: Duration,
        max: Duration,
    ) -> Self {
        Self {
            max_retries,
            strategy: RetryStrategy::Exponential { base, max },
            is_retryable: Box::new(|_| true),
        }
    }
}

#[async_trait]
impl<I, O, E> Middleware<I, O, E> for RetryMiddleware<E>
where
    I: Clone + Send + 'static,
    O: Send + 'static,
    E: std::error::Error + Send + 'static,
{
    async fn call(
        &self,
        input: I,
        context: MiddlewareContext,
        next: Next<I, O, E>,
    ) -> Result<O, E> {
        let mut last_error: Option<E> = None;
        
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay = self.strategy.get_delay(attempt - 1);
                info!(
                    request_id = %context.request_id,
                    attempt = attempt,
                    delay_ms = delay.as_millis(),
                    "Retrying after failure"
                );
                sleep(delay).await;
            }
            
            match next.call(input.clone(), context.clone()).await {
                Ok(output) => {
                    if attempt > 0 {
                        info!(
                            request_id = %context.request_id,
                            attempt = attempt,
                            "Retry succeeded"
                        );
                    }
                    return Ok(output);
                }
                Err(error) => {
                    if !(self.is_retryable)(&error) {
                        warn!(
                            request_id = %context.request_id,
                            error = %error,
                            "Error is not retryable, giving up"
                        );
                        return Err(error);
                    }
                    
                    if attempt < self.max_retries {
                        warn!(
                            request_id = %context.request_id,
                            attempt = attempt,
                            error = %error,
                            "Attempt failed, will retry"
                        );
                    }
                    
                    last_error = Some(error);
                }
            }
        }
        
        warn!(
            request_id = %context.request_id,
            max_retries = self.max_retries,
            "All retry attempts exhausted"
        );
        
        Err(last_error.unwrap())
    }
}

/// 使用示例
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use thiserror::Error;
    
    #[derive(Debug, Error)]
    enum TestError {
        #[error("temporary error")]
        Temporary,
        #[error("permanent error")]
        Permanent,
    }
    
    #[tokio::test]
    async fn test_retry_succeeds_on_second_attempt() {
        let attempt_counter = Arc::new(AtomicU32::new(0));
        let counter_clone = attempt_counter.clone();
        
        let middleware = RetryMiddleware::new(3)
            .with_strategy(RetryStrategy::Fixed(Duration::from_millis(10)));
        
        let context = MiddlewareContext {
            request_id: "test-retry-1".to_string(),
            timestamp: std::time::SystemTime::now(),
            metadata: std::collections::HashMap::new(),
        };
        
        let next = Next::new(move |_input: (), _ctx| {
            let counter = counter_clone.clone();
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt < 1 {
                    Err(TestError::Temporary)
                } else {
                    Ok("success".to_string())
                }
            }
        });
        
        let result = middleware.call((), context, next).await;
        
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "success");
        assert_eq!(attempt_counter.load(Ordering::SeqCst), 2);
    }
    
    #[tokio::test]
    async fn test_retry_exhausted() {
        let middleware = RetryMiddleware::new(2)
            .with_strategy(RetryStrategy::Fixed(Duration::from_millis(10)));
        
        let context = MiddlewareContext {
            request_id: "test-retry-2".to_string(),
            timestamp: std::time::SystemTime::now(),
            metadata: std::collections::HashMap::new(),
        };
        
        let next = Next::new(|_input: (), _ctx| async {
            Err::<String, _>(TestError::Temporary)
        });
        
        let result = middleware.call((), context, next).await;
        
        assert!(result.is_err());
    }
    
    #[tokio::test]
    async fn test_non_retryable_error() {
        let middleware = RetryMiddleware::new(3)
            .with_retryable_check(|e| matches!(e, TestError::Temporary));
        
        let context = MiddlewareContext {
            request_id: "test-retry-3".to_string(),
            timestamp: std::time::SystemTime::now(),
            metadata: std::collections::HashMap::new(),
        };
        
        let next = Next::new(|_input: (), _ctx| async {
            Err::<String, _>(TestError::Permanent)
        });
        
        let result = middleware.call((), context, next).await;
        
        // 应该立即失败，不重试
        assert!(result.is_err());
    }
    
    #[tokio::test]
    async fn test_exponential_backoff() {
        let middleware = RetryMiddleware::with_exponential_backoff(
            3,
            Duration::from_millis(10),
            Duration::from_millis(100),
        );
        
        // 验证延迟时间计算
        assert_eq!(
            middleware.strategy.get_delay(0),
            Duration::from_millis(10)
        );
        assert_eq!(
            middleware.strategy.get_delay(1),
            Duration::from_millis(20)
        );
        assert_eq!(
            middleware.strategy.get_delay(2),
            Duration::from_millis(40)
        );
        assert_eq!(
            middleware.strategy.get_delay(3),
            Duration::from_millis(80)
        );
        // 达到最大值
        assert_eq!(
            middleware.strategy.get_delay(4),
            Duration::from_millis(100)
        );
    }
}
