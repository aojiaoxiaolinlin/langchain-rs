//! 中间件集成示例
//! 
//! 演示如何在实际的 langchain-rs Agent 中集成和使用多个中间件：
//! - 日志记录中间件
//! - 性能监控中间件
//! - 重试中间件
//! - 内容过滤中间件

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use async_trait::async_trait;
use tracing::{info, warn};

// 注意：这是一个设计示例，展示如何集成中间件
// 实际使用时需要根据真实的 langchain-rs API 调整

/// 中间件上下文
#[derive(Clone)]
pub struct MiddlewareContext {
    pub request_id: String,
    pub timestamp: SystemTime,
    pub metadata: std::collections::HashMap<String, String>,
}

/// 示例：完整的监控 Agent 构建
/// 
/// 这个示例展示如何构建一个具有完整可观测性的 Agent，包括：
/// 1. 日志记录
/// 2. 性能监控
/// 3. 错误重试
/// 4. 内容过滤
pub async fn build_production_agent() -> Result<(), Box<dyn std::error::Error>> {
    // 1. 初始化日志系统
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .init();
    
    info!("Initializing production agent with middleware stack");
    
    // 2. 创建基础 ChatModel
    // 在实际使用中，这里会是：
    // let base_model = ChatOpenAI::builder()
    //     .model("gpt-4")
    //     .api_key(std::env::var("OPENAI_API_KEY")?)
    //     .build();
    
    // 3. 包装模型，添加中间件层
    // let model_with_middleware = MiddlewareChatModel::new(base_model)
    //     // 日志记录：记录所有 LLM 调用
    //     .with_middleware(LoggingMiddleware::new()
    //         .with_level(tracing::Level::INFO)
    //         .with_performance_tracking(true)
    //     )
    //     // 内容过滤：过滤敏感内容
    //     .with_middleware(ContentFilterMiddleware::new()
    //         .with_input_rule(FilterRule::Blacklist(vec![
    //             "敏感词1".to_string(),
    //             "敏感词2".to_string(),
    //         ]))
    //         .with_output_rule(FilterRule::Regex {
    //             pattern: regex::Regex::new(r"电话号码：\d+")?,
    //             replacement: "电话号码：***".to_string(),
    //         })
    //     )
    //     // 重试逻辑：自动重试失败的请求
    //     .with_middleware(RetryMiddleware::new(3)
    //         .with_strategy(RetryStrategy::Exponential {
    //             base: Duration::from_millis(500),
    //             max: Duration::from_secs(30),
    //         })
    //         .with_retryable_check(|e| {
    //             // 只重试速率限制和网络错误
    //             matches!(e, 
    //                 ModelError::RateLimited | 
    //                 ModelError::Network(_)
    //             )
    //         })
    //     )
    //     // 成本追踪：记录 API 调用成本
    //     .with_middleware(CostTrackingMiddleware::new(cost_tracker))
    //     // 速率限制：防止超出 API 限制
    //     .with_middleware(RateLimitMiddleware::new(rate_limiter));
    
    // 4. 定义工具
    // let tools = vec![
    //     search_tool(),
    //     calculator_tool(),
    //     weather_tool(),
    // ];
    
    // 5. 创建 Agent
    // let agent = ReactAgent::builder(model_with_middleware)
    //     .with_tools(tools)
    //     .with_system_prompt("你是一个有用的助手，可以搜索信息和进行计算")
    //     .build();
    
    // 6. 为图添加全局中间件
    // let metrics_collector = Arc::new(PrometheusCollector::new());
    // let agent_with_monitoring = agent
    //     .graph
    //     // 性能监控：收集每个节点的执行时间
    //     .with_global_middleware(
    //         PerformanceMiddleware::new(metrics_collector.clone())
    //     )
    //     // 全局日志：记录所有节点执行
    //     .with_global_middleware(
    //         LoggingMiddleware::new()
    //             .with_level(tracing::Level::DEBUG)
    //     )
    //     // 缓存：缓存 LLM 响应
    //     .with_node_middleware(
    //         ReactAgentLabel::Llm,
    //         CacheMiddleware::new(cache_store, |state| {
    //             // 基于最后一条消息生成缓存键
    //             state.messages
    //                 .last()
    //                 .map(|m| m.content().to_string())
    //                 .unwrap_or_default()
    //         })
    //     );
    
    info!("Agent initialized successfully with full middleware stack");
    
    Ok(())
}

/// 示例：性能监控中间件
pub struct PerformanceMiddleware {
    collector: Arc<dyn MetricsCollector>,
}

pub trait MetricsCollector: Send + Sync {
    fn record_duration(&self, node_label: &str, duration: Duration);
    fn record_error(&self, node_label: &str, error: &str);
}

