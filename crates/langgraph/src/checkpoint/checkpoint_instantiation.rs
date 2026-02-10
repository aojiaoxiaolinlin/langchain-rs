use crate::{
    checkpoint::{CheckpointId, CheckpointType},
    interrupt::Interrupt,
};
use async_trait::async_trait;
use langchain_core::request::ResponseFormat;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{collections::HashMap, default, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

/// 检查点元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMetadata {
    /// 检查点唯一 ID
    pub id: CheckpointId,
    /// 父检查点 ID（用于构建检查点链）
    pub parent_id: Option<CheckpointId>,
    /// 线程 ID
    pub thread_id: String,
    /// 创建时间,时间戳方便序列化
    pub created_at: i64,
    /// 执行步数
    pub step: usize,
    // 执行的节点名称
    // pub source_node: String,
    /// 用户自定义标签
    pub tags: HashMap<String, String>,
    /// 检查点类型
    pub checkpoint_type: CheckpointType,
}

impl CheckpointMetadata {
    /// 创建 `CheckpointType::Final` 元数据
    fn new_final(thread_id: String, step: usize, parent_id: Option<String>) -> Self {
        CheckpointMetadata {
            id: Uuid::now_v7().to_string(),
            parent_id,
            thread_id,
            created_at: chrono::Utc::now().timestamp(),
            step,
            // source_node: "".to_string(),
            tags: HashMap::new(),
            checkpoint_type: CheckpointType::Final,
        }
    }

    /// 创建 `CheckpointType::Auto` 元数据
    fn new_auto(thread_id: String, step: usize, parent_id: Option<String>) -> Self {
        CheckpointMetadata {
            id: Uuid::now_v7().to_string(),
            parent_id,
            thread_id,
            created_at: chrono::Utc::now().timestamp(),
            step,
            tags: HashMap::new(),
            checkpoint_type: CheckpointType::Auto,
        }
    }
}

/// 检查点数据结构，包含业务状态和执行流位置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint<S> {
    /// 元数据
    pub metadata: CheckpointMetadata,
    /// 业务状态 (State)
    pub state: S,
    /// 下一步需要执行的节点 ID 列表
    /// 由于 InternedGraphLabel 无法直接序列化，这里存储字符串形式的 Label
    pub next_nodes: Vec<String>,
    /// 待处理的中断（如果有）
    pub pending_interrupt: Option<Interrupt>,
}

impl<S> Checkpoint<S> {
    /// 创建 `CheckpointType::Final` 元数据
    pub fn new_final(state: S, thread_id: String, step: usize, parent_id: Option<String>) -> Self {
        Checkpoint {
            state: state,
            next_nodes: Vec::new(),
            pending_interrupt: None,
            metadata: CheckpointMetadata::new_final(thread_id.clone(), step, parent_id),
        }
    }

    /// 创建 `CheckpointType::Auto` 元数据
    /// - 存在next_nodes
    pub fn new_auto_with_next_nodes(
        state: S,
        thread_id: String,
        step: usize,
        next_nodes: Vec<String>,
        parent_id: Option<String>,
    ) -> Self {
        Checkpoint {
            state: state,
            next_nodes,
            pending_interrupt: None,
            metadata: CheckpointMetadata::new_auto(thread_id.clone(), step, parent_id),
        }
    }

    /// 创建 `CheckpointType::Auto` 元数据
    pub fn new_auto(state: S, thread_id: String, step: usize, parent_id: Option<String>) -> Self {
        Checkpoint {
            state: state,
            next_nodes: Vec::new(),
            pending_interrupt: None,
            metadata: CheckpointMetadata::new_auto(thread_id.clone(), step, parent_id),
        }
    }
}
