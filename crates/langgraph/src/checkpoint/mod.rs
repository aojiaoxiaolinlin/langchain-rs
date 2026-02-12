mod checkpoint_instantiation;
mod checkpoint_memory_saver;
mod checkpoint_postgres_saver;
mod checkpoint_redis_saver;
mod checkpoint_sqlite_saver;
mod checkpoint_trait;

use langchain_core::request::ResponseFormat;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use checkpoint_instantiation::*;
#[cfg(feature = "memory")]
pub use checkpoint_memory_saver::*;
#[cfg(feature = "postgres")]
pub use checkpoint_postgres_saver::*;
#[cfg(feature = "sqlite")]
pub use checkpoint_sqlite_saver::*;
pub use checkpoint_trait::*;

/// 运行配置
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunnableConfig {
    /// 线程 ID，用于隔离不同的对话或执行流
    pub thread_id: Option<String>,
    /// 响应格式
    pub response_format: Option<ResponseFormat>,
}

<<<<<<< HEAD
pub mod checkpoint_struct_api {
    pub use super::checkpoint_instantiation::*;
    #[cfg(feature = "memory")]
    pub use super::checkpoint_memory_saver::*;
    #[cfg(feature = "postgres")]
    pub use super::checkpoint_postgres_saver::*;
    #[cfg(feature = "redis")]
    pub use super::checkpoint_redis_saver::*;
    #[cfg(feature = "sqlite")]
    pub use super::checkpoint_sqlite_saver::*;
    pub use super::checkpoint_trait::*;
}

=======
>>>>>>> ec47e8e6dfe559833633279e06cc4260beb12e5a
/// 检查点 ID（唯一标识-uuidv7）
pub type CheckpointId = String;

/// 检查点类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CheckpointType {
    /// 自动保存（每步执行后）
    Auto,
    /// 中断前保存
    InterruptBefore,
    /// 中断后保存
    InterruptAfter,
    /// 用户手动保存
    Manual,
    /// 终止状态
    Final,
}

/// 检查点查询条件
#[derive(Debug, Clone, Default)]
pub struct CheckpointQuery {
    /// 线程 ID
    pub thread_id: Option<String>,
    /// 起始时间
    pub start_time: Option<i64>,
    /// 结束时间
    pub end_time: Option<i64>,
    /// 检查点类型
    pub checkpoint_type: Option<CheckpointType>,
    /// 标签过滤
    pub tags: Option<HashMap<String, String>>,
    /// 限制返回数量
    pub limit: Option<usize>,
    /// 排序方式
    pub order: CheckpointOrder,
}

/// 排序方式
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CheckpointOrder {
    /// 按创建时间降序（最新的在前-default）
    #[default]
    Desc,
    /// 按创建时间升序（最旧的在前）
    Asc,
}

/// 检查点列表结果
#[derive(Debug, Clone)]
pub struct CheckpointListResult {
    /// 检查点列表
    pub checkpoints: Vec<CheckpointMetadata>,
    /// 总数
    pub total_count: usize,
}
