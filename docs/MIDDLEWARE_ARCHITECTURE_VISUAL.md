# langchain-rs 中间件架构可视化

本文档通过 ASCII 图表和流程图，帮助你快速理解 langchain-rs 中间件系统的架构设计。

## 目录
1. [整体架构](#整体架构)
2. [中间件执行流程](#中间件执行流程)
3. [中间件类型层次](#中间件类型层次)
4. [集成模式](#集成模式)
5. [使用示例](#使用示例)

---

## 整体架构

### 系统分层视图

```
┌─────────────────────────────────────────────────────────────┐
│                      用户应用层                               │
│              (Agent 定义, 工具注册, 业务逻辑)                 │
└─────────────────────┬───────────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────────┐
│                  中间件层 (Middleware Layer)                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  观察性中间件 (Observability)                        │   │
│  │  • LoggingMiddleware                                 │   │
│  │  • PerformanceMiddleware                             │   │
│  │  • TracingMiddleware                                 │   │
│  └─────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  可靠性中间件 (Reliability)                          │   │
│  │  • RetryMiddleware                                   │   │
│  │  • TimeoutMiddleware                                 │   │
│  │  • CircuitBreakerMiddleware                          │   │
│  └─────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  安全性中间件 (Security)                             │   │
│  │  • ContentFilterMiddleware                           │   │
│  │  • RateLimitMiddleware                               │   │
│  │  • AuthenticationMiddleware                          │   │
│  └─────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  优化中间件 (Optimization)                           │   │
│  │  • CacheMiddleware                                   │   │
│  │  • CompressionMiddleware                             │   │
│  │  • BatchingMiddleware                                │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────┬───────────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────────┐
│                  执行层 (Execution Layer)                    │
│  • StateGraph 执行器                                         │
│  • Node 运行时                                               │
│  • EventSink 事件系统                                        │
└─────────────────────┬───────────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────────┐
│                  核心层 (Core Layer)                         │
│  • ChatModel trait                                           │
│  • Message 类型                                              │
│  • Tool 系统                                                 │
└─────────────────────────────────────────────────────────────┘
```

---

## 中间件执行流程

### 洋葱模型（Onion Model）

```
请求流向 →

┌────────────────────────────────────────────────────────────┐
│  LoggingMiddleware (最外层)                                 │
│  ┌────────────────────────────────────────────────────┐   │
│  │  RetryMiddleware                                    │   │
│  │  ┌────────────────────────────────────────────┐   │   │
│  │  │  CacheMiddleware                            │   │   │
│  │  │  ┌────────────────────────────────────┐   │   │   │
│  │  │  │  ContentFilterMiddleware            │   │   │   │
│  │  │  │  ┌────────────────────────────┐   │   │   │   │
│  │  │  │  │                             │   │   │   │   │
│  │  │  │  │   核心业务逻辑               │   │   │   │   │
│  │  │  │  │   (ChatModel / Node)        │   │   │   │   │
│  │  │  │  │                             │   │   │   │   │
│  │  │  │  └────────────────────────────┘   │   │   │   │
│  │  │  └────────────────────────────────────┘   │   │   │
│  │  └────────────────────────────────────────────┘   │   │
│  └────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────┘

← 响应流向

执行顺序:
  请求: Log → Retry → Cache → Filter → 核心逻辑
  响应: 核心逻辑 → Filter → Cache → Retry → Log
```

### 详细执行时序

```
时间 ↓

  用户请求
     │
     ▼
┌─────────────────────┐
│ LoggingMiddleware   │──── before_run()
│   记录请求开始       │
└──────┬──────────────┘
       │
       ▼
┌─────────────────────┐
│ RetryMiddleware     │──── 尝试 1
│   准备重试逻辑       │
└──────┬──────────────┘
       │
       ▼
┌─────────────────────┐
│ CacheMiddleware     │──── 检查缓存
│   缓存未命中         │
└──────┬──────────────┘
       │
       ▼
┌─────────────────────┐
│ ContentFilter       │──── 过滤输入
│   清理敏感内容       │
└──────┬──────────────┘
       │
       ▼
┌─────────────────────┐
│   核心业务逻辑       │──── 执行 LLM 调用
│   ChatModel.invoke  │
└──────┬──────────────┘
       │
       ▼ (成功)
┌─────────────────────┐
│ ContentFilter       │──── 过滤输出
│   清理敏感内容       │
└──────┬──────────────┘
       │
       ▼
┌─────────────────────┐
│ CacheMiddleware     │──── 保存到缓存
│   写入缓存           │
└──────┬──────────────┘
       │
       ▼
┌─────────────────────┐
│ RetryMiddleware     │──── 成功，无需重试
│   记录成功           │
└──────┬──────────────┘
       │
       ▼
┌─────────────────────┐
│ LoggingMiddleware   │──── after_run()
│   记录响应           │
└──────┬──────────────┘
       │
       ▼
   返回给用户
```

---

## 中间件类型层次

### Trait 继承关系

```
┌──────────────────────────────────────────┐
│         Middleware<I, O, E>               │
│         (通用中间件)                      │
│                                           │
│  + async fn call(input, ctx, next)       │
│     -> Result<O, E>                       │
└──────────────┬───────────────────────────┘
               │
               ├──────────────────────────────────────┐
               │                                       │
┌──────────────▼───────────────┐    ┌─────────────────▼──────────────┐
│ NodeMiddleware<I, O, E, Ev>  │    │  ChatModelMiddleware            │
│ (节点级中间件)                │    │  (模型级中间件)                 │
│                               │    │                                 │
│ + async fn before_run()       │    │ + async fn before_invoke()      │
│ + async fn after_run()        │    │ + async fn after_invoke()       │
│ + async fn on_error()         │    │ + async fn on_stream_chunk()    │
│ + async fn on_stream_event()  │    │                                 │
└───────────────────────────────┘    └─────────────────────────────────┘
```

### 中间件实现示例

```
抽象层次 (从抽象到具体)

Middleware trait
    │
    ├─→ LoggingMiddleware      (实现通用日志)
    ├─→ RetryMiddleware        (实现重试逻辑)
    └─→ PerformanceMiddleware  (实现性能监控)

NodeMiddleware trait
    │
    ├─→ NodeLoggingMiddleware  (节点级日志)
    ├─→ CacheMiddleware        (节点结果缓存)
    └─→ ValidationMiddleware   (节点输入验证)

ChatModelMiddleware trait
    │
    ├─→ ContentFilterMiddleware  (内容过滤)
    ├─→ TokenCounterMiddleware   (Token 计数)
    └─→ CostTrackingMiddleware   (成本追踪)
```

---

## 集成模式

### 模式 1: ChatModel 包装

```
                原始 ChatModel
                      │
                      ▼
            ┌──────────────────┐
            │  ChatOpenAI      │
            └──────────────────┘
                      │
                      ▼ wrap
┌─────────────────────────────────────────┐
│    MiddlewareChatModel                  │
│                                          │
│  inner: ChatOpenAI                       │
│  middlewares: [                          │
│    LoggingMiddleware,                    │
│    RetryMiddleware,                      │
│    CacheMiddleware                       │
│  ]                                       │
└─────────────────────────────────────────┘
                      │
                      ▼ 实现 ChatModel trait
            ┌──────────────────┐
            │  invoke()        │
            │  stream()        │
            └──────────────────┘
```

### 模式 2: Node 包装

```
           原始 Node
               │
               ▼
     ┌──────────────────┐
     │    LlmNode       │
     └──────────────────┘
               │
               ▼ wrap
┌──────────────────────────────┐
│   MiddlewareNode             │
│                              │
│  inner: LlmNode              │
│  middlewares: [              │
│    ValidationMiddleware,     │
│    LoggingMiddleware         │
│  ]                           │
└──────────────────────────────┘
               │
               ▼ 实现 Node trait
     ┌──────────────────┐
     │  run_sync()      │
     │  run_stream()    │
     └──────────────────┘
```

### 模式 3: StateGraph 全局中间件

```
      StateGraph
          │
          ▼
   with_global_middleware(LoggingMiddleware)
          │
          ▼
┌─────────────────────────────────────┐
│  Graph                              │
│                                     │
│  nodes: {                           │
│    "start": MiddlewareNode(         │
│              inner: StartNode,      │
│              mw: [Logging]          │
│            ),                       │
│    "llm":   MiddlewareNode(         │
│              inner: LlmNode,        │
│              mw: [Logging]          │
│            ),                       │
│    "tool":  MiddlewareNode(         │
│              inner: ToolNode,       │
│              mw: [Logging]          │
│            )                        │
│  }                                  │
└─────────────────────────────────────┘
```

---

## 使用示例

### 示例 1: 基础中间件栈

```rust
// 代码结构
let model = ChatOpenAI::new()
    .with_middleware(M1)  // 最外层
    .with_middleware(M2)  // 中间层
    .with_middleware(M3); // 最内层

// 执行流程
请求 → M1.before → M2.before → M3.before → 核心逻辑
响应 ← M1.after  ← M2.after  ← M3.after  ← 核心逻辑
```

```
可视化:

┌──────────────────────────────────┐
│  M1: LoggingMiddleware           │
│  ┌────────────────────────────┐ │
│  │  M2: RetryMiddleware       │ │
│  │  ┌──────────────────────┐ │ │
│  │  │  M3: CacheMiddleware │ │ │
│  │  │  ┌────────────────┐ │ │ │
│  │  │  │  ChatOpenAI    │ │ │ │
│  │  │  └────────────────┘ │ │ │
│  │  └──────────────────────┘ │ │
│  └────────────────────────────┘ │
└──────────────────────────────────┘
```

### 示例 2: 生产级配置

```rust
let agent = ReactAgent::builder(model)
    .with_tools(tools)
    .build()
    // 全局中间件
    .graph
    .with_global_middleware(PerformanceMiddleware)
    .with_global_middleware(LoggingMiddleware)
    // 特定节点中间件
    .with_node_middleware(
        LlmLabel, 
        CacheMiddleware
    );
```

```
图结构:

    Start (有 Perf + Log)
      │
      ▼
    LLM (有 Perf + Log + Cache)
      │
      ├─→ Tool (有 Perf + Log)
      │     │
      │     └─→ 返回 LLM
      │
      └─→ End (有 Perf + Log)
```

### 示例 3: 错误处理流程

```
正常流程:
  请求 → M1 → M2 → M3 → 业务逻辑 → M3 → M2 → M1 → 响应

错误流程 (在 M3 发生错误):
  请求 → M1.before → M2.before → M3.before → 业务逻辑 ✗ 错误
                                                │
  响应 ← M1.on_error ← M2.on_error ← M3.on_error ┘

重试流程 (M2 是 RetryMiddleware):
  请求 → M1 → M2 ──┐
                  │
                  ├─ 尝试 1 → M3 → 业务逻辑 ✗
                  │                    │
                  ├─ 延迟 100ms ←──────┘
                  │
                  ├─ 尝试 2 → M3 → 业务逻辑 ✗
                  │                    │
                  ├─ 延迟 200ms ←──────┘
                  │
                  └─ 尝试 3 → M3 → 业务逻辑 ✓ 成功
                                       │
  响应 ← M1 ← M2 ←─────────────────────┘
```

---

## 关键概念总结

### 1. 中间件链（Middleware Chain）

```
中间件链 = [M1, M2, M3, ..., Mn]

执行顺序:
  before: M1 → M2 → M3 → ... → Mn → 核心
  after:  M1 ← M2 ← M3 ← ... ← Mn ← 核心
```

### 2. 上下文传播（Context Propagation）

```
MiddlewareContext {
  request_id: "uuid-123",
  timestamp: SystemTime::now(),
  metadata: {
    "user_id": "user-456",
    "start_time": "...",
    "custom_key": "custom_value"
  }
}
    │
    ├─→ 传递给 M1
    ├─→ 传递给 M2
    └─→ 传递给 M3
```

### 3. Next 闭包（Next Closure）

```rust
Next {
  inner: Box<dyn Fn(I, Ctx) -> Future<Result<O, E>>>
}

// 调用链:
M1.call(input, ctx, next1)
  └─→ next1.call(input, ctx)
        └─→ M2.call(input, ctx, next2)
              └─→ next2.call(input, ctx)
                    └─→ M3.call(input, ctx, next3)
                          └─→ next3.call(input, ctx)
                                └─→ 核心逻辑
```

---

## 性能优化

### 零成本抽象

```
源代码:
  model.with_middleware(M1)
       .with_middleware(M2)
       .with_middleware(M3)

编译后 (单态化):
  MiddlewareChatModel::<ChatOpenAI, M1, M2, M3>
    - 无虚函数调用
    - 内联优化
    - 零运行时开销
```

### 并发执行

```
串行中间件:
  M1 → M2 → M3 → 核心
  
并行节点 (在 Graph 中):
  ┌─→ Node1 (有中间件栈) ─┐
  │                      │
Start                    ├─→ End
  │                      │
  └─→ Node2 (有中间件栈) ─┘
```

---

*创建日期: 2026-02-12*  
*文档版本: 1.0.0*
