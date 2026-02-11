use async_trait::async_trait;
use chrono::Utc;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{PgPool, Row, postgres::PgConnectOptions};
use std::str::FromStr;

use crate::checkpoint::{
    CheckpointId, CheckpointListResult, CheckpointOrder, CheckpointQuery, CheckpointType,
    checkpoint_struct_api::{Checkpoint, CheckpointMetadata},
    checkpoint_trait::{CheckpointError, CheckpointStats, Checkpointer, CleanupPolicy},
};

/// PostgreSQL 检查点保存器配置
#[derive(Debug, Clone)]
pub struct PostgresSaverConfig {
    /// 数据库连接 URL
    pub database_url: String,
    /// 连接池大小
    pub pool_size: u32,
    /// 是否自动创建表
    pub auto_create_table: bool,
}

impl PostgresSaverConfig {
    /// 创建配置
    pub fn new(database_url: String) -> Self {
        Self {
            database_url,
            pool_size: 10,
            auto_create_table: true,
        }
    }

    /// 设置连接池大小
    pub fn with_pool_size(mut self, size: u32) -> Self {
        self.pool_size = size;
        self
    }

    /// 设置是否自动创建表
    pub fn with_auto_create(mut self, auto: bool) -> Self {
        self.auto_create_table = auto;
        self
    }
}

/// PostgreSQL 检查点保存器
#[derive(Debug, Clone)]
pub struct PostgresSaver {
    pool: PgPool,
}

impl PostgresSaver {
    /// 创建新的 PostgreSQL 检查点保存器
    pub async fn new(config: PostgresSaverConfig) -> Result<Self, CheckpointError> {
        // 解析连接选项
        let options = PgConnectOptions::from_str(&config.database_url)
            .map_err(|e| CheckpointError::Storage(format!("Invalid database URL: {}", e)))?;

        // 创建连接池
        let pool = PgPool::connect_with(options)
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
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// 返回连接池的引用
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// 初始化数据库表结构
    async fn init_schema(&self) -> Result<(), CheckpointError> {
        let query_vec = vec![
            r#"
            CREATE TABLE IF NOT EXISTS langchain_rs_checkpoints (
                id TEXT PRIMARY KEY NOT NULL,
                parent_id TEXT,
                thread_id TEXT NOT NULL,
                created_at BIGINT NOT NULL,
                step INTEGER NOT NULL,
                checkpoint_type TEXT NOT NULL,
                tags JSONB,
                state_json JSONB NOT NULL,
                next_nodes JSONB NOT NULL,
                pending_interrupt JSONB,
                size_bytes INTEGER NOT NULL DEFAULT 0,
                updated_at BIGINT NOT NULL
            );"#,
            r#"CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_id
                ON langchain_rs_checkpoints(thread_id);"#,
            r#"CREATE INDEX IF NOT EXISTS idx_checkpoints_parent_id
                ON langchain_rs_checkpoints(parent_id);"#,
            r#"CREATE INDEX IF NOT EXISTS idx_checkpoints_created_at
                ON langchain_rs_checkpoints(created_at);"#,
            r#"CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_created
                ON langchain_rs_checkpoints(thread_id, created_at DESC);"#,
            r#"CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_step
                ON langchain_rs_checkpoints(thread_id, step);"#,
            r#"CREATE INDEX IF NOT EXISTS idx_checkpoints_tags
                ON langchain_rs_checkpoints USING GIN (tags);"#,
            r#"CREATE INDEX IF NOT EXISTS idx_checkpoints_type_partial
                ON langchain_rs_checkpoints(checkpoint_type) WHERE checkpoint_type = 'Final';"#,
        ];

        for query in query_vec {
            sqlx::query(query)
                .execute(&self.pool)
                .await
                .map_err(|e| CheckpointError::Storage(format!("Failed to create table: {}", e)))?;
        }

        tracing::info!("PostgreSQL checkpoint schema initialized");
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
        row: &sqlx::postgres::PgRow,
    ) -> Result<Checkpoint<S>, CheckpointError> {
        let checkpoint_type_str: String = row
            .try_get("checkpoint_type")
            .map_err(|e| CheckpointError::Storage(format!("Missing checkpoint_type: {}", e)))?;

        let checkpoint_type = match checkpoint_type_str.as_str() {
            "Auto" => CheckpointType::Auto,
            "Manual" => CheckpointType::Manual,
            "InterruptBefore" => CheckpointType::InterruptBefore,
            "InterruptAfter" => CheckpointType::InterruptAfter,
            "Final" => CheckpointType::Final,
            _ => {
                return Err(CheckpointError::Storage(format!(
                    "Unknown checkpoint type: {}",
                    checkpoint_type_str
                )));
            }
        };
        let tags_json: Option<String> = row.try_get("tags").ok();
        let tags = Self::parse_tags(tags_json.as_deref())?;
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
            step: row
                .try_get::<i32, _>("step")
                .map_err(|e| CheckpointError::Storage(format!("Missing step: {}", e)))?
                .try_into()
                .map_err(|_| {
                    CheckpointError::Storage("Step value too large for usize".to_owned())
                })?,
            tags,
            checkpoint_type,
        };
        let state_json: serde_json::Value = row
            .try_get("state_json")
            .map_err(|e| CheckpointError::Storage(format!("Missing state_json: {}", e)))?;
        let state = serde_json::from_value(state_json).map_err(|e| {
            CheckpointError::Serialization(format!("Failed to deserialize state: {}", e))
        })?;
        let next_nodes_jsonb: serde_json::Value = row
            .try_get("next_nodes")
            .map_err(|e| CheckpointError::Storage(format!("Missing next_nodes: {}", e)))?;
        let next_nodes = serde_json::from_value(next_nodes_jsonb).map_err(|e| {
            CheckpointError::Serialization(format!("Failed to deserialize next_nodes: {}", e))
        })?;
        let pending_interrupt: Option<serde_json::Value> = row.try_get("pending_interrupt").ok();
        let pending_interrupt = if let Some(val) = pending_interrupt {
            Some(serde_json::from_value(val).map_err(|e| {
                CheckpointError::Serialization(format!(
                    "Failed to deserialize pending_interrupt: {}",
                    e
                ))
            })?)
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
    fn row_to_metadata(row: &sqlx::postgres::PgRow) -> Result<CheckpointMetadata, CheckpointError> {
        let checkpoint_type_str: String = row
            .try_get("checkpoint_type")
            .map_err(|e| CheckpointError::Storage(format!("Missing checkpoint_type: {}", e)))?;
        let checkpoint_type = match checkpoint_type_str.as_str() {
            "Auto" => CheckpointType::Auto,
            "Manual" => CheckpointType::Manual,
            "InterruptBefore" => CheckpointType::InterruptBefore,
            "InterruptAfter" => CheckpointType::InterruptAfter,
            "Final" => CheckpointType::Final,
            _ => {
                return Err(CheckpointError::Storage(format!(
                    "Unknown checkpoint type: {}",
                    checkpoint_type_str
                )));
            }
        };

        let tags_json: Option<String> = row.try_get("tags").ok();
        let tags = Self::parse_tags(tags_json.as_deref())?;

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
            step: row
                .try_get::<i32, _>("step")
                .map_err(|e| CheckpointError::Storage(format!("Missing step: {}", e)))?
                .try_into()
                .map_err(|_| {
                    CheckpointError::Storage("Step value too large for usize".to_owned())
                })?,
            tags,
            checkpoint_type,
        })
    }
}