/// 示例：Prometheus 指标收集器
pub struct PrometheusCollector {
    // 在实际实现中，这里会包含 Prometheus 的 Registry 和 Histogram
}

impl PrometheusCollector {
    pub fn new() -> Self {
        Self {}
    }
}

impl MetricsCollector for PrometheusCollector {
    fn record_duration(&self, node_label: &str, duration: Duration) {
        info!(
            node = node_label,
            duration_ms = duration.as_millis(),
            "Recording performance metric"
        );
        // 实际实现会推送到 Prometheus
    }
    
    fn record_error(&self, node_label: &str, error: &str) {
        warn!(
            node = node_label,
            error = error,
            "Recording error metric"
        );
        // 实际实现会推送到 Prometheus
    }
}

/// 示例：成本追踪中间件
pub struct CostTrackingMiddleware {
    tracker: Arc<CostTracker>,
}

pub struct CostTracker {
    total_cost: std::sync::Mutex<f64>,
}

impl CostTracker {
    pub fn new() -> Self {
        Self {
            total_cost: std::sync::Mutex::new(0.0),
        }
    }
    
    pub fn add_cost(&self, cost: f64) {
        if let Ok(mut total) = self.total_cost.lock() {
            *total += cost;
            info!(cost = cost, total = *total, "Cost tracked");
        }
    }
    
    pub fn get_total_cost(&self) -> f64 {
        self.total_cost.lock().map(|c| *c).unwrap_or(0.0)
    }
}

impl CostTrackingMiddleware {
    pub fn new(tracker: Arc<CostTracker>) -> Self {
        Self { tracker }
    }
    
    fn calculate_cost(&self, usage: &Usage) -> f64 {
        // 示例价格（实际应该基于模型和用途）
        const INPUT_PRICE_PER_1K: f64 = 0.03;  // $0.03 per 1K input tokens
        const OUTPUT_PRICE_PER_1K: f64 = 0.06; // $0.06 per 1K output tokens
        
        let input_cost = (usage.prompt_tokens as f64 / 1000.0) * INPUT_PRICE_PER_1K;
        let output_cost = (usage.completion_tokens as f64 / 1000.0) * OUTPUT_PRICE_PER_1K;
        
        input_cost + output_cost
    }
}

pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// 示例：配置中间件栈的辅助函数
pub fn create_middleware_stack() -> MiddlewareStack {
    MiddlewareStack::new()
        // 最外层：日志记录（记录整个请求）
        .with_middleware(LoggingMiddleware::new())
        // 第二层：重试（处理临时失败）
        .with_middleware(RetryMiddleware::new(3))
        // 第三层：性能监控（测量实际执行时间）
        .with_middleware(PerformanceMiddleware::new(
            Arc::new(PrometheusCollector::new())
        ))
        // 最内层：内容过滤（最接近业务逻辑）
        .with_middleware(ContentFilterMiddleware::new())
}

pub struct MiddlewareStack {
    // 中间件栈的实际实现
}

impl MiddlewareStack {
    pub fn new() -> Self {
        Self {}
    }
    
    pub fn with_middleware<M>(self, _middleware: M) -> Self {
        // 实际实现会添加中间件到栈
        self
    }
}

/// 日志记录中间件（简化版）
pub struct LoggingMiddleware;

impl LoggingMiddleware {
    pub fn new() -> Self {
        Self
    }
}

/// 重试中间件（简化版）
pub struct RetryMiddleware {
    max_retries: u32,
}

impl RetryMiddleware {
    pub fn new(max_retries: u32) -> Self {
        Self { max_retries }
    }
}

/// 内容过滤中间件（简化版）
pub struct ContentFilterMiddleware;

impl ContentFilterMiddleware {
    pub fn new() -> Self {
        Self
    }
}

/// 使用示例
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_middleware_integration() {
        // 初始化日志
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .try_init()
            .ok();
        
        // 测试成本追踪
        let cost_tracker = Arc::new(CostTracker::new());
        let middleware = CostTrackingMiddleware::new(cost_tracker.clone());
        
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };
        
        let cost = middleware.calculate_cost(&usage);
        middleware.tracker.add_cost(cost);
        
        assert!(cost > 0.0);
        assert_eq!(cost_tracker.get_total_cost(), cost);
    }
    
    #[tokio::test]
    async fn test_metrics_collector() {
        let collector = PrometheusCollector::new();
        
        collector.record_duration("test_node", Duration::from_millis(100));
        collector.record_error("test_node", "test error");
    }
}

/// 主函数示例
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    build_production_agent().await
}
