use async_trait::async_trait;
use chrono::Utc;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;

use crate::checkpoint::checkpoint_struct_api::{Checkpoint, CheckpointMetadata};
use crate::checkpoint::checkpoint_trait::{
    CheckpointError, CheckpointStats, Checkpointer, CleanupPolicy,
};
use crate::checkpoint::{
    CheckpointId, CheckpointListResult, CheckpointOrder, CheckpointQuery, CheckpointType,
};

/// Redis 检查点保存器配置
#[derive(Debug, Clone)]
pub struct RedisSaverConfig {
    /// Redis 连接 URL
    pub redis_url: String,
    /// 键前缀
    pub key_prefix: String,
    /// 是否启用索引
    pub enable_indexes: bool,
}

impl Default for RedisSaverConfig {
    fn default() -> Self {
        Self {
            redis_url: "redis://localhost:6379/0".to_owned(),
            key_prefix: "langchain_rs_checkpoint".to_owned(),
            enable_indexes: true,
        }
    }
}

impl RedisSaverConfig {
    /// 创建配置
    pub fn new(redis_url: String) -> Self {
        Self {
            redis_url,
            key_prefix: "langchain_rs_checkpoint".to_owned(),
            enable_indexes: true,
        }
    }

    /// 设置键前缀
    pub fn with_prefix(mut self, prefix: String) -> Self {
        self.key_prefix = prefix;
        self
    }

    /// 设置是否启用索引
    pub fn with_indexes(mut self, enable: bool) -> Self {
        self.enable_indexes = enable;
        self
    }
}

/// Redis 检查点保存器
#[derive(Debug, Clone)]
pub struct RedisSaver {
    conn: ConnectionManager,
    key_prefix: String,
    enable_indexes: bool,
}

// Redis 键格式辅助方法
impl RedisSaver {
    fn checkpoint_key(&self, id: &str) -> String {
        format!("{}:{}", self.key_prefix, id)
    }

    fn timeline_key(&self, thread_id: &str) -> String {
        format!("{}:{}:timeline", self.key_prefix, thread_id)
    }

    fn steps_key(&self, thread_id: &str) -> String {
        format!("{}:{}:steps", self.key_prefix, thread_id)
    }

    fn thread_index_key(&self, thread_id: &str) -> String {
        format!("{}:thread:{}", self.key_prefix, thread_id)
    }

    fn parent_index_key(&self, id: &str) -> String {
        format!("{}:index:{}:parent", self.key_prefix, id)
    }

    /// 全局线程集合键
    fn all_threads_key(&self) -> String {
        format!("{}:all_threads", self.key_prefix)
    }

    fn checkpoint_type_to_string(cp_type: &CheckpointType) -> &'static str {
        match cp_type {
            CheckpointType::Auto => "Auto",
            CheckpointType::Manual => "Manual",
            CheckpointType::InterruptBefore => "InterruptBefore",
            CheckpointType::InterruptAfter => "InterruptAfter",
            CheckpointType::Final => "Final",
        }
    }

    fn parse_checkpoint_type(s: &str) -> Result<CheckpointType, CheckpointError> {
        match s {
            "Auto" => Ok(CheckpointType::Auto),
            "Manual" => Ok(CheckpointType::Manual),
            "InterruptBefore" => Ok(CheckpointType::InterruptBefore),
            "InterruptAfter" => Ok(CheckpointType::InterruptAfter),
            "Final" => Ok(CheckpointType::Final),
            _ => Err(CheckpointError::Storage(format!(
                "Unknown checkpoint type: {}",
                s
            ))),
        }
    }
}

impl RedisSaver {
    /// 创建新的 Redis 检查点保存器
    pub async fn new(config: RedisSaverConfig) -> Result<Self, CheckpointError> {
        let client = redis::Client::open(config.redis_url.clone())
            .map_err(|e| CheckpointError::Storage(format!("Invalid Redis URL: {}", e)))?;

        let conn = ConnectionManager::new(client)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Failed to connect: {}", e)))?;

        Ok(Self {
            conn,
            key_prefix: config.key_prefix,
            enable_indexes: config.enable_indexes,
        })
    }

