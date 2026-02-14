# langchain-rs 架构设计文档

## 目录

1. [概述](#概述)
2. [整体架构](#整体架构)
3. [核心层次](#核心层次)
4. [模块详解](#模块详解)
5. [设计模式](#设计模式)
6. [执行流程](#执行流程)
7. [技术栈](#技术栈)

---

## 概述

**langchain-rs** 是 LangChain 和 LangGraph 概念的 Rust 实现，是一个高性能、类型安全的框架，用于构建基于 LLM 的有状态智能体和工作流。

### 核心设计理念

- **类型安全优先**：充分利用 Rust 的类型系统确保编译时安全
- **异步优先设计**：基于 Tokio 的异步 I/O，支持高并发场景
- **分层清晰**：核心抽象 → 图引擎 → 框架 → 实现，层次分明
- **标准兼容**：遵循 OpenAI API 标准，确保最大兼容性
- **开发者友好**：丰富的宏系统和清晰的 API 设计

---

## 整体架构

### Crate 层次结构

```
┌─────────────────────────────────────────────────────┐
│              langchain_openai                        │
│         (OpenAI 适配器实现)                          │
└────────────────┬────────────────────────────────────┘
                 │ 依赖
┌────────────────▼────────────────────────────────────┐
│               langchain                              │
│       (ReAct Agent 高层框架)                        │
│  ┌─────────────────────────────────────────────┐   │
│  │  • ReactAgent                               │   │
│  │  • LlmNode<M>                               │   │
│  │  • ToolNode<E>                              │   │
│  └─────────────────────────────────────────────┘   │
└────────────────┬────────────────────────────────────┘
                 │ 依赖
┌────────────────▼────────────────────────────────────┐
│               langgraph                              │
│         (图执行引擎)                                 │
│  ┌─────────────────────────────────────────────┐   │
│  │  • Graph<I,O,E,Ev>                          │   │
│  │  • StateGraph<S,U,E,Ev>                     │   │
│  │  • Node trait                               │   │
│  │  • GraphLabel trait                         │   │
│  │  • Checkpointer                             │   │
│  │  • RunStrategy                              │   │
│  └─────────────────────────────────────────────┘   │
└────────────────┬────────────────────────────────────┘
                 │ 依赖
┌────────────────▼────────────────────────────────────┐
│            langchain_core                            │
│       (核心抽象和基础类型)                          │
│  ┌─────────────────────────────────────────────┐   │
│  │  • Message 枚举                             │   │
│  │  • MessagesState                            │   │
│  │  • ChatModel trait                          │   │
│  │  • RegisteredTool<E>                        │   │
│  │  • #[tool] 宏                               │   │
│  └─────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────┘
```

### 依赖关系图

```
langchain_openai
    │
    ├─→ langchain_core (依赖 OpenAI 兼容的类型)
    │
langchain
    │
    ├─→ langgraph (图执行能力)
    ├─→ langchain_core (核心类型)
    │
langgraph
    │
    ├─→ tokio (异步运行时)
    ├─→ futures (流式处理)
    ├─→ tracing (日志记录)
    │
langchain_core
    │
    ├─→ serde (序列化)
    ├─→ schemars (JSON schema)
    ├─→ im (不可变集合)
    └─→ async-trait (异步 trait)
```

---

## 核心层次

### 1️⃣ langchain_core - 基础抽象层

**职责**：提供框架的核心抽象、trait 和数据结构

#### 核心类型

```rust
// 消息类型系统
pub enum Message {
    User { content: Content, name: Option<String> },
    Assistant {
        content: String,
        tool_calls: Option<Vec<ToolCall>>,
        name: Option<String>
    },
    System { content: String, name: Option<String> },
    Developer { content: String, name: Option<String> },
    Tool { tool_call_id: String, content: String },
}

// 状态管理（不可变、仅追加）
pub struct MessagesState {
    pub messages: Vector<Arc<Message>>,  // 持久化数据结构
    pub llm_calls: u32,                  // LLM 调用计数器
}

// ChatModel trait - 所有 LLM 实现的抽象
#[async_trait]
pub trait ChatModel: Send + Sync {
    async fn invoke(
        &self,
        messages: &[Arc<Message>],
        options: &InvokeOptions<'_>
    ) -> Result<ChatCompletion>;

    async fn stream(
        &self,
        messages: &[Arc<Message>],
        options: &InvokeOptions<'_>
    ) -> Result<StandardChatStream>;
}

// 工具注册系统
pub struct RegisteredTool<E> {
    pub function: Function,  // JSON schema
    pub handler: Box<ToolFn<E>>,  // 执行函数
}
```

#### 工具宏系统

```rust
#[tool(description = "搜索工具")]
async fn search(
    #[arg(description = "搜索关键词")] query: String
) -> Result<String, ToolError> {
    Ok(format!("搜索结果: {}", query))
}

// 自动生成：
// 1. SearchArgs 结构体（带 serde/schemars derives）
// 2. search_tool() 函数返回 RegisteredTool<ToolError>
// 3. JSON schema 用于 OpenAI function calling
```

---

### 2️⃣ langgraph - 图执行引擎

**职责**：提供有向图执行能力，支持条件分支、并行执行和检查点

#### 核心设计哲学

> **最小化图引擎**：只计算"有哪些后继节点"，但**不决定"如何执行它们"**
> - 图层提供后继节点计算
> - 执行策略由 `RunStrategy` 决策
> - 这种分离支持多种执行语义

#### 核心类型

```rust
// 通用图结构
pub struct Graph<I, O, E, Ev> {
    nodes: HashMap<InternedGraphLabel, NodeState<I, O, E, Ev>>,
    edges: HashMap<InternedGraphLabel, Vec<Edge>>,
}

// 简化的状态图 API
pub struct StateGraph<S, U, E, Ev> {
    pub graph: Graph<S, U, E, Ev>,
    pub reducer: Reducer<S, U>,  // 状态合并函数
    pub entry: InternedGraphLabel,
    pub checkpointer: Option<Arc<dyn Checkpointer>>,
}

// 节点 trait
#[async_trait]
pub trait Node<I, O, E, Ev>: Send + Sync {
    async fn run_sync(&self, input: &I) -> Result<O, E>;

    async fn run_stream(
        &self,
        input: &I,
        sink: &mut dyn EventSink<Ev>
    ) -> Result<O, E>;
}

// 执行策略
pub enum RunStrategy {
    StopAtNonLinear,  // 遇到分支时停止
    PickFirst,        // 选择第一个分支
    PickLast,         // 选择最后一个分支
    Parallel,         // 并行执行所有分支
}
```

#### 标签系统

```rust
// 动态类型擦除的节点标签
pub trait GraphLabel: Send + Sync + DynClone + DynEq + DynHash {
    fn as_str(&self) -> &str;
    fn dyn_clone(&self) -> Box<dyn GraphLabel>;
}

// 使用 #[derive(GraphLabel)] 自动实现
#[derive(Debug, Clone, PartialEq, Eq, Hash, GraphLabel)]
enum MyLabel {
    Start,
    Process,
    End,
}

// 全局内化（Interning）用于去重和快速比较
pub type InternedGraphLabel = &'static GraphLabelDyn;
```

---

### 3️⃣ langchain - 高层 Agent 框架

**职责**：提供面向用户的 Agent 实现，采用 ReAct 模式

#### ReAct Agent 架构

```
用户输入
    │
    ▼
┌─────────┐
│  Start  │
└────┬────┘
     │
     ▼
┌─────────────────────────┐
│   LlmNode (推理)        │
│  • 调用 ChatModel       │
│  • 决定是否使用工具     │
└────┬────────────────────┘
     │
     ├─→ 有 tool_calls ────────┐
     │                        ▼
     │               ┌─────────────────┐
     │               │   ToolNode      │
     │               │  (并行执行工具) │
     └───────────────┴─────────────────┘
                    │
                    ▼ (循环回 LLM)
              (回到 LlmNode)
                    │
                    ▼
                 无 tool_calls
                    │
                    ▼
┌─────────────────────────┐
│       End               │
└─────────────────────────┘
```

#### 核心组件

```rust
// ReAct Agent
pub struct ReactAgent {
    graph: StateGraph<MessagesState, MessagesState, AgentError, ChatStreamEvent>,
    system_prompt: Option<String>,
}

impl ReactAgent {
    pub fn create_agent<M>(
        model: M,
        tools: Vec<RegisteredTool<ToolError>>
    ) -> Self
    where
        M: ChatModel + Send + Sync + 'static;

    pub async fn invoke(
        &self,
        message: Message,
        config: Option<&Configuration>
    ) -> Result<MessagesState, AgentError>;

    pub async fn stream(
        &self,
        message: Message,
        config: Option<&Configuration>
    ) -> Result<impl Stream<Item = ChatStreamEvent>, AgentError>;
}

// LLM 节点
pub struct LlmNode<M>
where
    M: ChatModel + 'static,
{
    pub model: M,
    pub tools: Vec<ToolSpec>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

// 工具节点（并行执行）
pub struct ToolNode<E>
where
    E: Send + Sync + 'static,
{
    pub tools: HashMap<String, Box<ToolFn<E>>>,
}
```

#### Agent 执行示例

```rust
// 1. 创建模型
let model = ChatOpenAI::builder()
    .model("gpt-4")
    .api_key("sk-...")
    .build();

// 2. 定义工具
#[tool(description = "获取天气")]
async fn get_weather(city: String) -> Result<String, ToolError> {
    Ok(format!("{} 的天气是晴天", city))
}

// 3. 创建 Agent
let agent = ReactAgent::create_agent(model, vec![get_weather_tool()])
    .with_system_prompt("你是一个有用的助手");

// 4. 执行
let result = agent.invoke(Message::user("北京天气怎么样？"), None).await?;
```

---

### 4️⃣ langchain_openai - OpenAI 实现

**职责**：实现 OpenAI 兼容的 HTTP 客户端

#### 核心实现

```rust
pub struct ChatOpenAI {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
    default_temperature: Option<f32>,
    default_max_tokens: Option<u32>,
}

impl ChatOpenAI {
    pub fn builder() -> ChatOpenAIBuilder {
        ChatOpenAIBuilder::default()
    }
}

#[async_trait]
impl ChatModel for ChatOpenAI {
    async fn invoke(
        &self,
        messages: &[Arc<Message>],
        options: &InvokeOptions<'_>
    ) -> Result<ChatCompletion> {
        // HTTP POST /v1/chat/completions
        // 解析响应为 ChatCompletion
    }

    async fn stream(
        &self,
        messages: &[Arc<Message>],
        options: &InvokeOptions<'_>
    ) -> Result<StandardChatStream> {
        // SSE 流式响应解析
    }
}
```

#### 流式处理

```rust
// SSE (Server-Sent Events) 解析
pub struct StandardChatStream {
    stream: Pin<Box<dyn Stream<Item = Result<ChatStreamEvent>> + Send>>,
}

pub enum ChatStreamEvent {
    Content(String),  // 文本增量
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        type_name: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
    Done {
        finish_reason: Option<String>,
        usage: Option<Usage>,
    },
}
```

---

## 设计模式

### 1. 类型安全的错误处理

```rust
// 所有组件都使用泛型错误类型
pub trait ChatModel {
    async fn invoke(...) -> Result<ChatCompletion, Box<dyn Error + Send + Sync>>;
}

// 用户可以自定义错误类型
#[derive(Debug, Error)]
pub enum MyError {
    #[error("model error: {0}")]
    Model(#[from] ModelError),
    #[error("tool error: {0}")]
    Tool(#[from] ToolError),
}
```

### 2. 不可变状态管理

```rust
// 使用 im::Vector (持久化数据结构)
pub struct MessagesState {
    pub messages: Vector<Arc<Message>>,  // O(1) 复制、O(log n) 更新
    pub llm_calls: u32,
}

// 状态通过 Reducer 合并
let reducer = |mut old: MessagesState, update: MessagesState| {
    old.extend_messages(update.messages);
    old.llm_calls += update.llm_calls;
    old
};
```

### 3. 异步流式处理

```rust
// 所有操作都支持同步和流式两种模式
#[async_trait]
pub trait Node<I, O, E, Ev> {
    async fn run_sync(&self, input: &I) -> Result<O, E>;

    async fn run_stream(
        &self,
        input: &I,
        sink: &mut dyn EventSink<Ev>
    ) -> Result<O, E>;
}

// 流式 API
pub async fn stream(
    &'a self,
    message: Message
) -> Result<impl Stream<Item = ChatStreamEvent> + 'a, AgentError> {
    // 返回事件流
}
```

### 4. 标签内化（Interning）

```rust
// 全局 Interner 减少内存使用
thread_local! {
    static INTERNER: Interner<GraphLabelDyn> = Interner::new();
}

// 标签自动去重
let label1 = MyLabel::Start.intern();  // 第一次：存储
let label2 = MyLabel::Start.intern();  // 后续：复用

// 比较只需指针比较
label1 == label2  // O(1)
```

### 5. Builder 模式

```rust
// 灵活的配置 API
let model = ChatOpenAI::builder()
    .model("gpt-4")
    .base_url("https://api.openai.com/v1")
    .api_key("sk-...")
    .temperature(0.7)
    .max_tokens(2000)
    .build();
```

---

## 执行流程

### 图执行流程

```
1. StateGraph::run(state, config, max_steps, strategy)
   │
   ├─→ 2. run_once(current_node, state)
   │     │
   │     ├─→ 执行节点: node.run_sync(&state)
   │     │     │
   │     │     └─→ 返回 delta (状态增量)
   │     │
   │     ├─→ 合并状态: new_state = reducer(state, delta)
   │     │
   │     └─→ 计算后继: successors = get_successors(current_node)
   │
   ├─→ 3. 根据 successors.len() 决策
   │     │
   │     ├─→ successors.len() == 1
   │     │     └─→ 自动推进到线性后继
   │     │
   │     └─→ successors.len() != 1
   │           └─→ 根据 RunStrategy 决策
   │                 ├─→ StopAtNonLinear: 返回所有后继
   │                 ├─→ PickFirst: 选择第一个
   │                 ├─→ PickLast: 选择最后一个
   │                 └─→ Parallel: 并行执行
   │
   └─→ 4. 重复直到：
         ├─→ 无后继节点
         ├─→ 达到 max_steps
         └─→ 遇到错误
```

### Agent 执行示例

```rust
// 用户输入: "北京天气怎么样？"

// 步骤 1: Start → LlmNode
// LLM 决定需要调用 get_weather 工具
MessagesState {
    messages: [
        User("北京天气怎么样？"),
        Assistant("", tool_calls=[
            ToolCall { name: "get_weather", args: {"city": "北京"} }
        ])
    ],
    llm_calls: 1
}

// 步骤 2: LlmNode → ToolNode (条件路由)
// 并行执行工具调用
MessagesState {
    messages: [
        User("北京天气怎么样？"),
        Assistant("", tool_calls=[...]),
        Tool(tool_call_id="call_123", content="北京 的天气是晴天")
    ],
    llm_calls: 1
}

// 步骤 3: ToolNode → LlmNode (循环)
// LLM 基于工具结果生成最终回复
MessagesState {
    messages: [
        User("北京天气怎么样？"),
        Assistant("", tool_calls=[...]),
        Tool(...),
        Assistant("北京今天天气晴朗，适合出行！")
    ],
    llm_calls: 2
}

// 步骤 4: LlmNode → End (条件路由：无 tool_calls)
// 执行完成
```

---

## 技术栈

### 核心依赖

| 分类 | Crate | 用途 |
|------|-------|------|
| **异步运行时** | tokio 1.48.0 | 异步执行器 |
| **异步工具** | async-trait | 异步 trait 支持 |
| **流处理** | async-stream, futures-util | 异步流 |
| **HTTP** | reqwest 0.12.24 | HTTP 客户端 |
| **序列化** | serde, serde_json | JSON 序列化 |
| **Schema** | schemars | JSON schema 生成 |
| **类型系统** | downcast-rs | 类型转换 |
| **错误处理** | thiserror, anyhow | 错误派生 |
| **数据结构** | im | 不可变集合 |
| **过程宏** | syn, proc-macro2, quote | 宏实现 |
| **属性解析** | darling | 属性解析 |
| **日志** | tracing, tracing-subscriber | 结构化日志 |

### 版本要求

- **Rust**: 2024 edition
- **MSRV**: 待定
- **Tier 1 Support**: Linux, macOS, Windows

---

## 总结

### 架构优势

1. **类型安全**：Rust 类型系统确保编译时安全，避免运行时错误
2. **高性能**：异步 I/O、零拷贝设计、内存优化
3. **可扩展**：插件化架构，易于添加新模型和工具
4. **标准兼容**：遵循 OpenAI 标准，生态兼容性好
5. **开发者友好**：丰富的宏系统和清晰的 API 设计

### 设计原则

- **分离关注点**：图引擎 vs 执行策略，核心类型 vs 具体实现
- **不可变性**：状态不可变，通过 Reducer 合并
- **异步优先**：所有 I/O 操作都是异步的
- **流式支持**：同步和流式两种执行模式
- **错误透明**：错误信息传播给 LLM，让其决定是否重试

### 适用场景

- ✅ **ReAct Agent**：推理 + 行动循环的智能体
- ✅ **多步工作流**：复杂的 LLM 应用流程
- ✅ **工具调用**：集成外部 API 和功能
- ✅ **流式输出**：实时响应的对话系统
- ✅ **状态持久化**：支持检查点的长期任务

---

*文档版本: 0.1.0*
*最后更新: 2025-12-27*
