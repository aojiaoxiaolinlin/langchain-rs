# langchain-rs

`langchain-rs` 是一个 **LangChain** 和 **LangGraph** 概念的 Rust 实现。它旨在提供一个高性能、类型安全的框架，用于构建基于 LLM（大语言模型）的有状态智能体（Agents）和工作流。

本项目不仅实现了基本的 LLM 调用，还核心实现了 **LangGraph** 的图执行引擎，支持构建复杂的、有状态的、多步骤的 AI 应用。

## 核心特性 (Features)

- **LangGraph Rust 实现**: 提供了完整的图执行引擎 (`langgraph`)，支持定义节点 (`Node`)、边 (`Edge`) 和状态图 (`StateGraph`)。
- **ReAct Agent**: 内置了标准的 ReAct (Reasoning + Acting) Agent 实现，支持工具调用和推理循环。
- **OpenAI 集成**: 通过 `langchain_openai` 提供了对 OpenAI 及其兼容接口（如 DeepSeek, SiliconFlow 等）的支持。
- **宏支持 (Macros)**: 提供了 `#[tool]` 宏，极大地简化了自定义工具的定义过程。
- **类型安全**: 利用 Rust 的类型系统保证状态传递和工具调用的安全性。
- **异步支持**: 全异步设计 (`tokio`, `async-trait`)，支持流式输出 (`Stream`)。

## 项目结构 (Project Structure)

本项目是一个 Cargo Workspace，包含以下主要 Crates：

| Crate | 描述 |
|-------|------|
| **`langchain`** | 顶层库，整合了核心功能，提供 `ReactAgent` 等高层抽象。包含示例代码。 |
| **`langgraph`** | 核心图执行引擎。定义了 `StateGraph`, `Node`, `Executor` 等核心组件。实现了复杂的图遍历和状态管理逻辑。 |
| **`langchain_core`** | 核心定义库。包含 `Message`, `State`, `Tool`, `Request/Response` 等基础类型定义。 |
| **`langchain_openai`** | OpenAI 聊天模型接口实现。支持配置 Base URL 和 API Key。 |

## 快速开始 (Quick Start)

### 1. 依赖配置

在你的 `Cargo.toml` 中添加依赖（假设从本地路径引入，实际使用请根据发布情况调整）：

```toml
[dependencies]
langchain = { path = "crates/langchain" }
tokio = { version = "1", features = ["full"] }
```

### 2. 定义工具

使用 `#[tool]` 宏轻松定义 LLM 可调用的工具：

```rust
use langchain_core::tool;

#[tool(
    description = "计算两个数字的和",
    args(a = "第一个数字", b = "第二个数字")
)]
async fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[tool(
    description = "计算两个数字的差",
    args(a = "被减数", b = "减数")
)]
async fn subtract(a: i32, b: i32) -> i32 {
    a - b
}
```

### 3. 创建并运行 Agent

以下示例展示了如何创建一个 ReAct Agent 并使用 DeepSeek/OpenAI 模型进行计算：

```rust
use langchain::ReactAgent;
use langchain_core::message::Message;
use langchain_openai::ChatOpenAIBuilder;
use std::env;

#[tokio::main]
async fn main() {
    // 1. 设置 API Key 和模型配置
    let api_key = env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    // 示例使用 SiliconFlow 的 DeepSeek 模型，也可以直接使用 OpenAI
    let base_url = "https://api.siliconflow.cn/v1"; 
    let model_name = "deepseek-ai/DeepSeek-V3.2";

    let model = ChatOpenAIBuilder::from_base(model_name, base_url, &api_key).build();

    // 2. 创建 Agent 并注册工具
    // 系统提示词可以指导 Agent 的行为模式
    let agent = ReactAgent::create_agent(model, vec![add_tool(), subtract_tool()])
        .with_system_prompt(
            "你是一个智能助手，可以使用提供的工具来回答问题。支持并行工具调用。".to_string(),
        );

    // 3. 调用 Agent
    let response = agent
        .invoke(Message::user("计算100和200的和，同时计算999减去800的差"))
        .await
        .unwrap();

    // 4. 获取结果
    if let Some(last_msg) = response.messages.last() {
        println!("Agent 回复: {:?}", last_msg.content);
    }
}
```

## 架构设计说明 (Architecture Highlights)

### LangGraph 核心
`langgraph` 并非简单地硬编码执行流，而是设计了一个灵活的图运行时：
- **Label System**: 实现了一套动态标签系统（`GraphLabel`），支持任意类型作为图的节点标识，利用 `DynEq` 和 `DynHash` 实现 Trait 对象的相等性和哈希计算。
- **Executor**: 图执行器不预设具体的调度策略（如并行 vs 串行），而是提供底层机制计算“后继节点”，允许上层构建复杂的路由逻辑（如条件分支、循环）。

### 状态管理
Agent 的状态通过 `MessagesState` 管理，它是一个 append-only 的消息列表。每次 LLM 调用或工具执行都会产生新的状态，通过图的边流转。

## 环境变量

运行示例或使用 OpenAI 模块时，需要设置以下环境变量：

- `OPENAI_API_KEY`: 你的 LLM 服务提供商的 API Key。

## 示例

查看 `crates/langchain/examples` 目录获取更多示例：
- `agent_openai.rs`: 基础的 OpenAI Agent 示例。
- `agent_openai_stream.rs`: 流式输出示例（如果存在）。