    /// 从连接管理器创建
    pub fn from_manager(conn: ConnectionManager, key_prefix: String) -> Self {
        Self {
            conn,
            key_prefix,
            enable_indexes: true,
        }
    }

    /// 返回连接管理器的引用
    pub fn conn(&self) -> &ConnectionManager {
        &self.conn
    }

    /// 计算检查点数据大小
    fn calculate_size<S: Serialize>(checkpoint: &Checkpoint<S>) -> Result<usize, CheckpointError> {
        let state_size = serde_json::to_vec(&checkpoint.state)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?
            .len();
        let next_nodes_size = serde_json::to_vec(&checkpoint.next_nodes)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?
            .len();
        Ok(state_size + next_nodes_size)
    }

    /// 从 Redis Hash 获取检查点
    async fn get_checkpoint_from_hash<S: DeserializeOwned>(
        &self,
        checkpoint_key: &str,
    ) -> Result<Option<Checkpoint<S>>, CheckpointError> {
        let mut conn = self.conn.clone();
        let data: HashMap<String, String> = conn
            .hgetall(checkpoint_key)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Failed to get checkpoint: {}", e)))?;

        if data.is_empty() {
            return Ok(None);
        }

        let checkpoint_type_str = data
            .get("checkpoint_type")
            .ok_or_else(|| CheckpointError::Storage("Missing checkpoint_type".to_owned()))?;
        let checkpoint_type = Self::parse_checkpoint_type(checkpoint_type_str)?;

        let tags_json = data.get("tags").cloned().unwrap_or_else(|| "{}".to_owned());
        let tags: HashMap<String, String> = serde_json::from_str(&tags_json)
            .map_err(|e| CheckpointError::Serialization(format!("Failed to parse tags: {}", e)))?;

        let metadata = CheckpointMetadata {
            id: data
                .get("id")
                .ok_or_else(|| CheckpointError::Storage("Missing id".to_owned()))?
                .clone(),
            parent_id: data.get("parent_id").cloned().filter(|s| !s.is_empty()),
            thread_id: data
                .get("thread_id")
                .ok_or_else(|| CheckpointError::Storage("Missing thread_id".to_owned()))?
                .clone(),
            created_at: data
                .get("created_at")
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| CheckpointError::Storage("Invalid created_at".to_owned()))?,
            step: data
                .get("step")
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| CheckpointError::Storage("Invalid step".to_owned()))?,
            tags,
            checkpoint_type,
        };

        let state_json = data
            .get("state_json")
            .ok_or_else(|| CheckpointError::Storage("Missing state_json".to_owned()))?;
        let state: S = serde_json::from_str(state_json).map_err(|e| {
            CheckpointError::Serialization(format!("Failed to deserialize state: {}", e))
        })?;

        let next_nodes_json = data
            .get("next_nodes")
            .ok_or_else(|| CheckpointError::Storage("Missing next_nodes".to_owned()))?;
        let next_nodes: serde_json::Value = serde_json::from_str(next_nodes_json).map_err(|e| {
            CheckpointError::Serialization(format!("Failed to deserialize next_nodes: {}", e))
        })?;
        let next_nodes = serde_json::from_value(next_nodes).map_err(|e| {
            CheckpointError::Serialization(format!("Failed to deserialize next_nodes: {}", e))
        })?;

        let pending_interrupt = if let Some(interrupt_json) = data.get("pending_interrupt")
            && !interrupt_json.is_empty()
            && !interrupt_json.eq("null")
        {
            Some(serde_json::from_str(interrupt_json).map_err(|e| {
                CheckpointError::Serialization(format!(
                    "Failed to deserialize pending_interrupt: {}",
                    e
                ))
            })?)
        } else {
            None
        };

        Ok(Some(Checkpoint {
            metadata,
            state,
            next_nodes,
            pending_interrupt,
        }))
    }

    /// 从 Redis Hash 获取元数据
    async fn get_metadata_from_hash(
        &self,
        checkpoint_key: &str,
    ) -> Result<Option<CheckpointMetadata>, CheckpointError> {
        let mut conn = self.conn.clone();
        let data: HashMap<String, String> = conn
            .hgetall(checkpoint_key)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Failed to get checkpoint: {}", e)))?;

        if data.is_empty() {
            return Ok(None);
        }

        let checkpoint_type_str = data
            .get("checkpoint_type")
            .ok_or_else(|| CheckpointError::Storage("Missing checkpoint_type".to_owned()))?;
        let checkpoint_type = Self::parse_checkpoint_type(checkpoint_type_str)?;

        let tags_json = data.get("tags").cloned().unwrap_or_else(|| "{}".to_owned());
        let tags: HashMap<String, String> = serde_json::from_str(&tags_json)
            .map_err(|e| CheckpointError::Serialization(format!("Failed to parse tags: {}", e)))?;

        Ok(Some(CheckpointMetadata {
            id: data
                .get("id")
                .ok_or_else(|| CheckpointError::Storage("Missing id".to_owned()))?
                .clone(),
            parent_id: data.get("parent_id").cloned().filter(|s| !s.is_empty()),
            thread_id: data
                .get("thread_id")
                .ok_or_else(|| CheckpointError::Storage("Missing thread_id".to_owned()))?
                .clone(),
            created_at: data
                .get("created_at")
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| CheckpointError::Storage("Invalid created_at".to_owned()))?,
            step: data
                .get("step")
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| CheckpointError::Storage("Invalid step".to_owned()))?,
            tags,
            checkpoint_type,
        }))
    }

    /// 添加到索引
    async fn add_to_indexes<S>(
        &self,
        checkpoint: &Checkpoint<S>,
        _checkpoint_key: &str,
    ) -> Result<(), CheckpointError> {
        if !self.enable_indexes {
            return Ok(());
        }

        let mut conn = self.conn.clone();
        let checkpoint_id = &checkpoint.metadata.id;
        let thread_id = &checkpoint.metadata.thread_id;
        let created_at = checkpoint.metadata.created_at;
        let step = checkpoint.metadata.step;

        // 添加到时间线索引
        let timeline_key = self.timeline_key(thread_id);
        let _: () = conn
            .zadd(timeline_key, checkpoint_id, created_at)
            .await
            .map_err(|e| {
                CheckpointError::Storage(format!("Failed to add to timeline index: {}", e))
            })?;

        // 添加到步数索引
        let steps_key = self.steps_key(thread_id);
        let _: () = conn
            .zadd(steps_key, checkpoint_id, step as i64)
            .await
            .map_err(|e| {
                CheckpointError::Storage(format!("Failed to add to steps index: {}", e))
            })?;

        // 添加到线程索引
        let thread_index_key = self.thread_index_key(thread_id);
        let _: () = conn
            .sadd(thread_index_key, checkpoint_id)
            .await
            .map_err(|e| {
                CheckpointError::Storage(format!("Failed to add to thread index: {}", e))
            })?;

        // 添加到全局线程集合
        let all_threads_key = self.all_threads_key();
        let _: () = conn.sadd(all_threads_key, thread_id).await.map_err(|e| {
            CheckpointError::Storage(format!("Failed to add to all threads set: {}", e))
        })?;

        // 添加到父子关系索引
        if let Some(ref parent_id) = checkpoint.metadata.parent_id {
            let parent_index_key = self.parent_index_key(checkpoint_id);
            let _: () = conn
                .hset(parent_index_key, "parent_id", parent_id)
                .await
                .map_err(|e| {
                    CheckpointError::Storage(format!("Failed to add to parent index: {}", e))
                })?;
        }

        Ok(())
    }

    /// 从索引中移除
    async fn remove_from_indexes(
        &self,
        checkpoint: &CheckpointMetadata,
    ) -> Result<(), CheckpointError> {
        if !self.enable_indexes {
            return Ok(());
        }

        let mut conn = self.conn.clone();
        let checkpoint_id = &checkpoint.id;
        let thread_id = &checkpoint.thread_id;

        // 从时间线索引移除
        let timeline_key = self.timeline_key(thread_id);
        let _: () = conn.zrem(timeline_key, checkpoint_id).await.map_err(|e| {
            CheckpointError::Storage(format!("Failed to remove from timeline index: {}", e))
        })?;

        // 从步数索引移除
        let steps_key = self.steps_key(thread_id);
        let _: () = conn.zrem(steps_key, checkpoint_id).await.map_err(|e| {
            CheckpointError::Storage(format!("Failed to remove from steps index: {}", e))
        })?;

        // 从线程索引移除
        let thread_index_key = self.thread_index_key(thread_id);
        let _: () = conn
            .srem(&thread_index_key, checkpoint_id)
            .await
            .map_err(|e| {
                CheckpointError::Storage(format!("Failed to remove from thread index: {}", e))
            })?;

        // 检查线程是否还有检查点，如果没有则从全局线程集合中移除
        let remaining: i64 = conn
            .scard(&thread_index_key)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Failed to check thread size: {}", e)))?;
        if remaining == 0 {
            let all_threads_key = self.all_threads_key();
            let _: () = conn.srem(all_threads_key, thread_id).await.map_err(|e| {
                CheckpointError::Storage(format!("Failed to remove from all threads set: {}", e))
            })?;
        }

        // 移除父子关系索引
        let parent_index_key = self.parent_index_key(checkpoint_id);
        let _: () = conn.del(parent_index_key).await.map_err(|e| {
            CheckpointError::Storage(format!("Failed to remove parent index: {}", e))
        })?;

        Ok(())
    }
}

