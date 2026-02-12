# Middleware 示例代码

这个目录包含了 langchain-rs 中间件系统的完整示例代码。每个文件都是一个独立的、可编译的示例，展示了如何实现和使用不同类型的中间件。

## 示例列表

### 1. [logging_middleware.rs](./logging_middleware.rs)

**日志记录中间件** - 展示如何实现一个完整的日志记录中间件。

**特性：**
- 节点执行前后的日志
- 流式事件日志
- 错误日志
- 结构化日志支持（使用 `tracing`）
- 性能指标追踪

**使用场景：**
- 调试和开发
- 生产环境监控
- 审计追踪

**代码亮点：**
```rust
let middleware = LoggingMiddleware::new()
    .with_level(tracing::Level::INFO)
    .with_performance_tracking(true)
    .with_input(true)
    .with_output(true);
```

---

### 2. [retry_middleware.rs](./retry_middleware.rs)

**重试中间件** - 演示如何实现智能重试逻辑。

**特性：**
- 可配置的重试策略（固定延迟、指数退避）
- 可重试错误判断
- 最大重试次数限制
- 重试间隔控制
- 详细的重试日志

**使用场景：**
- 处理临时网络故障
- LLM API 速率限制
- 提高系统可靠性

**代码亮点：**
```rust
let middleware = RetryMiddleware::with_exponential_backoff(
    3,  // 最大重试次数
    Duration::from_millis(100),  // 基础延迟
    Duration::from_secs(10),     // 最大延迟
).with_retryable_check(|e| {
    // 只重试特定类型的错误
    matches!(e, ModelError::RateLimited | ModelError::Network(_))
});
```

---

### 3. [integration_example.rs](./integration_example.rs)

**集成示例** - 展示如何在实际 Agent 中组合使用多个中间件。

**特性：**
- 完整的生产级 Agent 配置
- 多层中间件栈
- 性能监控集成
- 成本追踪
- Prometheus 指标导出示例

**使用场景：**
- 生产环境部署
- 多中间件协同工作
- 端到端的可观测性

**代码亮点：**
```rust
let model_with_middleware = MiddlewareChatModel::new(base_model)
    .with_middleware(LoggingMiddleware::new())
    .with_middleware(ContentFilterMiddleware::new())
    .with_middleware(RetryMiddleware::new(3))
    .with_middleware(CostTrackingMiddleware::new(tracker))
    .with_middleware(RateLimitMiddleware::new(limiter));
```

---

## 如何使用这些示例

### 1. 作为学习资源

每个示例都是独立的，包含完整的注释和测试代码。你可以：

1. **阅读代码**：理解中间件的实现原理
2. **运行测试**：验证功能正确性
3. **修改实验**：根据需求调整参数

### 2. 作为模板

这些示例可以作为实现自定义中间件的模板：

1. **复制示例**：选择最接近需求的示例
2. **修改逻辑**：根据具体需求调整实现
3. **添加测试**：确保新中间件正确工作

### 3. 直接集成

某些示例（如日志和重试）可以直接用于生产环境：

1. **复制代码**：将示例代码复制到项目中
2. **调整配置**：根据环境调整参数
3. **集成使用**：添加到 Agent 或 Graph 中

---

## 运行示例

### 前置条件

```bash
# 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 克隆仓库
git clone https://github.com/aojiaoxiaolinlin/langchain-rs.git
cd langchain-rs
```

### 运行单个示例的测试

```bash
# 运行日志中间件测试
cargo test --package langchain-rs-examples logging_middleware

# 运行重试中间件测试
cargo test --package langchain-rs-examples retry_middleware

# 运行集成示例测试
cargo test --package langchain-rs-examples integration_example
```

### 查看详细日志

```bash
# 启用详细日志输出
RUST_LOG=debug cargo test -- --nocapture
```

---

## 中间件实现指南

### 基本结构

所有中间件都遵循相似的模式：

```rust
use async_trait::async_trait;

// 1. 定义中间件结构
pub struct MyMiddleware {
    // 配置字段
}

// 2. 实现构造函数和配置方法
impl MyMiddleware {
    pub fn new() -> Self { /* ... */ }
    pub fn with_config(mut self, config: Config) -> Self { /* ... */ }
}

// 3. 实现中间件 trait
#[async_trait]
impl<I, O, E> Middleware<I, O, E> for MyMiddleware {
    async fn call(&self, input: I, ctx: Context, next: Next<I, O, E>) -> Result<O, E> {
        // 前置处理
        // ...
        
        // 调用下一个中间件
        let result = next.call(input, ctx).await;
        
        // 后置处理
        // ...
        
        result
    }
}
```

### 设计原则

1. **单一职责**：每个中间件只做一件事
2. **可组合**：中间件应该可以自由组合
3. **非侵入**：不修改业务逻辑
4. **高性能**：避免阻塞操作
5. **容错性**：优雅处理错误

### 常见模式

#### 1. 观察者模式（Logging）

```rust
async fn call(&self, input: I, ctx: Context, next: Next) -> Result<O, E> {
    self.log_before(&input);
    let result = next.call(input, ctx).await;
    self.log_after(&result);
    result
}
```

#### 2. 装饰器模式（Retry）

```rust
async fn call(&self, input: I, ctx: Context, next: Next) -> Result<O, E> {
    for attempt in 0..self.max_retries {
        match next.call(input.clone(), ctx.clone()).await {
            Ok(output) => return Ok(output),
            Err(e) if self.should_retry(&e) => continue,
            Err(e) => return Err(e),
        }
    }
}
```

#### 3. 拦截器模式（Filter）

```rust
async fn call(&self, mut input: I, ctx: Context, next: Next) -> Result<O, E> {
    // 修改输入
    self.filter_input(&mut input);
    
    // 执行
    let mut output = next.call(input, ctx).await?;
    
    // 修改输出
    self.filter_output(&mut output);
    
    Ok(output)
}
```

---

## 扩展阅读

- [主设计文档](../../MIDDLEWARE_DESIGN.md) - 完整的中间件设计指南
- [架构文档](../../ARCHITECTURE.md) - langchain-rs 整体架构
- [Tower Middleware](https://docs.rs/tower/latest/tower/trait.Service.html) - Rust 生态系统中的中间件参考

---

## 贡献

欢迎贡献新的中间件示例！如果你有有用的中间件实现，请：

1. Fork 仓库
2. 添加示例代码和测试
3. 更新此 README
4. 提交 Pull Request

---

## 许可证

这些示例代码与 langchain-rs 主项目使用相同的许可证。

---

*最后更新: 2026-02-12*
