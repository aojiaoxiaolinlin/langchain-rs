# langchain-rs 中间件设计指南

## 目录

1. [概述](#概述)
2. [LangChain v1 中间件功能分析](#langchain-v1-中间件功能分析)
3. [Rust 特性与中间件设计](#rust-特性与中间件设计)
4. [架构设计](#架构设计)
5. [实现模式](#实现模式)
6. [集成点](#集成点)
7. [最佳实践](#最佳实践)
8. [示例代码](#示例代码)

---

## 概述

### 什么是中间件？

中间件（Middleware）是一种软件设计模式，用于在请求/响应流程中插入自定义逻辑。在 LangChain 上下文中，中间件允许开发者在 LLM 调用、工具执行、图节点运行等关键点拦截和处理数据流。

### 中间件的核心价值

1. **可观测性**：日志记录、性能监控、调试追踪
2. **安全性**：输入验证、输出过滤、权限控制
3. **可靠性**：错误处理、重试逻辑、降级策略
4. **扩展性**：功能增强、行为修改，无需修改核心代码
5. **合规性**：审计日志、数据脱敏、策略执行

---

## LangChain v1 中间件功能分析

### 1. LangChain Python 中间件架构

LangChain v1（Python）提供了多层次的中间件支持：

#### 1.1 Runnable 中间件

```python
# Python LangChain v1
from langchain_core.runnables import RunnableMiddleware

class LoggingMiddleware(RunnableMiddleware):
    def invoke(self, input, config):
        print(f"Input: {input}")
        result = super().invoke(input, config)
        print(f"Output: {result}")
        return result
    
    async def ainvoke(self, input, config):
        print(f"Async Input: {input}")
        result = await super().ainvoke(input, config)
        print(f"Async Output: {result}")
        return result

# 使用
chain = prompt | LoggingMiddleware() | llm
```

#### 1.2 回调系统（Callbacks）

```python
from langchain_core.callbacks import BaseCallbackHandler

class MetricsCallback(BaseCallbackHandler):
    def on_llm_start(self, serialized, prompts, **kwargs):
        """LLM 开始调用时触发"""
        print(f"LLM starting with prompts: {prompts}")
    
    def on_llm_end(self, response, **kwargs):
        """LLM 调用结束时触发"""
        print(f"LLM response: {response}")
    
    def on_tool_start(self, serialized, input_str, **kwargs):
        """工具开始执行时触发"""
        pass
    
    def on_tool_end(self, output, **kwargs):
        """工具执行结束时触发"""
        pass

# 使用
chain.invoke(input, config={"callbacks": [MetricsCallback()]})
```

#### 1.3 拦截器（Interceptors）

```python
# 请求/响应拦截
class RetryInterceptor:
    def __call__(self, func):
        @wraps(func)
        async def wrapper(*args, **kwargs):
            for attempt in range(3):
                try:
                    return await func(*args, **kwargs)
                except Exception as e:
                    if attempt == 2:
                        raise
                    await asyncio.sleep(2 ** attempt)
        return wrapper
```

### 2. LangChain v1 中间件关键特性

| 特性 | 描述 | 用途 |
|------|------|------|
| **生命周期钩子** | on_start, on_end, on_error | 监控和日志 |
| **请求/响应修改** | 拦截并修改输入/输出 | 数据转换、验证 |
| **链式组合** | 中间件可以层叠组合 | 模块化功能 |
| **配置传播** | 配置在调用链中传递 | 统一设置 |
| **异步支持** | 同时支持同步和异步 | 性能优化 |
| **流式支持** | 中间件可以拦截流式输出 | 实时处理 |

---

## Rust 特性与中间件设计

### 1. Rust 语言特性优势

#### 1.1 类型安全的中间件

Rust 的类型系统可以在编译时确保中间件的正确性：

```rust
// 类型安全的中间件 trait
pub trait Middleware<I, O, E>: Send + Sync {
    type Error: From<E>;
    
    async fn process(
        &self,
        input: I,
        next: Next<I, O, E>
    ) -> Result<O, Self::Error>;
}
```

#### 1.2 零成本抽象

Rust 的 trait 和泛型允许在运行时无额外开销地使用中间件：

```rust
// 编译时单态化，运行时零成本
impl<M, I, O, E> Node<I, O, E> for MiddlewareNode<M, I, O, E>
where
    M: Middleware<I, O, E>,
{
    // 实现...
}
```

#### 1.3 所有权和借用

Rust 的所有权系统避免了数据竞争，适合并发中间件：

```rust
// 不可变借用，多线程安全
async fn run(&self, input: &I) -> Result<O, E> {
    self.middleware.pre_process(input).await?;
    let output = self.inner.run(input).await?;
    self.middleware.post_process(&output).await?;
    Ok(output)
}
```

#### 1.4 trait 组合和扩展

```rust
// Trait 组合提供灵活的中间件能力
pub trait LoggingMiddleware: Middleware {
    fn log_level(&self) -> LogLevel;
}

pub trait RetryMiddleware: Middleware {
    fn max_retries(&self) -> u32;
    fn backoff_strategy(&self) -> BackoffStrategy;
}
```

### 2. Rust 生态系统工具

| 工具/库 | 用途 | 示例 |
|---------|------|------|
| **tower** | 通用中间件框架 | 服务抽象、中间件栈 |
| **tracing** | 结构化日志和追踪 | 性能分析、调试 |
| **async-trait** | 异步 trait 支持 | 异步中间件 |
| **thiserror** | 错误派生 | 统一错误处理 |
| **pin-project** | Pin 投影 | 流式中间件 |

---

## 架构设计

### 1. 分层中间件架构

```
┌─────────────────────────────────────────────────────────┐
│                   应用层 (Application)                   │
│                 用户定义的 Agent/工作流                  │
└────────────────────┬────────────────────────────────────┘
                     │
┌────────────────────▼────────────────────────────────────┐
│              中间件栈 (Middleware Stack)                 │
│  ┌──────────────────────────────────────────────────┐  │
│  │  监控中间件 (Observability Middleware)           │  │
│  │  • 日志记录 • 指标收集 • 分布式追踪              │  │
│  └──────────────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────────────┐  │
│  │  安全中间件 (Security Middleware)                │  │
│  │  • 输入验证 • 输出过滤 • 权限检查                │  │
│  └──────────────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────────────┐  │
│  │  可靠性中间件 (Reliability Middleware)           │  │
│  │  • 重试逻辑 • 超时控制 • 降级策略                │  │
│  └──────────────────────────────────────────────────┘  │
└────────────────────┬────────────────────────────────────┘
                     │
┌────────────────────▼────────────────────────────────────┐
│               执行层 (Execution Layer)                   │
│     • Graph 执行器 • Node 运行时 • 状态管理             │
└────────────────────┬────────────────────────────────────┘
                     │
┌────────────────────▼────────────────────────────────────┐
│                核心层 (Core Layer)                       │
│          • ChatModel • Tool • Message                    │
└─────────────────────────────────────────────────────────┘
```

### 2. 核心中间件 Trait

#### 2.1 基础中间件 Trait

```rust
use async_trait::async_trait;
use std::future::Future;

/// 中间件上下文，包含请求元数据
pub struct MiddlewareContext {
    /// 请求 ID，用于追踪
    pub request_id: String,
    /// 时间戳
    pub timestamp: std::time::SystemTime,
    /// 自定义元数据
    pub metadata: std::collections::HashMap<String, String>,
}

/// 下一个处理器的抽象
pub struct Next<I, O, E> {
    inner: Box<dyn Fn(I, MiddlewareContext) -> 
        Pin<Box<dyn Future<Output = Result<O, E>> + Send>> + Send + Sync>,
}

/// 中间件核心 trait
#[async_trait]
pub trait Middleware<I, O, E>: Send + Sync + 'static {
    /// 处理请求
    /// 
    /// # 参数
    /// 
    /// * `input` - 输入数据
    /// * `context` - 中间件上下文
    /// * `next` - 下一个处理器
    async fn call(
        &self,
        input: I,
        context: MiddlewareContext,
        next: Next<I, O, E>,
    ) -> Result<O, E>;
}
```

#### 2.2 节点中间件 Trait

```rust
/// 节点级别的中间件
#[async_trait]
pub trait NodeMiddleware<I, O, E, Ev>: Send + Sync + 'static {
    /// 节点执行前调用
    async fn before_run(
        &self,
        input: &I,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<(), E> {
        Ok(())
    }
    
    /// 节点执行后调用
    async fn after_run(
        &self,
        input: &I,
        output: &O,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<(), E> {
        Ok(())
    }
    
    /// 节点执行失败时调用
    async fn on_error(
        &self,
        input: &I,
        error: &E,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<(), E> {
        Ok(())
    }
    
    /// 流式事件拦截
    async fn on_stream_event(
        &self,
        event: &Ev,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<Option<Ev>, E> {
        // 默认不修改事件
        Ok(None)
    }
}
```

#### 2.3 模型中间件 Trait

```rust
use langchain_core::message::Message;
use langchain_core::state::{ChatCompletion, InvokeOptions};

/// ChatModel 专用中间件
#[async_trait]
pub trait ChatModelMiddleware: Send + Sync + 'static {
    /// 调用前拦截，可以修改消息
    async fn before_invoke(
        &self,
        messages: &mut Vec<Arc<Message>>,
        options: &mut InvokeOptions<'_>,
        context: &MiddlewareContext,
    ) -> Result<(), ModelError>;
    
    /// 调用后拦截，可以修改响应
    async fn after_invoke(
        &self,
        messages: &[Arc<Message>],
        completion: &mut ChatCompletion,
        context: &MiddlewareContext,
    ) -> Result<(), ModelError>;
    
    /// 流式事件拦截
    async fn on_stream_chunk(
        &self,
        chunk: &ChatStreamEvent,
        context: &MiddlewareContext,
    ) -> Result<Option<ChatStreamEvent>, ModelError> {
        Ok(None)
    }
}
```

### 3. 中间件链（Middleware Chain）

#### 3.1 中间件栈设计

```rust
/// 中间件栈，支持多个中间件组合
pub struct MiddlewareStack<I, O, E> {
    middlewares: Vec<Box<dyn Middleware<I, O, E>>>,
}

impl<I, O, E> MiddlewareStack<I, O, E>
where
    I: Clone + Send + 'static,
    O: Send + 'static,
    E: Send + 'static,
{
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }
    
    /// 添加中间件到栈顶
    pub fn push<M>(mut self, middleware: M) -> Self
    where
        M: Middleware<I, O, E>,
    {
        self.middlewares.push(Box::new(middleware));
        self
    }
    
    /// 执行中间件链
    pub async fn execute<F, Fut>(
        &self,
        input: I,
        context: MiddlewareContext,
        handler: F,
    ) -> Result<O, E>
    where
        F: Fn(I, MiddlewareContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<O, E>> + Send + 'static,
    {
        // 递归构建中间件链
        self.execute_recursive(0, input, context, Arc::new(handler)).await
    }
    
    fn execute_recursive<F, Fut>(
        &self,
        index: usize,
        input: I,
        context: MiddlewareContext,
        handler: Arc<F>,
    ) -> Pin<Box<dyn Future<Output = Result<O, E>> + Send>>
    where
        F: Fn(I, MiddlewareContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<O, E>> + Send + 'static,
        I: 'static,
        O: 'static,
        E: 'static,
    {
        if index >= self.middlewares.len() {
            // 到达链尾，执行实际处理器
            Box::pin(async move { handler(input, context).await })
        } else {
            // 执行当前中间件
            let middleware = &self.middlewares[index];
            let next_index = index + 1;
            
            // 构建 next 闭包
            let next = Next {
                inner: Box::new(move |input, ctx| {
                    self.execute_recursive(next_index, input, ctx, handler.clone())
                }),
            };
            
            Box::pin(middleware.call(input, context, next))
        }
    }
}
```

### 4. 与现有架构集成

#### 4.1 集成到 Node Trait

```rust
/// 支持中间件的节点包装器
pub struct MiddlewareNode<I, O, E, Ev> {
    inner: Box<dyn Node<I, O, E, Ev>>,
    middlewares: Vec<Box<dyn NodeMiddleware<I, O, E, Ev>>>,
}

#[async_trait]
impl<I, O, E, Ev> Node<I, O, E, Ev> for MiddlewareNode<I, O, E, Ev>
where
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
    E: Send + Sync + 'static,
    Ev: Send + Sync + 'static,
{
    async fn run_sync(
        &self,
        input: &I,
        context: NodeContext<'_>,
    ) -> Result<O, E> {
        let mw_context = MiddlewareContext {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: std::time::SystemTime::now(),
            metadata: Default::default(),
        };
        
        // 执行 before_run 钩子
        for middleware in &self.middlewares {
            middleware.before_run(input, "node_label", &mw_context).await?;
        }
        
        // 执行实际节点
        let result = self.inner.run_sync(input, context).await;
        
        match &result {
            Ok(output) => {
                // 执行 after_run 钩子
                for middleware in &self.middlewares {
                    middleware.after_run(input, output, "node_label", &mw_context).await?;
                }
            }
            Err(error) => {
                // 执行 on_error 钩子
                for middleware in &self.middlewares {
                    middleware.on_error(input, error, "node_label", &mw_context).await?;
                }
            }
        }
        
        result
    }
    
    async fn run_stream(
        &self,
        input: &I,
        sink: &dyn EventSink<Ev>,
        context: NodeContext<'_>,
    ) -> Result<O, E> {
        // 创建包装的 EventSink，拦截流式事件
        let wrapped_sink = MiddlewareEventSink {
            inner: sink,
            middlewares: &self.middlewares,
            node_label: "node_label",
            context: MiddlewareContext {
                request_id: uuid::Uuid::new_v4().to_string(),
                timestamp: std::time::SystemTime::now(),
                metadata: Default::default(),
            },
        };
        
        self.inner.run_stream(input, &wrapped_sink, context).await
    }
}

/// 包装的 EventSink，拦截流式事件
struct MiddlewareEventSink<'a, Ev> {
    inner: &'a dyn EventSink<Ev>,
    middlewares: &'a [Box<dyn NodeMiddleware<I, O, E, Ev>>],
    node_label: &'a str,
    context: MiddlewareContext,
}

#[async_trait]
impl<'a, Ev> EventSink<Ev> for MiddlewareEventSink<'a, Ev>
where
    Ev: Send + Sync + 'static,
{
    async fn emit(&self, event: Ev) {
        let mut modified_event = Some(event);
        
        // 让每个中间件处理事件
        for middleware in self.middlewares {
            if let Some(event) = modified_event {
                modified_event = middleware
                    .on_stream_event(&event, self.node_label, &self.context)
                    .await
                    .ok()
                    .flatten()
                    .or(Some(event));
            }
        }
        
        // 发送最终事件
        if let Some(event) = modified_event {
            self.inner.emit(event).await;
        }
    }
}
```

#### 4.2 集成到 ChatModel

```rust
/// 支持中间件的 ChatModel 包装器
pub struct MiddlewareChatModel<M>
where
    M: ChatModel,
{
    inner: M,
    middlewares: Vec<Box<dyn ChatModelMiddleware>>,
}

impl<M> MiddlewareChatModel<M>
where
    M: ChatModel,
{
    pub fn new(model: M) -> Self {
        Self {
            inner: model,
            middlewares: Vec::new(),
        }
    }
    
    pub fn with_middleware<Mw>(mut self, middleware: Mw) -> Self
    where
        Mw: ChatModelMiddleware,
    {
        self.middlewares.push(Box::new(middleware));
        self
    }
}

#[async_trait]
impl<M> ChatModel for MiddlewareChatModel<M>
where
    M: ChatModel + Send + Sync,
{
    async fn invoke(
        &self,
        messages: &[Arc<Message>],
        options: &InvokeOptions<'_>,
    ) -> Result<ChatCompletion, ModelError> {
        let context = MiddlewareContext {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: std::time::SystemTime::now(),
            metadata: Default::default(),
        };
        
        let mut messages = messages.to_vec();
        let mut options = options.clone();
        
        // 执行 before_invoke 钩子
        for middleware in &self.middlewares {
            middleware.before_invoke(&mut messages, &mut options, &context).await?;
        }
        
        // 执行实际调用
        let mut completion = self.inner.invoke(&messages, &options).await?;
        
        // 执行 after_invoke 钩子
        for middleware in &self.middlewares {
            middleware.after_invoke(&messages, &mut completion, &context).await?;
        }
        
        Ok(completion)
    }
    
    async fn stream(
        &self,
        messages: &[Arc<Message>],
        options: &InvokeOptions<'_>,
    ) -> Result<StandardChatStream, ModelError> {
        let stream = self.inner.stream(messages, options).await?;
        
        // 包装流，拦截每个 chunk
        let context = MiddlewareContext {
            request_id: uuid::Uuid::new_v4().to_string(),
            timestamp: std::time::SystemTime::now(),
            metadata: Default::default(),
        };
        
        let middlewares = self.middlewares.clone();
        let wrapped_stream = stream.then(move |chunk| {
            let context = context.clone();
            let middlewares = middlewares.clone();
            async move {
                if let Ok(chunk) = chunk {
                    // 让中间件处理 chunk
                    for middleware in &middlewares {
                        if let Ok(Some(modified)) = middleware.on_stream_chunk(&chunk, &context).await {
                            return Ok(modified);
                        }
                    }
                    Ok(chunk)
                } else {
                    chunk
                }
            }
        });
        
        Ok(Box::pin(wrapped_stream))
    }
}
```

#### 4.3 集成到 StateGraph

```rust
impl<S, U, E, Ev> StateGraph<S, U, E, Ev>
where
    S: Send + Sync + Clone + 'static,
    U: Send + Sync + 'static,
    E: Send + Sync + 'static,
    Ev: Send + Sync + Debug + 'static,
{
    /// 为所有节点添加中间件
    pub fn with_global_middleware<M>(mut self, middleware: M) -> Self
    where
        M: NodeMiddleware<S, U, E, Ev> + Clone,
    {
        // 遍历所有节点，包装为 MiddlewareNode
        for (label, node_state) in &mut self.graph.nodes {
            let middleware_node = MiddlewareNode {
                inner: std::mem::replace(&mut node_state.node, Box::new(DummyNode)),
                middlewares: vec![Box::new(middleware.clone())],
            };
            node_state.node = Box::new(middleware_node);
        }
        self
    }
    
    /// 为特定节点添加中间件
    pub fn with_node_middleware<M>(
        mut self,
        label: impl GraphLabel,
        middleware: M,
    ) -> Self
    where
        M: NodeMiddleware<S, U, E, Ev>,
    {
        let interned = label.intern();
        if let Some(node_state) = self.graph.nodes.get_mut(&interned) {
            let middleware_node = MiddlewareNode {
                inner: std::mem::replace(&mut node_state.node, Box::new(DummyNode)),
                middlewares: vec![Box::new(middleware)],
            };
            node_state.node = Box::new(middleware_node);
        }
        self
    }
}
```

---

## 实现模式

### 1. 日志记录中间件

```rust
use tracing::{info, error, debug};

/// 日志记录中间件
pub struct LoggingMiddleware {
    level: tracing::Level,
    include_input: bool,
    include_output: bool,
}

impl LoggingMiddleware {
    pub fn new() -> Self {
        Self {
            level: tracing::Level::INFO,
            include_input: true,
            include_output: true,
        }
    }
    
    pub fn with_level(mut self, level: tracing::Level) -> Self {
        self.level = level;
        self
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
        Ok(None) // 不修改事件
    }
}
```

### 2. 重试中间件

```rust
use std::time::Duration;
use tokio::time::sleep;

/// 重试策略
pub enum RetryStrategy {
    /// 固定延迟
    Fixed(Duration),
    /// 指数退避
    Exponential { base: Duration, max: Duration },
    /// 自定义策略
    Custom(Box<dyn Fn(u32) -> Duration + Send + Sync>),
}

/// 重试中间件
pub struct RetryMiddleware {
    max_retries: u32,
    strategy: RetryStrategy,
    /// 可重试的错误判断函数
    is_retryable: Box<dyn Fn(&dyn std::error::Error) -> bool + Send + Sync>,
}

impl RetryMiddleware {
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
    
    pub fn with_strategy(mut self, strategy: RetryStrategy) -> Self {
        self.strategy = strategy;
        self
    }
    
    pub fn with_retryable_check<F>(mut self, check: F) -> Self
    where
        F: Fn(&dyn std::error::Error) -> bool + Send + Sync + 'static,
    {
        self.is_retryable = Box::new(check);
        self
    }
    
    fn get_delay(&self, attempt: u32) -> Duration {
        match &self.strategy {
            RetryStrategy::Fixed(duration) => *duration,
            RetryStrategy::Exponential { base, max } => {
                let delay = base.as_millis() as u64 * 2u64.pow(attempt);
                Duration::from_millis(delay.min(max.as_millis() as u64))
            }
            RetryStrategy::Custom(f) => f(attempt),
        }
    }
}

#[async_trait]
impl<I, O, E> Middleware<I, O, E> for RetryMiddleware
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
        let mut last_error = None;
        
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay = self.get_delay(attempt - 1);
                tracing::info!(
                    attempt = attempt,
                    delay_ms = delay.as_millis(),
                    "Retrying after failure"
                );
                sleep(delay).await;
            }
            
            match next.call(input.clone(), context.clone()).await {
                Ok(output) => return Ok(output),
                Err(error) => {
                    if !self.is_retryable(&error) {
                        return Err(error);
                    }
                    last_error = Some(error);
                }
            }
        }
        
        Err(last_error.unwrap())
    }
}
```

### 3. 性能监控中间件

```rust
use std::time::Instant;

/// 性能指标
#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    pub node_label: String,
    pub duration: Duration,
    pub input_size: usize,
    pub output_size: usize,
    pub timestamp: SystemTime,
}

/// 指标收集器 trait
pub trait MetricsCollector: Send + Sync {
    fn record(&self, metrics: PerformanceMetrics);
}

/// 性能监控中间件
pub struct PerformanceMiddleware {
    collector: Arc<dyn MetricsCollector>,
}

impl PerformanceMiddleware {
    pub fn new(collector: Arc<dyn MetricsCollector>) -> Self {
        Self { collector }
    }
}

#[async_trait]
impl<I, O, E, Ev> NodeMiddleware<I, O, E, Ev> for PerformanceMiddleware
where
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
    E: Send + Sync + 'static,
    Ev: Send + Sync + 'static,
{
    async fn before_run(
        &self,
        _input: &I,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<(), E> {
        // 在 context.metadata 中存储开始时间
        context.metadata.insert(
            "start_time".to_string(),
            Instant::now().elapsed().as_nanos().to_string(),
        );
        Ok(())
    }
    
    async fn after_run(
        &self,
        _input: &I,
        _output: &O,
        node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<(), E> {
        if let Some(start) = context.metadata.get("start_time") {
            if let Ok(start_nanos) = start.parse::<u128>() {
                let duration = Duration::from_nanos(
                    (Instant::now().elapsed().as_nanos() - start_nanos) as u64
                );
                
                let metrics = PerformanceMetrics {
                    node_label: node_label.to_string(),
                    duration,
                    input_size: std::mem::size_of::<I>(),
                    output_size: std::mem::size_of::<O>(),
                    timestamp: context.timestamp,
                };
                
                self.collector.record(metrics);
            }
        }
        Ok(())
    }
}
```

### 4. 内容过滤中间件

```rust
use regex::Regex;

/// 内容过滤规则
pub enum FilterRule {
    /// 正则表达式替换
    Regex { pattern: Regex, replacement: String },
    /// 关键词黑名单
    Blacklist(Vec<String>),
    /// 自定义过滤函数
    Custom(Box<dyn Fn(&str) -> String + Send + Sync>),
}

/// 内容过滤中间件
pub struct ContentFilterMiddleware {
    input_rules: Vec<FilterRule>,
    output_rules: Vec<FilterRule>,
}

impl ContentFilterMiddleware {
    pub fn new() -> Self {
        Self {
            input_rules: Vec::new(),
            output_rules: Vec::new(),
        }
    }
    
    pub fn with_input_rule(mut self, rule: FilterRule) -> Self {
        self.input_rules.push(rule);
        self
    }
    
    pub fn with_output_rule(mut self, rule: FilterRule) -> Self {
        self.output_rules.push(rule);
        self
    }
    
    fn apply_rules(&self, text: &str, rules: &[FilterRule]) -> String {
        let mut result = text.to_string();
        for rule in rules {
            result = match rule {
                FilterRule::Regex { pattern, replacement } => {
                    pattern.replace_all(&result, replacement).to_string()
                }
                FilterRule::Blacklist(words) => {
                    let mut temp = result;
                    for word in words {
                        temp = temp.replace(word, "***");
                    }
                    temp
                }
                FilterRule::Custom(f) => f(&result),
            };
        }
        result
    }
}

#[async_trait]
impl ChatModelMiddleware for ContentFilterMiddleware {
    async fn before_invoke(
        &self,
        messages: &mut Vec<Arc<Message>>,
        _options: &mut InvokeOptions<'_>,
        _context: &MiddlewareContext,
    ) -> Result<(), ModelError> {
        // 过滤输入消息
        for message in messages.iter_mut() {
            if let Message::User { content, .. } = Arc::make_mut(message) {
                *content = self.apply_rules(content, &self.input_rules);
            }
        }
        Ok(())
    }
    
    async fn after_invoke(
        &self,
        _messages: &[Arc<Message>],
        completion: &mut ChatCompletion,
        _context: &MiddlewareContext,
    ) -> Result<(), ModelError> {
        // 过滤输出消息
        for message in &mut completion.messages {
            if let Message::Assistant { content, .. } = Arc::make_mut(message) {
                *content = self.apply_rules(content, &self.output_rules);
            }
        }
        Ok(())
    }
}
```

### 5. 缓存中间件

```rust
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::RwLock;

/// 缓存键
pub trait CacheKey: Hash + Eq + Clone + Send + Sync + 'static {}

/// 缓存存储 trait
pub trait CacheStore<K, V>: Send + Sync {
    fn get(&self, key: &K) -> Option<V>;
    fn set(&self, key: K, value: V);
    fn invalidate(&self, key: &K);
    fn clear(&self);
}

/// 内存缓存实现
pub struct InMemoryCache<K, V> {
    store: RwLock<HashMap<K, V>>,
    max_size: usize,
}

impl<K, V> InMemoryCache<K, V>
where
    K: Hash + Eq + Clone,
    V: Clone,
{
    pub fn new(max_size: usize) -> Self {
        Self {
            store: RwLock::new(HashMap::new()),
            max_size,
        }
    }
}

impl<K, V> CacheStore<K, V> for InMemoryCache<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn get(&self, key: &K) -> Option<V> {
        self.store.read().ok()?.get(key).cloned()
    }
    
    fn set(&self, key: K, value: V) {
        if let Ok(mut store) = self.store.write() {
            if store.len() >= self.max_size {
                // 简单的 LRU：移除第一个元素
                if let Some(first_key) = store.keys().next().cloned() {
                    store.remove(&first_key);
                }
            }
            store.insert(key, value);
        }
    }
    
    fn invalidate(&self, key: &K) {
        if let Ok(mut store) = self.store.write() {
            store.remove(key);
        }
    }
    
    fn clear(&self) {
        if let Ok(mut store) = self.store.write() {
            store.clear();
        }
    }
}

/// 缓存中间件
pub struct CacheMiddleware<K, V> {
    cache: Arc<dyn CacheStore<K, V>>,
    key_extractor: Box<dyn Fn(&MessagesState) -> K + Send + Sync>,
}

impl<K, V> CacheMiddleware<K, V>
where
    K: CacheKey,
    V: Clone + Send + Sync + 'static,
{
    pub fn new<F>(cache: Arc<dyn CacheStore<K, V>>, key_extractor: F) -> Self
    where
        F: Fn(&MessagesState) -> K + Send + Sync + 'static,
    {
        Self {
            cache,
            key_extractor: Box::new(key_extractor),
        }
    }
}

#[async_trait]
impl<K> NodeMiddleware<MessagesState, MessagesState, AgentError, ChatStreamEvent>
    for CacheMiddleware<K, ChatCompletion>
where
    K: CacheKey,
{
    async fn before_run(
        &self,
        input: &MessagesState,
        _node_label: &str,
        context: &MiddlewareContext,
    ) -> Result<(), AgentError> {
        let key = (self.key_extractor)(input);
        if let Some(cached) = self.cache.get(&key) {
            // 将缓存命中标记存入 context
            context.metadata.insert("cache_hit".to_string(), "true".to_string());
            tracing::info!(request_id = %context.request_id, "Cache hit");
        }
        Ok(())
    }
}
```

---

## 集成点

### 1. StateGraph 集成示例

```rust
use langchain::ReactAgent;
use langchain_openai::ChatOpenAI;

// 创建 Agent，添加中间件
let model = ChatOpenAI::builder()
    .model("gpt-4")
    .api_key("sk-...")
    .build();

// 包装模型，添加模型级中间件
let model_with_middleware = MiddlewareChatModel::new(model)
    .with_middleware(LoggingMiddleware::new())
    .with_middleware(ContentFilterMiddleware::new()
        .with_input_rule(FilterRule::Blacklist(vec!["敏感词".to_string()]))
    )
    .with_middleware(RetryMiddleware::new(3));

// 创建 Agent
let agent = ReactAgent::builder(model_with_middleware)
    .with_tools(vec![search_tool(), calculate_tool()])
    .build();

// 为整个图添加全局中间件
let agent_with_middleware = agent
    .graph
    .with_global_middleware(PerformanceMiddleware::new(metrics_collector))
    .with_global_middleware(LoggingMiddleware::new().with_level(tracing::Level::DEBUG));

// 为特定节点添加中间件
let agent_final = agent_with_middleware
    .with_node_middleware(
        ReactAgentLabel::Llm,
        CacheMiddleware::new(cache_store, |state| {
            // 提取缓存键
            state.messages.last().map(|m| m.content()).unwrap_or("")
        })
    );
```

### 2. 自定义中间件示例

```rust
/// 速率限制中间件
pub struct RateLimitMiddleware {
    limiter: Arc<RwLock<RateLimiter>>,
}

#[async_trait]
impl ChatModelMiddleware for RateLimitMiddleware {
    async fn before_invoke(
        &self,
        _messages: &mut Vec<Arc<Message>>,
        _options: &mut InvokeOptions<'_>,
        context: &MiddlewareContext,
    ) -> Result<(), ModelError> {
        // 等待速率限制器允许
        self.limiter.write().unwrap().wait().await;
        tracing::debug!(request_id = %context.request_id, "Rate limit passed");
        Ok(())
    }
}

/// 成本追踪中间件
pub struct CostTrackingMiddleware {
    cost_tracker: Arc<RwLock<CostTracker>>,
}

#[async_trait]
impl ChatModelMiddleware for CostTrackingMiddleware {
    async fn after_invoke(
        &self,
        _messages: &[Arc<Message>],
        completion: &mut ChatCompletion,
        context: &MiddlewareContext,
    ) -> Result<(), ModelError> {
        if let Some(usage) = &completion.usage {
            let cost = self.calculate_cost(usage);
            self.cost_tracker.write().unwrap().add_cost(cost);
            
            tracing::info!(
                request_id = %context.request_id,
                tokens = usage.total_tokens,
                cost = cost,
                "API call cost tracked"
            );
        }
        Ok(())
    }
}
```

---

## 最佳实践

### 1. 中间件设计原则

#### 单一职责原则
每个中间件应该只做一件事：
- ✅ `LoggingMiddleware` - 只负责日志
- ✅ `RetryMiddleware` - 只负责重试
- ❌ `LoggingAndRetryMiddleware` - 职责混杂

#### 最小侵入性
中间件不应该修改核心业务逻辑：
```rust
// ✅ 好的实践：观察但不修改
async fn after_run(&self, output: &O) {
    self.log(output);
    // 不修改 output
}

// ❌ 坏的实践：修改业务数据
async fn after_run(&self, output: &mut O) {
    output.data = "modified".to_string(); // 避免
}
```

#### 可组合性
中间件应该可以自由组合：
```rust
let model = ChatOpenAI::new(...)
    .with_middleware(LoggingMiddleware::new())
    .with_middleware(RetryMiddleware::new(3))
    .with_middleware(CacheMiddleware::new(cache))
    .with_middleware(RateLimitMiddleware::new(limiter));
```

### 2. 性能考虑

#### 避免阻塞操作
```rust
// ✅ 异步日志
async fn after_run(&self, output: &O) {
    tokio::spawn(async move {
        self.logger.log_async(output).await;
    });
}

// ❌ 同步阻塞
async fn after_run(&self, output: &O) {
    self.logger.log_sync(output); // 阻塞当前任务
}
```

#### 使用缓冲和批处理
```rust
pub struct BatchingMiddleware {
    buffer: Arc<Mutex<Vec<Event>>>,
    batch_size: usize,
}

async fn on_event(&self, event: Event) {
    let mut buffer = self.buffer.lock().await;
    buffer.push(event);
    
    if buffer.len() >= self.batch_size {
        let batch = std::mem::take(&mut *buffer);
        self.send_batch(batch).await;
    }
}
```

### 3. 错误处理

#### 优雅降级
```rust
async fn on_error(&self, error: &E) -> Result<(), E> {
    // 记录错误，但不阻止执行
    if let Err(log_err) = self.logger.log_error(error).await {
        tracing::warn!("Failed to log error: {}", log_err);
    }
    Ok(()) // 不传播错误
}
```

#### 错误转换
```rust
#[derive(Debug, Error)]
pub enum MiddlewareError {
    #[error("retry exhausted: {0}")]
    RetryExhausted(String),
    #[error("cache error: {0}")]
    Cache(#[from] CacheError),
}

impl From<MiddlewareError> for AgentError {
    fn from(e: MiddlewareError) -> Self {
        AgentError::Agent(e.to_string())
    }
}
```

### 4. 配置管理

#### 使用 Builder 模式
```rust
let middleware = RetryMiddleware::builder()
    .max_retries(3)
    .strategy(RetryStrategy::Exponential {
        base: Duration::from_millis(100),
        max: Duration::from_secs(10),
    })
    .retryable_errors(|e| matches!(e, ModelError::RateLimited))
    .build();
```

#### 环境变量支持
```rust
pub struct LoggingMiddleware {
    level: tracing::Level,
}

impl LoggingMiddleware {
    pub fn from_env() -> Self {
        let level = std::env::var("LOG_LEVEL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(tracing::Level::INFO);
        Self { level }
    }
}
```

### 5. 测试策略

#### 单元测试
```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_retry_middleware() {
        let mut attempt = 0;
        let middleware = RetryMiddleware::new(3);
        
        let result = middleware.call(
            (),
            MiddlewareContext::default(),
            Next::new(|| async {
                attempt += 1;
                if attempt < 3 {
                    Err(AgentError::Agent("temporary".into()))
                } else {
                    Ok(())
                }
            }),
        ).await;
        
        assert!(result.is_ok());
        assert_eq!(attempt, 3);
    }
}
```

#### 集成测试
```rust
#[tokio::test]
async fn test_middleware_stack() {
    let model = TestModel::new();
    let model_with_mw = MiddlewareChatModel::new(model)
        .with_middleware(LoggingMiddleware::new())
        .with_middleware(RetryMiddleware::new(2));
    
    let result = model_with_mw.invoke(&messages, &options).await;
    assert!(result.is_ok());
}
```

---

## 示例代码

### 完整示例：构建一个监控 Agent

```rust
use langchain::{ReactAgent, AgentError};
use langchain_core::{tool, ToolError};
use langchain_openai::ChatOpenAI;
use std::sync::Arc;
use tracing_subscriber;

// 1. 定义工具
#[tool(description = "搜索互联网")]
async fn search(query: String) -> Result<String, ToolError> {
    Ok(format!("搜索结果: {}", query))
}

// 2. 创建指标收集器
struct PrometheusCollector;

impl MetricsCollector for PrometheusCollector {
    fn record(&self, metrics: PerformanceMetrics) {
        // 发送到 Prometheus
        println!("Metrics: {:?}", metrics);
    }
}

// 3. 主函数
#[tokio::main]
async fn main() -> Result<(), AgentError> {
    // 初始化日志
    tracing_subscriber::fmt::init();
    
    // 创建模型
    let model = ChatOpenAI::builder()
        .model("gpt-4")
        .api_key(std::env::var("OPENAI_API_KEY").unwrap())
        .build();
    
    // 包装模型，添加中间件
    let model_with_middleware = MiddlewareChatModel::new(model)
        // 日志记录
        .with_middleware(LoggingMiddleware::new()
            .with_level(tracing::Level::INFO)
        )
        // 内容过滤
        .with_middleware(ContentFilterMiddleware::new()
            .with_output_rule(FilterRule::Blacklist(vec![
                "不当内容".to_string(),
            ]))
        )
        // 重试逻辑
        .with_middleware(RetryMiddleware::new(3)
            .with_strategy(RetryStrategy::Exponential {
                base: Duration::from_millis(500),
                max: Duration::from_secs(30),
            })
        );
    
    // 创建 Agent
    let agent = ReactAgent::builder(model_with_middleware)
        .with_tools(vec![search_tool()])
        .with_system_prompt("你是一个有用的助手")
        .build();
    
    // 为图添加全局中间件
    let metrics_collector = Arc::new(PrometheusCollector);
    let agent_with_monitoring = agent
        .graph
        .with_global_middleware(
            PerformanceMiddleware::new(metrics_collector)
        )
        .with_global_middleware(
            LoggingMiddleware::new()
        );
    
    // 执行查询
    let result = agent_with_monitoring
        .invoke(Message::user("搜索 Rust 编程语言"), None)
        .await?;
    
    println!("结果: {:?}", result.last_assistant());
    
    Ok(())
}
```

### 自定义中间件示例

```rust
/// 自定义中间件：Token 计数器
pub struct TokenCounterMiddleware {
    counter: Arc<AtomicUsize>,
}

impl TokenCounterMiddleware {
    pub fn new() -> Self {
        Self {
            counter: Arc::new(AtomicUsize::new(0)),
        }
    }
    
    pub fn get_count(&self) -> usize {
        self.counter.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[async_trait]
impl ChatModelMiddleware for TokenCounterMiddleware {
    async fn after_invoke(
        &self,
        _messages: &[Arc<Message>],
        completion: &mut ChatCompletion,
        _context: &MiddlewareContext,
    ) -> Result<(), ModelError> {
        if let Some(usage) = &completion.usage {
            self.counter.fetch_add(
                usage.total_tokens as usize,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        Ok(())
    }
}

// 使用
let token_counter = Arc::new(TokenCounterMiddleware::new());
let model = MiddlewareChatModel::new(base_model)
    .with_middleware(token_counter.clone());

// 执行多次调用
model.invoke(&messages, &options).await?;

// 获取总 token 数
println!("Total tokens used: {}", token_counter.get_count());
```

---

## 总结

### 关键设计决策

1. **Trait-based 架构**
   - 使用 `Middleware`, `NodeMiddleware`, `ChatModelMiddleware` trait 提供灵活性
   - 支持泛型，确保类型安全

2. **洋葱模型**
   - 中间件按照栈的顺序执行（先进后出）
   - 支持 before/after 钩子

3. **非侵入式集成**
   - 中间件通过包装器模式集成
   - 不修改核心 trait 定义

4. **异步优先**
   - 所有中间件都是异步的
   - 支持流式处理

5. **可组合性**
   - 中间件可以自由组合和嵌套
   - 支持全局和节点级中间件

### 实现路线图

**Phase 1: 核心基础设施**
- [ ] 实现 `Middleware`, `NodeMiddleware`, `ChatModelMiddleware` trait
- [ ] 实现 `MiddlewareContext` 和 `MiddlewareStack`
- [ ] 集成到 `Node` 和 `ChatModel`

**Phase 2: 内置中间件**
- [ ] `LoggingMiddleware` - 日志记录
- [ ] `RetryMiddleware` - 重试逻辑
- [ ] `PerformanceMiddleware` - 性能监控
- [ ] `CacheMiddleware` - 缓存支持

**Phase 3: 高级功能**
- [ ] `ContentFilterMiddleware` - 内容过滤
- [ ] `RateLimitMiddleware` - 速率限制
- [ ] `CostTrackingMiddleware` - 成本追踪
- [ ] 流式中间件支持

**Phase 4: 生态集成**
- [ ] OpenTelemetry 集成
- [ ] Prometheus metrics 导出
- [ ] 分布式追踪支持

### 参考资源

- [LangChain Python Callbacks](https://python.langchain.com/docs/modules/callbacks/)
- [Tower Middleware](https://docs.rs/tower/latest/tower/)
- [Rust Async Book](https://rust-lang.github.io/async-book/)
- [Tracing](https://docs.rs/tracing/latest/tracing/)

---

*文档版本: 1.0.0*  
*创建日期: 2026-02-12*  
*作者: langchain-rs 社区*
