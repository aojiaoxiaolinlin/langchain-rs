use async_trait::async_trait;
use chrono::Utc;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Row, SqlitePool, sqlite::SqliteConnectOptions};
use std::path::Path;
use std::str::FromStr;

use crate::checkpoint::checkpoint_struct_api::{Checkpoint, CheckpointMetadata};
use crate::checkpoint::checkpoint_trait::{
    CheckpointError, CheckpointStats, Checkpointer, CleanupPolicy,
};
use crate::checkpoint::{
    CheckpointId, CheckpointListResult, CheckpointOrder, CheckpointQuery, CheckpointType,
};

/// SQLite 检查点保存器配置
#[derive(Debug, Clone)]
pub struct SqliteSaverConfig {
    /// 数据库文件路径（默认 "checkoutpoint.db"）
    pub database_path: String,
    /// 连接池大小
    pub pool_size: u32,
    /// 是否自动创建表
    pub auto_create_table: bool,
}

impl Default for SqliteSaverConfig {
    fn default() -> Self {
        Self {
            database_path: "sqlite://data/checkoutpoint.db".to_owned(),
            pool_size: 3,
            auto_create_table: true,
        }
    }
}

impl SqliteSaverConfig {
    /// 创建文件数据库配置
    pub fn file<P: AsRef<Path>>(path: P) -> Self {
        Self {
            database_path: path.as_ref().to_string_lossy().to_string(),
            pool_size: 3,
            auto_create_table: true,
        }
    }
}

/// SQLite 检查点保存器
#[derive(Debug, Clone)]
pub struct SqliteSaver {
    pool: SqlitePool,
}

impl SqliteSaver {
    /// 创建新的 SQLite 检查点保存器
    pub async fn new(config: SqliteSaverConfig) -> Result<Self, CheckpointError> {
        // 解析连接选项
        let options = SqliteConnectOptions::from_str(&config.database_path)
            .map_err(|e| CheckpointError::Storage(format!("Invalid database path: {}", e)))?
            .create_if_missing(true);
        // 创建连接池
        let pool = SqlitePool::connect_with(options)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Failed to connect: {}", e)))?;
        let saver = Self { pool };
        // 自动创建表
        if config.auto_create_table {
            saver.init_schema().await?;
        }
        Ok(saver)
    }