#[async_trait]
impl<S> Checkpointer<S> for RedisSaver
where
    S: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    async fn get(&self, thread_id: &str) -> Result<Option<Checkpoint<S>>, CheckpointError> {
        let mut conn = self.conn.clone();
        let timeline_key = self.timeline_key(thread_id);

        // 从时间线索引获取最新的检查点 ID
        let checkpoints: Vec<String> = conn.zrevrange(timeline_key, 0, 0).await.map_err(|e| {
            CheckpointError::Storage(format!("Failed to get latest checkpoint: {}", e))
        })?;

        let latest_id: Option<String> = checkpoints.into_iter().next();

        match latest_id {
            Some(id) => {
                let checkpoint_key = self.checkpoint_key(&id);
                self.get_checkpoint_from_hash(&checkpoint_key).await
            }
            None => Ok(None),
        }
    }

    async fn put(&self, checkpoint: &Checkpoint<S>) -> Result<(), CheckpointError> {
        let mut conn = self.conn.clone();
        let checkpoint_key = self.checkpoint_key(&checkpoint.metadata.id);
        let now = Utc::now().timestamp();
        let size_bytes = Self::calculate_size(checkpoint)?;

        // 序列化数据
        let state_json = serde_json::to_string(&checkpoint.state)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        let next_nodes_json = serde_json::to_string(&checkpoint.next_nodes)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        let tags_json = serde_json::to_string(&checkpoint.metadata.tags)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        let pending_interrupt_json = serde_json::to_string(&checkpoint.pending_interrupt)
            .unwrap_or_else(|_| "null".to_owned());
        let checkpoint_type_str =
            Self::checkpoint_type_to_string(&checkpoint.metadata.checkpoint_type);

        // 构建 Hash 数据
        let fields: Vec<(&str, String)> = vec![
            ("id", checkpoint.metadata.id.clone()),
            (
                "parent_id",
                checkpoint.metadata.parent_id.clone().unwrap_or_default(),
            ),
            ("thread_id", checkpoint.metadata.thread_id.clone()),
            ("created_at", checkpoint.metadata.created_at.to_string()),
            ("step", checkpoint.metadata.step.to_string()),
            ("checkpoint_type", checkpoint_type_str.to_owned()),
            ("tags", tags_json),
            ("state_json", state_json),
            ("next_nodes", next_nodes_json),
            ("pending_interrupt", pending_interrupt_json),
            ("size_bytes", size_bytes.to_string()),
            ("updated_at", now.to_string()),
        ];

        // 存储到 Hash
        let _: () = conn
            .hset_multiple(&checkpoint_key, &fields)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Failed to save checkpoint: {}", e)))?;

        // 添加到索引
        self.add_to_indexes(checkpoint, &checkpoint_key).await?;

        Ok(())
    }

    async fn delete(&self, thread_id: &str) -> Result<(), CheckpointError> {
        let mut conn = self.conn.clone();
        let thread_index_key = self.thread_index_key(thread_id);

        // 获取该线程的所有检查点 ID
        let checkpoint_ids: Vec<String> = conn.smembers(&thread_index_key).await.map_err(|e| {
            CheckpointError::Storage(format!("Failed to get thread checkpoints: {}", e))
        })?;

        // 删除所有检查点
        for checkpoint_id in checkpoint_ids {
            let checkpoint_key = self.checkpoint_key(&checkpoint_id);
            let _: () = conn.del(&checkpoint_key).await.map_err(|e| {
                CheckpointError::Storage(format!("Failed to delete checkpoint: {}", e))
            })?;
        }

        // 删除索引
        let timeline_key = self.timeline_key(thread_id);
        let steps_key = self.steps_key(thread_id);
        let _: () = conn.del(timeline_key).await.map_err(|e| {
            CheckpointError::Storage(format!("Failed to delete timeline index: {}", e))
        })?;
        let _: () = conn.del(steps_key).await.map_err(|e| {
            CheckpointError::Storage(format!("Failed to delete steps index: {}", e))
        })?;
        let _: () = conn.del(&thread_index_key).await.map_err(|e| {
            CheckpointError::Storage(format!("Failed to delete thread index: {}", e))
        })?;

        Ok(())
    }

    async fn delete_checkpoint(&self, checkpoint_id: &CheckpointId) -> Result<(), CheckpointError> {
        // 首先获取检查点元数据（用于清理索引）
        let checkpoint_key = self.checkpoint_key(checkpoint_id);
        let metadata = self.get_metadata_from_hash(&checkpoint_key).await?;

        match metadata {
            Some(meta) => {
                // 删除检查点数据
                let mut conn = self.conn.clone();
                let _: () = conn.del(&checkpoint_key).await.map_err(|e| {
                    CheckpointError::Storage(format!("Failed to delete checkpoint: {}", e))
                })?;

                // 从索引中移除
                self.remove_from_indexes(&meta).await?;

                Ok(())
            }
            None => Err(CheckpointError::NotFound(checkpoint_id.clone())),
        }
    }

    async fn list(
        &self,
        thread_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<CheckpointMetadata>, CheckpointError> {
        let mut conn = self.conn.clone();
        let timeline_key = self.timeline_key(thread_id);

        // 从时间线索引获取检查点 ID（按时间倒序）
        let limit_val = limit.unwrap_or(usize::MAX) as isize;
        let checkpoint_ids: Vec<String> = conn
            .zrevrange(timeline_key, 0, limit_val - 1)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Failed to list checkpoints: {}", e)))?;

        let mut results = Vec::new();
        for checkpoint_id in checkpoint_ids {
            let checkpoint_key = self.checkpoint_key(&checkpoint_id);
            if let Some(metadata) = self.get_metadata_from_hash(&checkpoint_key).await? {
                results.push(metadata);
            }
        }

        Ok(results)
    }

    async fn search(
        &self,
        query: CheckpointQuery,
    ) -> Result<CheckpointListResult, CheckpointError> {
        let mut results: Vec<CheckpointMetadata> = Vec::new();

        // 确定要搜索的线程范围
        let thread_ids = if let Some(ref thread_id) = query.thread_id {
            vec![thread_id.clone()]
        } else {
            let mut conn = self.conn.clone();
            conn.smembers(self.all_threads_key()).await.map_err(|e| {
                CheckpointError::Storage(format!("Failed to get all threads: {}", e))
            })?
        };

        for thread_id in thread_ids {
            let timeline_key = self.timeline_key(&thread_id);

            // 根据时间范围获取检查点 ID
            let (min_score, max_score) = match (query.start_time, query.end_time) {
                (Some(start), Some(end)) => (start, end),
                (Some(start), None) => (start, i64::MAX),
                (None, Some(end)) => (i64::MIN, end),
                (None, None) => (i64::MIN, i64::MAX),
            };

            let mut conn = self.conn.clone();
            let checkpoint_ids: Vec<String> = conn
                .zrangebyscore(&timeline_key, min_score, max_score)
                .await
                .map_err(|e| {
                    CheckpointError::Storage(format!("Failed to search checkpoints: {}", e))
                })?;

            // 获取检查点元数据
            for checkpoint_id in checkpoint_ids {
                let checkpoint_key = self.checkpoint_key(&checkpoint_id);
                if let Some(metadata) = self.get_metadata_from_hash(&checkpoint_key).await? {
                    // 过滤条件
                    let mut matches = true;

                    // checkpoint_type 过滤
                    if let Some(ref cp_type) = query.checkpoint_type
                        && metadata.checkpoint_type != *cp_type
                    {
                        matches = false;
                    }

                    // 标签过滤
                    if let Some(ref tags) = query.tags {
                        for (key, value) in tags.iter() {
                            if metadata.tags.get(key) != Some(value) {
                                matches = false;
                                break;
                            }
                        }
                    }

                    if matches {
                        results.push(metadata);
                    }
                }
            }
        }

        // 排序
        match query.order {
            CheckpointOrder::Desc => {
                results.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            }
            CheckpointOrder::Asc => {
                results.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            }
        }

        let total_count = results.len();

        // 限制数量
        if let Some(limit) = query.limit {
            results.truncate(limit);
        }

        Ok(CheckpointListResult {
            checkpoints: results,
            total_count,
        })
    }

    async fn get_by_id(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Option<Checkpoint<S>>, CheckpointError> {
        let checkpoint_key = self.checkpoint_key(checkpoint_id);
        self.get_checkpoint_from_hash(&checkpoint_key).await
    }

    async fn get_metadata(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Option<CheckpointMetadata>, CheckpointError> {
        let checkpoint_key = self.checkpoint_key(checkpoint_id);
        self.get_metadata_from_hash(&checkpoint_key).await
    }

    async fn get_metadata_parent_id(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Option<String>, CheckpointError> {
        let mut conn = self.conn.clone();
        let checkpoint_key = self.checkpoint_key(checkpoint_id);

        let parent_id: Option<String> = conn
            .hget(&checkpoint_key, "parent_id")
            .await
            .map_err(|e| CheckpointError::Storage(format!("Failed to get parent_id: {}", e)))?;

        // 过滤空字符串
        Ok(parent_id.filter(|s| !s.is_empty()))
    }

    async fn get_history(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Vec<CheckpointMetadata>, CheckpointError> {
        let mut history = Vec::new();
        let mut current_id: Option<String> = Some(checkpoint_id.clone());

        while let Some(id) = current_id.take() {
            let checkpoint_key = self.checkpoint_key(&id);
            match self.get_metadata_from_hash(&checkpoint_key).await? {
                Some(metadata) => {
                    current_id = metadata.parent_id.clone();
                    history.push(metadata);
                }
                None => break,
            }
        }

        history.reverse();
        Ok(history)
    }

    async fn get_at_time(
        &self,
        thread_id: &str,
        time: i64,
    ) -> Result<Option<Checkpoint<S>>, CheckpointError> {
        let mut conn = self.conn.clone();
        let timeline_key = self.timeline_key(thread_id);

        // 使用 ZREVRANGEBYSCORE 找到最接近的检查点
        let raw_results: Vec<String> = conn
            .zrevrangebyscore(&timeline_key, time, "-inf")
            .await
            .map_err(|e| {
                CheckpointError::Storage(format!("Failed to get checkpoint at time: {}", e))
            })?;

        let checkpoint_ids: Vec<String> = raw_results.into_iter().take(1).collect();

        match checkpoint_ids.first() {
            Some(id) => {
                let checkpoint_key = self.checkpoint_key(id);
                self.get_checkpoint_from_hash(&checkpoint_key).await
            }
            None => Ok(None),
        }
    }

    async fn cleanup(&self, policy: &CleanupPolicy) -> Result<usize, CheckpointError> {
        match policy {
            CleanupPolicy::KeepLast(n) => {
                let mut conn = self.conn.clone();
                let all_threads_key = self.all_threads_key();

                // 获取所有线程 ID
                let thread_ids: Vec<String> =
                    conn.smembers(&all_threads_key).await.map_err(|e| {
                        CheckpointError::Storage(format!("Failed to get threads: {}", e))
                    })?;

                let mut total_deleted = 0;

                for thread_id in thread_ids {
                    let timeline_key = self.timeline_key(&thread_id);

                    // 获取该线程所有检查点（按时间倒序）
                    let all_ids: Vec<String> =
                        conn.zrevrange(&timeline_key, 0, -1).await.map_err(|e| {
                            CheckpointError::Storage(format!("Failed to get checkpoints: {}", e))
                        })?;

                    // 跳过前 n 个，删除其余的
                    if all_ids.len() > *n {
                        let ids_to_delete = &all_ids[*n..];
                        for checkpoint_id in ids_to_delete {
                            // 获取元数据以便清理索引
                            let checkpoint_key = self.checkpoint_key(checkpoint_id);
                            if let Some(metadata) =
                                self.get_metadata_from_hash(&checkpoint_key).await?
                            {
                                // 删除检查点数据
                                let _: () = conn.del(&checkpoint_key).await.map_err(|e| {
                                    CheckpointError::Storage(format!(
                                        "Failed to delete checkpoint: {}",
                                        e
                                    ))
                                })?;

                                // 从索引中移除
                                self.remove_from_indexes(&metadata).await?;
                                total_deleted += 1;
                            }
                        }
                    }
                }

                Ok(total_deleted)
            }
            CleanupPolicy::KeepDays(days) => {
                let cutoff = Utc::now().timestamp_millis() - (days * 86400 * 1000);
                let mut conn = self.conn.clone();
                let all_threads_key = self.all_threads_key();

                // 获取所有线程 ID
                let thread_ids: Vec<String> =
                    conn.smembers(&all_threads_key).await.map_err(|e| {
                        CheckpointError::Storage(format!("Failed to get threads: {}", e))
                    })?;

                let mut total_deleted = 0;

                for thread_id in thread_ids {
                    let timeline_key = self.timeline_key(&thread_id);

                    // 获取时间早于 cutoff 的检查点
                    let old_ids: Vec<String> = conn
                        .zrangebyscore_limit(&timeline_key, i64::MIN, cutoff, 0, -1)
                        .await
                        .map_err(|e| {
                            CheckpointError::Storage(format!(
                                "Failed to get old checkpoints: {}",
                                e
                            ))
                        })?;

                    for checkpoint_id in old_ids {
                        let checkpoint_key = self.checkpoint_key(&checkpoint_id);
                        if let Some(metadata) = self.get_metadata_from_hash(&checkpoint_key).await?
                        {
                            // 删除检查点数据
                            let _: () = conn.del(&checkpoint_key).await.map_err(|e| {
                                CheckpointError::Storage(format!(
                                    "Failed to delete checkpoint: {}",
                                    e
                                ))
                            })?;

                            // 从索引中移除
                            self.remove_from_indexes(&metadata).await?;
                            total_deleted += 1;
                        }
                    }
                }

                Ok(total_deleted)
            }
            CleanupPolicy::KeepMaxSizeBytes(max_size) => {
                let mut conn = self.conn.clone();
                let all_threads_key = self.all_threads_key();

                // 获取所有线程 ID
                let thread_ids: Vec<String> =
                    conn.smembers(&all_threads_key).await.map_err(|e| {
                        CheckpointError::Storage(format!("Failed to get threads: {}", e))
                    })?;

                // 收集所有检查点及其大小和时间
                let mut all_checkpoints: Vec<(String, usize, i64, String)> = Vec::new(); // (id, size, created_at, thread_id)

                for thread_id in &thread_ids {
                    let thread_index_key = self.thread_index_key(thread_id);
                    let checkpoint_ids: Vec<String> =
                        conn.smembers(&thread_index_key).await.map_err(|e| {
                            CheckpointError::Storage(format!("Failed to get checkpoints: {}", e))
                        })?;

                    for checkpoint_id in checkpoint_ids {
                        let checkpoint_key = self.checkpoint_key(&checkpoint_id);

                        // 获取大小和创建时间
                        let size_str: Option<String> = conn
                            .hget(&checkpoint_key, "size_bytes")
                            .await
                            .map_err(|e| {
                                CheckpointError::Storage(format!("Failed to get size: {}", e))
                            })?;
                        let created_str: Option<String> = conn
                            .hget(&checkpoint_key, "created_at")
                            .await
                            .map_err(|e| {
                                CheckpointError::Storage(format!("Failed to get created_at: {}", e))
                            })?;

                        if let (Some(size_str), Some(created_str)) = (size_str, created_str)
                            && let (Ok(size), Ok(created)) =
                                (size_str.parse::<usize>(), created_str.parse::<i64>())
                        {
                            all_checkpoints.push((checkpoint_id, size, created, thread_id.clone()));
                        }
                    }
                }

                // 计算总大小
                let total_size: usize = all_checkpoints.iter().map(|(_, s, _, _)| s).sum();

                if total_size <= *max_size {
                    return Ok(0);
                }

                // 按创建时间升序排序（最旧的在前）
                all_checkpoints.sort_by_key(|(_, _, created, _)| *created);

                // 删除最旧的检查点直到满足大小限制
                let mut current_size = total_size;
                let mut deleted = 0;

                for (checkpoint_id, size, _, _thread_id) in all_checkpoints {
                    if current_size <= *max_size {
                        break;
                    }

                    let checkpoint_key = self.checkpoint_key(&checkpoint_id);
                    if let Some(metadata) = self.get_metadata_from_hash(&checkpoint_key).await? {
                        // 删除检查点数据
                        let _: () = conn.del(&checkpoint_key).await.map_err(|e| {
                            CheckpointError::Storage(format!("Failed to delete checkpoint: {}", e))
                        })?;

                        // 从索引中移除
                        self.remove_from_indexes(&metadata).await?;
                        current_size -= size;
                        deleted += 1;
                    }
                }

                Ok(deleted)
            }
        }
    }

    async fn stats(&self, thread_id: Option<&str>) -> Result<CheckpointStats, CheckpointError> {
        let mut total_count = 0;
        let mut total_size = 0;
        let mut oldest = None;
        let mut newest = None;

        if let Some(tid) = thread_id {
            let mut conn = self.conn.clone();
            let thread_index_key = self.thread_index_key(tid);

            // 获取该线程的所有检查点 ID
            let checkpoint_ids: Vec<String> =
                conn.smembers(&thread_index_key).await.map_err(|e| {
                    CheckpointError::Storage(format!("Failed to get thread checkpoints: {}", e))
                })?;

            for checkpoint_id in checkpoint_ids {
                let checkpoint_key = self.checkpoint_key(&checkpoint_id);
                let mut conn = self.conn.clone();

                // 获取大小
                if let Ok(Some(size_str)) = conn
                    .hget::<_, _, Option<String>>(&checkpoint_key, "size_bytes")
                    .await
                    && let Ok(size) = size_str.parse::<usize>()
                {
                    total_count += 1;
                    total_size += size;
                }

                // 获取创建时间
                if let Ok(Some(created_str)) = conn
                    .hget::<_, _, Option<String>>(&checkpoint_key, "created_at")
                    .await
                    && let Ok(created) = created_str.parse::<i64>()
                {
                    if oldest.is_none() || Some(created) < oldest {
                        oldest = Some(created);
                    }
                    if newest.is_none() || Some(created) > newest {
                        newest = Some(created);
                    }
                }
            }
        }

        Ok(CheckpointStats {
            total_count,
            total_size_bytes: total_size,
            oldest_checkpoint: oldest,
            newest_checkpoint: newest,
        })
    }
}
