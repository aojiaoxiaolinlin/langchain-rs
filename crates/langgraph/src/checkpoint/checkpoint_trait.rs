use async_trait::async_trait;

use crate::checkpoint::{
    CheckpointId, CheckpointListResult, CheckpointQuery,
    checkpoint_struct_api::{Checkpoint, CheckpointMetadata},
};

/// 检查点保存器接口
#[async_trait]
pub trait Checkpointer<S>: Send + Sync {
    // ========== 基础操作 ==========

    /// 获取指定线程的最新检查点
    async fn get(&self, thread_id: &str) -> Result<Option<Checkpoint<S>>, CheckpointError>;

    /// 保存检查点
    async fn put(&self, checkpoint: &Checkpoint<S>) -> Result<(), CheckpointError>;

    /// 删除指定线程的所有检查点
    async fn delete(&self, thread_id: &str) -> Result<(), CheckpointError>;

    /// 删除指定检查点
    async fn delete_checkpoint(&self, checkpoint_id: &CheckpointId) -> Result<(), CheckpointError>;

    // ========== 查询操作 ==========

    /// 列出指定线程的所有检查点（元数据）
    async fn list(
        &self,
        thread_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<CheckpointMetadata>, CheckpointError>;

    /// 根据查询条件搜索检查点
    async fn search(&self, query: CheckpointQuery)
    -> Result<CheckpointListResult, CheckpointError>;

    /// 获取指定检查点的完整数据
    async fn get_by_id(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Option<Checkpoint<S>>, CheckpointError>;

    /// 获取指定检查点的元数据
    async fn get_metadata(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Option<CheckpointMetadata>, CheckpointError>;

    /// 获取指定检查点的元数据
    async fn get_metadata_parent_id(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Option<String>, CheckpointError>;

    // 获取指定检查点的元数据
    // async fn get_metadata_parent_id_by_thread_id(
    //     &self,
    //     thread_id: &str,
    // ) -> Result<Option<String>, CheckpointError>;

    // ========== 版本管理 ==========

    /// 获取检查点的历史链（从根到当前检查点）
    async fn get_history(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Vec<CheckpointMetadata>, CheckpointError>;

    /// 获取指定时间点的检查点
    async fn get_at_time(
        &self,
        thread_id: &str,
        time: i64,
    ) -> Result<Option<Checkpoint<S>>, CheckpointError>;

    // ========== 维护操作 ==========

    /// 清理旧检查点
    async fn cleanup(&self, policy: &CleanupPolicy) -> Result<usize, CheckpointError>;

    /// 获取存储统计信息
    async fn stats(&self, thread_id: Option<&str>) -> Result<CheckpointStats, CheckpointError>;
}

/// 检查点错误类型
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Checkpoint not found: {0}")]
    NotFound(String),

    #[error("Invalid checkpoint ID: {0}")]
    InvalidId(String),

    #[error("Concurrency conflict: {0}")]
    Conflict(String),
}

/// 清理策略
#[derive(Debug, Clone)]
pub enum CleanupPolicy {
    /// 保留最近 N 个检查点
    KeepLast(usize),

    /// 保留最近 N 天的检查点
    KeepDays(i64),

    /// 保留检查点直到总大小超过限制
    KeepMaxSizeBytes(usize),
    // 自定义清理条件
    // Custom(Box<dyn Fn(&CheckpointMetadata) -> bool + Send + Sync>),
}

/// 检查点统计信息
#[derive(Debug, Clone)]
pub struct CheckpointStats {
    /// 检查点总数
    pub total_count: usize,
    /// 总存储大小（字节）
    pub total_size_bytes: usize,
    /// 最早检查点时间
    pub oldest_checkpoint: Option<i64>,
    /// 最新检查点时间
    pub newest_checkpoint: Option<i64>,
}