    /// 从连接池创建
    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 返回连接池的引用
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// 初始化数据库表结构
    async fn init_schema(&self) -> Result<(), CheckpointError> {
        let query = r#"
            CREATE TABLE IF NOT EXISTS checkpoints (
                id TEXT PRIMARY KEY NOT NULL,
                parent_id TEXT,
                thread_id TEXT NOT NULL,
                created_at BIGINT NOT NULL,
                step INTEGER NOT NULL,
                checkpoint_type TEXT NOT NULL,
                tags TEXT,
                state_json TEXT NOT NULL,
                next_nodes TEXT NOT NULL,
                pending_interrupt TEXT,
                size_bytes INTEGER NOT NULL DEFAULT 0,
                updated_at BIGINT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_id
                ON checkpoints(thread_id);

            CREATE INDEX IF NOT EXISTS idx_checkpoints_parent_id
                ON checkpoints(parent_id);

            CREATE INDEX IF NOT EXISTS idx_checkpoints_created_at
                ON checkpoints(created_at);

            CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_created
                ON checkpoints(thread_id, created_at DESC);

            CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_step
                ON checkpoints(thread_id, step);
        "#;

        sqlx::query(query)
            .execute(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Failed to create table: {}", e)))?;

        tracing::info!("SQLite checkpoint schema initialized");
        Ok(())
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

    /// 将 CheckpointMetadata 转换为 CheckpointType
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

    /// 将 CheckpointType 转换为字符串
    fn checkpoint_type_to_string(cp_type: &CheckpointType) -> &'static str {
        match cp_type {
            CheckpointType::Auto => "Auto",
            CheckpointType::Manual => "Manual",
            CheckpointType::InterruptBefore => "InterruptBefore",
            CheckpointType::InterruptAfter => "InterruptAfter",
            CheckpointType::Final => "Final",
        }
    }

    /// 解析标签 JSON
    fn parse_tags(
        tags_json: Option<&str>,
    ) -> Result<std::collections::HashMap<String, String>, CheckpointError> {
        match tags_json {
            Some(json) if !json.is_empty() => serde_json::from_str(json).map_err(|e| {
                CheckpointError::Serialization(format!("Failed to parse tags: {}", e))
            }),
            _ => Ok(std::collections::HashMap::new()),
        }
    }

    /// 从数据库行构建 Checkpoint
    fn row_to_checkpoint<S: DeserializeOwned>(
        row: &sqlx::sqlite::SqliteRow,
    ) -> Result<Checkpoint<S>, CheckpointError> {
        let checkpoint_type_str: String = row
            .try_get("checkpoint_type")
            .map_err(|e| CheckpointError::Storage(format!("Missing checkpoint_type: {}", e)))?;
        let checkpoint_type = Self::parse_checkpoint_type(&checkpoint_type_str)?;

        let tags_json: Option<String> = row.try_get("tags").ok();
        let tags = Self::parse_tags(tags_json.as_deref())?;

        let step: u64 = row
            .try_get("step")
            .map_err(|e| CheckpointError::Storage(format!("Missing step: {}", e)))?;
        let metadata = CheckpointMetadata {
            id: row
                .try_get("id")
                .map_err(|e| CheckpointError::Storage(format!("Missing id: {}", e)))?,
            parent_id: row.try_get("parent_id").ok(),
            thread_id: row
                .try_get("thread_id")
                .map_err(|e| CheckpointError::Storage(format!("Missing thread_id: {}", e)))?,
            created_at: row
                .try_get("created_at")
                .map_err(|e| CheckpointError::Storage(format!("Missing created_at: {}", e)))?,
            step: step as usize,
            tags,
            checkpoint_type,
        };
        let state_json: String = row
            .try_get("state_json")
            .map_err(|e| CheckpointError::Storage(format!("Missing state_json: {}", e)))?;
        let state = serde_json::from_str(&state_json).map_err(|e| {
            CheckpointError::Serialization(format!("Failed to deserialize state: {}", e))
        })?;
        let next_nodes_json: String = row
            .try_get("next_nodes")
            .map_err(|e| CheckpointError::Storage(format!("Missing next_nodes: {}", e)))?;
        let next_nodes = serde_json::from_str(&next_nodes_json).map_err(|e| {
            CheckpointError::Serialization(format!("Failed to deserialize next_nodes: {}", e))
        })?;
        let pending_interrupt_json: Option<String> = row.try_get("pending_interrupt").ok();
        let pending_interrupt = if let Some(json) = pending_interrupt_json {
            if json.eq("null") {
                None
            } else {
                Some(serde_json::from_str(&json).map_err(|e| {
                    CheckpointError::Serialization(format!(
                        "Failed to deserialize pending_interrupt: {}",
                        e
                    ))
                })?)
            }
        } else {
            None
        };
        Ok(Checkpoint {
            metadata,
            state,
            next_nodes,
            pending_interrupt,
        })
    }

    /// 从数据库行构建 CheckpointMetadata
    fn row_to_metadata(
        row: &sqlx::sqlite::SqliteRow,
    ) -> Result<CheckpointMetadata, CheckpointError> {
        let checkpoint_type_str: String = row
            .try_get("checkpoint_type")
            .map_err(|e| CheckpointError::Storage(format!("Missing checkpoint_type: {}", e)))?;
        let checkpoint_type = Self::parse_checkpoint_type(&checkpoint_type_str)?;

        let tags_json: Option<String> = row.try_get("tags").ok();
        let tags = Self::parse_tags(tags_json.as_deref())?;

        let step: u64 = row
            .try_get("step")
            .map_err(|e| CheckpointError::Storage(format!("Missing step: {}", e)))?;
        Ok(CheckpointMetadata {
            id: row
                .try_get("id")
                .map_err(|e| CheckpointError::Storage(format!("Missing id: {}", e)))?,
            parent_id: row.try_get("parent_id").ok(),
            thread_id: row
                .try_get("thread_id")
                .map_err(|e| CheckpointError::Storage(format!("Missing thread_id: {}", e)))?,
            created_at: row
                .try_get("created_at")
                .map_err(|e| CheckpointError::Storage(format!("Missing created_at: {}", e)))?,
            step: step as usize,
            tags,
            checkpoint_type,
        })
    }
}

#[async_trait]
impl<S> Checkpointer<S> for SqliteSaver
where
    S: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    async fn get(&self, thread_id: &str) -> Result<Option<Checkpoint<S>>, CheckpointError> {
        let query = r#"
            SELECT * FROM checkpoints
            WHERE thread_id = ?
            ORDER BY id DESC
            LIMIT 1
        "#;

        let row = sqlx::query(query)
            .bind(thread_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;

        match row {
            Some(r) => {
                let checkpoint = Self::row_to_checkpoint::<S>(&r);
                if let Err(e) = &checkpoint {
                    println!("e::{}", e);
                }
                Ok(Some(checkpoint.unwrap()))
            }
            None => Ok(None),
        }
    }

    async fn put(&self, checkpoint: &Checkpoint<S>) -> Result<(), CheckpointError> {
        let now = Utc::now().timestamp_millis();
        let size_bytes = Self::calculate_size(checkpoint)?;

        let state_json = serde_json::to_vec(&checkpoint.state)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        let next_nodes_json = serde_json::to_vec(&checkpoint.next_nodes)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        let tags_json = serde_json::to_string(&checkpoint.metadata.tags)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        let pending_interrupt_json = serde_json::to_string(&checkpoint.pending_interrupt)
            .unwrap_or_else(|_| "null".to_owned());
        let checkpoint_type_str =
            Self::checkpoint_type_to_string(&checkpoint.metadata.checkpoint_type);

        let query = r#"
            INSERT INTO checkpoints (
                id, parent_id, thread_id, created_at, step, checkpoint_type,
                tags, state_json, next_nodes, pending_interrupt, size_bytes, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                parent_id = excluded.parent_id,
                thread_id = excluded.thread_id,
                created_at = excluded.created_at,
                step = excluded.step,
                checkpoint_type = excluded.checkpoint_type,
                tags = excluded.tags,
                state_json = excluded.state_json,
                next_nodes = excluded.next_nodes,
                pending_interrupt = excluded.pending_interrupt,
                size_bytes = excluded.size_bytes,
                updated_at = excluded.updated_at
        "#;

        sqlx::query(query)
            .bind(&checkpoint.metadata.id)
            .bind(&checkpoint.metadata.parent_id)
            .bind(&checkpoint.metadata.thread_id)
            .bind(checkpoint.metadata.created_at)
            .bind(checkpoint.metadata.step as i64)
            .bind(checkpoint_type_str)
            .bind(tags_json)
            .bind(
                String::from_utf8(state_json)
                    .map_err(|e| CheckpointError::Serialization(e.to_string()))?,
            )
            .bind(
                String::from_utf8(next_nodes_json)
                    .map_err(|e| CheckpointError::Serialization(e.to_string()))?,
            )
            .bind(pending_interrupt_json)
            .bind(size_bytes as i64)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Insert failed: {}", e)))?;

        Ok(())
    }

    async fn delete(&self, thread_id: &str) -> Result<(), CheckpointError> {
        let query = "DELETE FROM checkpoints WHERE thread_id = ?";

        sqlx::query(query)
            .bind(thread_id)
            .execute(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Delete failed: {}", e)))?;

        Ok(())
    }

    async fn delete_checkpoint(&self, checkpoint_id: &CheckpointId) -> Result<(), CheckpointError> {
        let query = "DELETE FROM checkpoints WHERE id = ?";

        let result = sqlx::query(query)
            .bind(checkpoint_id)
            .execute(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Delete failed: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(CheckpointError::NotFound(checkpoint_id.clone()));
        }

        Ok(())
    }

    async fn list(
        &self,
        thread_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<CheckpointMetadata>, CheckpointError> {
        let query = r#"
            SELECT * FROM checkpoints
            WHERE thread_id = ?
            ORDER BY created_at DESC
            LIMIT ?
        "#;

        let rows = sqlx::query(query)
            .bind(thread_id)
            .bind(limit.unwrap_or(usize::MAX) as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("List failed: {}", e)))?;

        rows.iter().map(Self::row_to_metadata).collect()
    }

    async fn search(
        &self,
        query: CheckpointQuery,
    ) -> Result<CheckpointListResult, CheckpointError> {
        let mut sql = "SELECT * FROM checkpoints WHERE 1=1".to_owned();
        let mut params: Vec<String> = Vec::new();
        let param_index = &mut 0;

        // thread_id 过滤
        if let Some(ref thread_id) = query.thread_id {
            sql.push_str(&format!(" AND thread_id = ?{}", param_index));
            params.push(thread_id.clone());
            *param_index += 1;
        }

        // start_time 过滤
        if let Some(start) = query.start_time {
            sql.push_str(&format!(" AND created_at >= ?{}", param_index));
            params.push(start.to_string());
            *param_index += 1;
        }

        // end_time 过滤
        if let Some(end) = query.end_time {
            sql.push_str(&format!(" AND created_at <= ?{}", param_index));
            params.push(end.to_string());
            *param_index += 1;
        }

        // checkpoint_type 过滤
        if let Some(ref cp_type) = query.checkpoint_type {
            sql.push_str(&format!(" AND checkpoint_type = ?{}", param_index));
            params.push(Self::checkpoint_type_to_string(cp_type).to_owned());
            *param_index += 1;
        }

        // 排序
        match query.order {
            CheckpointOrder::Desc => sql.push_str(" ORDER BY created_at DESC"),
            CheckpointOrder::Asc => sql.push_str(" ORDER BY created_at ASC"),
        }

        // 限制数量
        let total_sql = format!("SELECT COUNT(*) as count FROM ({}) as subq", sql);
        let count_row = sqlx::query(&total_sql)
            // .bind_all(params.iter().map(|s| s.as_str()))
            .fetch_one(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Count query failed: {}", e)))?;
        let total_count: i64 = count_row
            .try_get("count")
            .map_err(|e| CheckpointError::Storage(format!("Failed to get count: {}", e)))?;

        // 获取结果
        if let Some(limit) = query.limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }

        let mut q = sqlx::query(&sql);
        for p in &params {
            q = q.bind(p.as_str());
        }

        let rows = q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Search failed: {}", e)))?;

        let checkpoints: Result<Vec<_>, _> = rows.iter().map(Self::row_to_metadata).collect();
        let checkpoints = checkpoints?;

        // 标签过滤（在内存中进行，因为 SQLite 不支持 JSON 索引查询）
        let checkpoints = if let Some(ref tags) = query.tags {
            checkpoints
                .into_iter()
                .filter(|m| tags.iter().all(|(k, v)| m.tags.get(k) == Some(v)))
                .collect()
        } else {
            checkpoints
        };

        Ok(CheckpointListResult {
            checkpoints,
            total_count: total_count as usize,
        })
    }

    async fn get_by_id(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Option<Checkpoint<S>>, CheckpointError> {
        let query = "SELECT * FROM checkpoints WHERE id = ?";

        let row = sqlx::query(query)
            .bind(checkpoint_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;

        match row {
            Some(r) => {
                let checkpoint = Self::row_to_checkpoint::<S>(&r)?;
                Ok(Some(checkpoint))
            }
            None => Ok(None),
        }
    }

    async fn get_metadata(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Option<CheckpointMetadata>, CheckpointError> {
        let query = "SELECT * FROM checkpoints WHERE id = ?";

        let row = sqlx::query(query)
            .bind(checkpoint_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;

        match row {
            Some(r) => Ok(Some(Self::row_to_metadata(&r)?)),
            None => Ok(None),
        }
    }

    async fn get_metadata_parent_id(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Option<String>, CheckpointError> {
        let query = "SELECT parent_id FROM checkpoints WHERE id = ?";

        let row = sqlx::query(query)
            .bind(checkpoint_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;

        match row {
            Some(r) => Ok(r.try_get("parent_id").ok()),
            None => Ok(None),
        }
    }

    async fn get_history(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Vec<CheckpointMetadata>, CheckpointError> {
        let mut history = Vec::new();
        let mut current_id: Option<String> = Some(checkpoint_id.clone());

        while let Some(id) = current_id.take() {
            let query = "SELECT * FROM checkpoints WHERE id = ?";
            let row = sqlx::query(query)
                .bind(&id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;

            match row {
                Some(r) => {
                    let metadata = Self::row_to_metadata(&r)?;
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
        let query = r#"
            SELECT * FROM checkpoints
            WHERE thread_id = ? AND created_at <= ?
            ORDER BY created_at DESC
            LIMIT 1
        "#;

        let row = sqlx::query(query)
            .bind(thread_id)
            .bind(time)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;

        match row {
            Some(r) => {
                let checkpoint = Self::row_to_checkpoint::<S>(&r)?;
                Ok(Some(checkpoint))
            }
            None => Ok(None),
        }
    }

    async fn cleanup(&self, policy: &CleanupPolicy) -> Result<usize, CheckpointError> {
        let mut to_delete: Vec<String> = Vec::new();

        match policy {
            CleanupPolicy::KeepLast(n) => {
                // 获取所有 thread_id
                let threads_query = "SELECT DISTINCT thread_id FROM checkpoints";
                let thread_rows = sqlx::query(threads_query)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;

                for row in thread_rows {
                    let thread_id: String = row.try_get("thread_id").map_err(|e| {
                        CheckpointError::Storage(format!("Missing thread_id: {}", e))
                    })?;

                    // 获取该线程的所有检查点，按时间排序
                    let query = r#"
                        SELECT id FROM checkpoints
                        WHERE thread_id = ?
                        ORDER BY created_at ASC
                    "#;
                    let checkpoint_rows = sqlx::query(query)
                        .bind(&thread_id)
                        .fetch_all(&self.pool)
                        .await
                        .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;

                    let checkpoint_count = checkpoint_rows.len();
                    if checkpoint_count > *n {
                        // 删除前面的 (count - n) 个
                        for row in checkpoint_rows.iter().take(checkpoint_count - n) {
                            let id: String = row.try_get("id").map_err(|e| {
                                CheckpointError::Storage(format!("Missing id: {}", e))
                            })?;
                            to_delete.push(id);
                        }
                    }
                }
            }
            CleanupPolicy::KeepDays(days) => {
                let cutoff = Utc::now().timestamp_millis() - (days * 86400 * 1000);

                let query = "SELECT id FROM checkpoints WHERE created_at < ?";
                let rows = sqlx::query(query)
                    .bind(cutoff)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;

                for row in rows {
                    let id: String = row
                        .try_get("id")
                        .map_err(|e| CheckpointError::Storage(format!("Missing id: {}", e)))?;
                    to_delete.push(id);
                }
            }
            CleanupPolicy::KeepMaxSizeBytes(max_size) => {
                // 计算总大小
                let size_query = "SELECT SUM(size_bytes) as total FROM checkpoints";
                let size_row = sqlx::query(size_query)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;
                let total_size: i64 = size_row.try_get("total").unwrap_or(0);

                if total_size as usize > *max_size {
                    // 按时间顺序删除，直到总大小小于限制
                    let query = r#"
                        SELECT id, size_bytes FROM checkpoints
                        ORDER BY created_at ASC
                    "#;
                    let rows = sqlx::query(query)
                        .fetch_all(&self.pool)
                        .await
                        .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;

                    let mut current_size = total_size as usize;
                    for row in rows {
                        if current_size <= *max_size {
                            break;
                        }
                        let id: String = row
                            .try_get("id")
                            .map_err(|e| CheckpointError::Storage(format!("Missing id: {}", e)))?;
                        let size: i64 = row.try_get("size_bytes").map_err(|e| {
                            CheckpointError::Storage(format!("Missing size_bytes: {}", e))
                        })?;
                        to_delete.push(id);
                        current_size = current_size.saturating_sub(size as usize);
                    }
                }
            }
        }

        // 批量删除
        let mut count = 0;
        if !to_delete.is_empty() {
            let placeholders = to_delete.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let query = format!(
                "DELETE FROM
          checkpoints WHERE id IN ({})",
                placeholders
            );

            let mut q = sqlx::query(&query);
            for id in &to_delete {
                q = q.bind(id);
            }
            q.execute(&self.pool).await.map_err(|e| {
                CheckpointError::Storage(format!(
                    "Delete
              failed: {}",
                    e
                ))
            })?;
            count = to_delete.len();
        }

        Ok(count)
    }

    async fn stats(&self, thread_id: Option<&str>) -> Result<CheckpointStats, CheckpointError> {
        let (count_query, _size_query, time_query) = if let Some(tid) = thread_id {
            (
                "SELECT COUNT(*) as count, SUM(size_bytes) as size FROM checkpoints WHERE thread_id = ?".to_owned(),
                format!("SELECT COUNT(*) as count, SUM(size_bytes) as size FROM checkpoints WHERE thread_id = '{}'", tid),
                format!("SELECT MIN(created_at) as oldest, MAX(created_at) as newest FROM checkpoints WHERE thread_id = '{}'", tid)
            )
        } else {
            (
                "SELECT COUNT(*) as count, SUM(size_bytes) as size FROM checkpoints".to_owned(),
                "SELECT COUNT(*) as count, SUM(size_bytes) as size FROM checkpoints".to_owned(),
                "SELECT MIN(created_at) as oldest, MAX(created_at) as newest FROM checkpoints"
                    .to_owned(),
            )
        };

        // 获取计数和大小
        let mut count_q = sqlx::query(&count_query);
        if let Some(tid) = thread_id {
            count_q = count_q.bind(tid);
        }
        let stats_row = count_q
            .fetch_one(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Stats query failed: {}", e)))?;
        let total_count: i64 = stats_row.try_get("count").unwrap_or(0);
        let total_size: i64 = stats_row.try_get("size").unwrap_or(0);

        // 获取时间范围
        let time_row = sqlx::query(&time_query)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Time query failed: {}", e)))?;
        let oldest: Option<i64> = time_row.try_get("oldest").ok();
        let newest: Option<i64> = time_row.try_get("newest").ok();

        Ok(CheckpointStats {
            total_count: total_count as usize,
            total_size_bytes: total_size as usize,
            oldest_checkpoint: oldest,
            newest_checkpoint: newest,
        })
    }
}