#[async_trait]
impl<S> Checkpointer<S> for PostgresSaver
where
    S: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    async fn get(&self, thread_id: &str) -> Result<Option<Checkpoint<S>>, CheckpointError> {
        let query = r#"
            SELECT * FROM langchain_rs_checkpoints
            WHERE thread_id = $1
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
                let checkpoint = Self::row_to_checkpoint::<S>(&r)?;
                Ok(Some(checkpoint))
            }
            None => Ok(None),
        }
    }

    async fn put(&self, checkpoint: &Checkpoint<S>) -> Result<(), CheckpointError> {
        let now = Utc::now().timestamp();
        let size_bytes = Self::calculate_size(checkpoint)?;

        let state_json: serde_json::Value = serde_json::to_value(&checkpoint.state)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        let next_nodes: serde_json::Value = serde_json::to_value(&checkpoint.next_nodes)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        let tags: serde_json::Value = serde_json::to_value(&checkpoint.metadata.tags)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
        let pending_interrupt: Option<serde_json::Value> = checkpoint
            .pending_interrupt
            .as_ref()
            .and_then(|interrupt| serde_json::to_value(interrupt).ok());
        let checkpoint_type_str =
            Self::checkpoint_type_to_string(&checkpoint.metadata.checkpoint_type);

        let query = r#"
            INSERT INTO langchain_rs_checkpoints (
                id, parent_id, thread_id, created_at, step, checkpoint_type,
                tags, state_json, next_nodes, pending_interrupt, size_bytes, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT(id) DO UPDATE SET
                parent_id = EXCLUDED.parent_id,
                thread_id = EXCLUDED.thread_id,
                created_at = EXCLUDED.created_at,
                step = EXCLUDED.step,
                checkpoint_type = EXCLUDED.checkpoint_type,
                tags = EXCLUDED.tags,
                state_json = EXCLUDED.state_json,
                next_nodes = EXCLUDED.next_nodes,
                pending_interrupt = EXCLUDED.pending_interrupt,
                size_bytes = EXCLUDED.size_bytes,
                updated_at = EXCLUDED.updated_at
        "#;

        sqlx::query(query)
            .bind(&checkpoint.metadata.id)
            .bind(&checkpoint.metadata.parent_id)
            .bind(&checkpoint.metadata.thread_id)
            .bind(checkpoint.metadata.created_at)
            .bind(checkpoint.metadata.step as i64)
            .bind(checkpoint_type_str)
            .bind(tags)
            .bind(state_json)
            .bind(next_nodes)
            .bind(pending_interrupt)
            .bind(size_bytes as i64)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Insert failed: {}", e)))?;

        Ok(())
    }

    async fn delete(&self, thread_id: &str) -> Result<(), CheckpointError> {
        let query = "DELETE FROM langchain_rs_checkpoints WHERE thread_id = $1";

        sqlx::query(query)
            .bind(thread_id)
            .execute(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Delete failed: {}", e)))?;

        Ok(())
    }

    async fn delete_checkpoint(&self, checkpoint_id: &CheckpointId) -> Result<(), CheckpointError> {
        let query = "DELETE FROM langchain_rs_checkpoints WHERE id = $1";

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
            SELECT * FROM langchain_rs_checkpoints
            WHERE thread_id = $1
            ORDER BY id DESC
            LIMIT $2
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
        let mut sql = "SELECT * FROM langchain_rs_checkpoints WHERE 1=1".to_owned();
        let mut bind_index = 1;

        if query.thread_id.is_some() {
            sql.push_str(&format!(" AND thread_id = ${}", bind_index));
            bind_index += 1;
        }

        if query.start_time.is_some() {
            sql.push_str(&format!(" AND created_at >= ${}", bind_index));
            bind_index += 1;
        }

        if query.end_time.is_some() {
            sql.push_str(&format!(" AND created_at <= ${}", bind_index));
            bind_index += 1;
        }

        if query.checkpoint_type.is_some() {
            sql.push_str(&format!(" AND checkpoint_type = ${}", bind_index));
            bind_index += 1;
        }

        if let Some(ref tags) = query.tags {
            if !tags.is_empty() {
                sql.push_str(&format!(" AND tags @> ${}", bind_index));
            }
        }

        match query.order {
            CheckpointOrder::Desc => sql.push_str(" ORDER BY created_at DESC"),
            CheckpointOrder::Asc => sql.push_str(" ORDER BY created_at ASC"),
        }

        if let Some(limit) = query.limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }

        let mut q = sqlx::query(&sql);

        if let Some(ref thread_id) = query.thread_id {
            q = q.bind(thread_id);
        }
        if let Some(start) = query.start_time {
            q = q.bind(start);
        }
        if let Some(end) = query.end_time {
            q = q.bind(end);
        }
        if let Some(ref cp_type) = query.checkpoint_type {
            q = q.bind(Self::checkpoint_type_to_string(cp_type));
        }
        if let Some(ref tags) = query.tags {
            if !tags.is_empty() {
                let tags_json = serde_json::to_value(tags)
                    .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
                q = q.bind(tags_json);
            }
        }

        let rows = q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Search failed: {}", e)))?;

        let checkpoints: Result<Vec<_>, _> = rows.iter().map(Self::row_to_metadata).collect();
        let checkpoints = checkpoints?;

        Ok(CheckpointListResult {
            total_count: checkpoints.len(),
            checkpoints,
        })
    }

    async fn get_by_id(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Option<Checkpoint<S>>, CheckpointError> {
        let query = "SELECT * FROM langchain_rs_checkpoints WHERE id = $1";

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
        let query = "SELECT * FROM langchain_rs_checkpoints WHERE id = $1";

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
        let query = "SELECT parent_id FROM langchain_rs_checkpoints WHERE id = $1";

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
        // PostgreSQL 递归 CTE 查询
        let query = r#"
            WITH RECURSIVE checkpoint_chain AS (
                -- 初始节点
                SELECT * FROM langchain_rs_checkpoints WHERE id = $1
                UNION ALL
                -- 递归获取父节点
                SELECT c.* FROM langchain_rs_checkpoints c
                INNER JOIN checkpoint_chain cc ON c.id = cc.parent_id
            )
            SELECT * FROM checkpoint_chain ORDER BY created_at ASC
        "#;

        let rows = sqlx::query(query)
            .bind(checkpoint_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;

        rows.iter().map(Self::row_to_metadata).collect()
    }

    async fn get_at_time(
        &self,
        thread_id: &str,
        time: i64,
    ) -> Result<Option<Checkpoint<S>>, CheckpointError> {
        let query = r#"
            SELECT * FROM langchain_rs_checkpoints
            WHERE thread_id = $1 AND created_at <= $2
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
        match policy {
            CleanupPolicy::KeepLast(n) => {
                // PostgreSQL 窗口函数优化
                let query = r#"
                    DELETE FROM langchain_rs_checkpoints
                    WHERE ctid IN (
                        SELECT ctid FROM (
                            SELECT ctid, ROW_NUMBER() OVER (
                                PARTITION BY thread_id
                                ORDER BY created_at ASC
                            ) as rn
                            FROM langchain_rs_checkpoints
                        ) sub
                        WHERE rn > $1
                    )
                "#;

                let result = sqlx::query(query)
                    .bind(*n as i64)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| CheckpointError::Storage(format!("Cleanup failed: {}", e)))?;

                Ok(result.rows_affected() as usize)
            }
            CleanupPolicy::KeepDays(days) => {
                let cutoff = Utc::now().timestamp() - (days * 86400);

                let query = "DELETE FROM langchain_rs_checkpoints WHERE created_at < $1";

                let result = sqlx::query(query)
                    .bind(cutoff)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| CheckpointError::Storage(format!("Cleanup failed: {}", e)))?;

                Ok(result.rows_affected() as usize)
            }
            CleanupPolicy::KeepMaxSizeBytes(max_size) => {
                // 计算总大小
                let size_query = "SELECT SUM(size_bytes) as total FROM langchain_rs_checkpoints";
                let size_row = sqlx::query(size_query)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|e| CheckpointError::Storage(format!("Query failed: {}", e)))?;
                let total_size: i64 = size_row.try_get("total").unwrap_or(0);

                if total_size as usize <= *max_size {
                    return Ok(0);
                }

                // 使用窗口函数删除最旧的检查点
                let query = r#"
                    WITH ordered_checkpoints AS (
                        SELECT id, size_bytes,
                            SUM(size_bytes) OVER (ORDER BY created_at ASC) as cumulative_size
                        FROM langchain_rs_checkpoints
                        ORDER BY created_at ASC
                    )
                    DELETE FROM langchain_rs_checkpoints
                    WHERE id IN (
                        SELECT id FROM ordered_checkpoints
                        WHERE cumulative_size > $1
                    )
                "#;

                let result = sqlx::query(query)
                    .bind(*max_size as i64)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| CheckpointError::Storage(format!("Cleanup failed: {}", e)))?;

                Ok(result.rows_affected() as usize)
            }
        }
    }

    async fn stats(&self, thread_id: Option<&str>) -> Result<CheckpointStats, CheckpointError> {
        let (count_query, time_query) = if let Some(_tid) = thread_id {
            (
                "SELECT COUNT(*) as count, SUM(size_bytes) as size FROM langchain_rs_checkpoints WHERE thread_id = $1",
                "SELECT MIN(created_at) as oldest, MAX(created_at) as newest FROM langchain_rs_checkpoints WHERE thread_id = $1",
            )
        } else {
            (
                "SELECT COUNT(*) as count, SUM(size_bytes) as size FROM langchain_rs_checkpoints",
                "SELECT MIN(created_at) as oldest, MAX(created_at) as newest FROM langchain_rs_checkpoints",
            )
        };

        // 获取计数和大小
        let mut count_q = sqlx::query(count_query);
        if let Some(thread_id) = thread_id {
            count_q = count_q.bind(thread_id);
        }
        let stats_row = count_q
            .fetch_one(&self.pool)
            .await
            .map_err(|e| CheckpointError::Storage(format!("Stats query failed: {}", e)))?;
        let total_count: i64 = stats_row.try_get("count").unwrap_or(0);
        let total_size: i64 = stats_row.try_get("size").unwrap_or(0);

        // 获取时间范围
        let mut time_q = sqlx::query(time_query);
        if let Some(thread_id) = thread_id {
            time_q = time_q.bind(thread_id);
        }

        let time_row = time_q
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
