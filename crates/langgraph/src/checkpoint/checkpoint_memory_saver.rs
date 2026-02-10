use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::checkpoint::checkpoint_struct_api::{Checkpoint, CheckpointMetadata};
use crate::checkpoint::checkpoint_trait::{
    CheckpointError, CheckpointStats, Checkpointer, CleanupPolicy,
};
use crate::checkpoint::{CheckpointId, CheckpointListResult, CheckpointOrder, CheckpointQuery};

pub type MemorySaverStorage = Arc<RwLock<HashMap<String, HashMap<CheckpointId, Vec<u8>>>>>;

/// 内存检查点保存器
#[derive(Debug, Clone)]
pub struct MemorySaver {
    /// 存储：thread_id -> (checkpoint_id -> checkpoint)
    storage: MemorySaverStorage,
    /// 元数据索引：thread_id -> vec of metadata
    metadata_index: Arc<RwLock<HashMap<String, Vec<CheckpointMetadata>>>>,
}

impl MemorySaver {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
            metadata_index: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for MemorySaver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<S> Checkpointer<S> for MemorySaver
where
    S: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    async fn get(&self, thread_id: &str) -> Result<Option<Checkpoint<S>>, CheckpointError> {
        let storage = self.storage.read().await;
        let index = self.metadata_index.read().await;

        if let Some(checkpoints) = storage.get(thread_id) {
            // 获取最新的检查点
            if let Some(metadata) = index.get(thread_id).and_then(|v| v.last())
                && let Some(data) = checkpoints.get(&metadata.id)
            {
                let checkpoint: Checkpoint<S> = serde_json::from_slice(data)
                    .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
                return Ok(Some(checkpoint));
            }
        }
        Ok(None)
    }

    async fn put(&self, checkpoint: &Checkpoint<S>) -> Result<(), CheckpointError> {
        let data = serde_json::to_vec(checkpoint)
            .map_err(|e| CheckpointError::Serialization(e.to_string()))?;

        let mut storage = self.storage.write().await;
        let mut index = self.metadata_index.write().await;

        let thread_id = checkpoint.metadata.thread_id.clone();
        let checkpoint_id = checkpoint.metadata.id.clone();

        storage
            .entry(thread_id.clone())
            .or_insert_with(HashMap::new)
            .insert(checkpoint_id.clone(), data);

        index
            .entry(thread_id)
            .or_insert_with(Vec::new)
            .push(checkpoint.metadata.clone());

        Ok(())
    }

    async fn delete(&self, thread_id: &str) -> Result<(), CheckpointError> {
        let mut storage = self.storage.write().await;
        let mut index = self.metadata_index.write().await;

        storage.remove(thread_id);
        index.remove(thread_id);

        Ok(())
    }

    async fn delete_checkpoint(&self, checkpoint_id: &CheckpointId) -> Result<(), CheckpointError> {
        let mut storage = self.storage.write().await;
        let mut index = self.metadata_index.write().await;

        // 遍历所有线程查找并删除
        for (_, checkpoints) in storage.iter_mut() {
            checkpoints.remove(checkpoint_id);
        }

        for (_, metadatas) in index.iter_mut() {
            metadatas.retain(|m| &m.id != checkpoint_id);
        }

        Ok(())
    }

    async fn list(
        &self,
        thread_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<CheckpointMetadata>, CheckpointError> {
        let index = self.metadata_index.read().await;

        if let Some(metadatas) = index.get(thread_id) {
            let result = if let Some(limit) = limit {
                metadatas.iter().rev().take(limit).cloned().collect()
            } else {
                metadatas.clone()
            };
            Ok(result)
        } else {
            Ok(Vec::new())
        }
    }

    async fn search(
        &self,
        query: CheckpointQuery,
    ) -> Result<CheckpointListResult, CheckpointError> {
        let index = self.metadata_index.read().await;

        let mut results = Vec::new();

        for (thread_id, metadatas) in index.iter() {
            // 过滤 thread_id
            if let Some(ref query_thread_id) = query.thread_id
                && thread_id != query_thread_id
            {
                continue;
            }

            for metadata in metadatas {
                let mut match_condition = true;

                // 过滤时间范围
                if let Some(start) = query.start_time
                    && metadata.created_at < start
                {
                    match_condition = false;
                }
                if let Some(end) = query.end_time
                    && metadata.created_at > end
                {
                    match_condition = false;
                }

                // 过滤类型
                if let Some(ref cp_type) = query.checkpoint_type
                    && &metadata.checkpoint_type != cp_type
                {
                    match_condition = false;
                }

                // 过滤标签
                if let Some(ref tags) = query.tags {
                    for (key, value) in tags.iter() {
                        if metadata.tags.get(key) != Some(value) {
                            match_condition = false;
                            break;
                        }
                    }
                }

                if match_condition {
                    results.push(metadata.clone());
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

        // 限制数量
        let total_count = results.len();
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
        let storage = self.storage.read().await;

        for (_, checkpoints) in storage.iter() {
            if let Some(data) = checkpoints.get(checkpoint_id) {
                let checkpoint: Checkpoint<S> = serde_json::from_slice(data)
                    .map_err(|e| CheckpointError::Serialization(e.to_string()))?;
                return Ok(Some(checkpoint));
            }
        }

        Ok(None)
    }

    async fn get_metadata(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Option<CheckpointMetadata>, CheckpointError> {
        let index = self.metadata_index.read().await;

        for (_, metadatas) in index.iter() {
            if let Some(metadata) = metadatas.iter().find(|m| &m.id == checkpoint_id) {
                return Ok(Some(metadata.clone()));
            }
        }

        Ok(None)
    }

    async fn get_metadata_parent_id(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Option<String>, CheckpointError> {
        let index = self.metadata_index.read().await;

        for (_, metadatas) in index.iter() {
            if let Some(metadata) = metadatas.iter().find(|m| &m.id == checkpoint_id) {
                return Ok(metadata.parent_id.clone());
            }
        }

        Ok(None)
    }

    async fn get_history(
        &self,
        checkpoint_id: &CheckpointId,
    ) -> Result<Vec<CheckpointMetadata>, CheckpointError> {
        let mut history = Vec::new();
        let mut current_id = Some(checkpoint_id.clone());
        let index = self.metadata_index.read().await;

        while let Some(id) = &current_id {
            let mut found = false;
            for (_, metadatas) in index.iter() {
                if let Some(metadata) = metadatas.iter().find(|m| m.id.eq(id)) {
                    history.push(metadata.clone());
                    current_id = metadata.parent_id.clone();
                    found = true;
                    break;
                }
            }
            if !found {
                break;
            }
        }

        // 反转使其按时间顺序排列（从旧到新）
        history.reverse();
        Ok(history)
    }

    async fn get_at_time(
        &self,
        thread_id: &str,
        time: i64,
    ) -> Result<Option<Checkpoint<S>>, CheckpointError> {
        let index = self.metadata_index.read().await;

        if let Some(metadatas) = index.get(thread_id) {
            // 找到最接近指定时间的检查点
            let metadata = metadatas.iter().rfind(|m| m.created_at <= time);

            if let Some(metadata) = metadata {
                return self.get_by_id(&metadata.id).await;
            }
        }

        Ok(None)
    }

    async fn cleanup(&self, policy: &CleanupPolicy) -> Result<usize, CheckpointError> {
        let index = self.metadata_index.read().await;

        let mut to_delete = Vec::new();

        match policy {
            CleanupPolicy::KeepLast(n) => {
                for (_, metadatas) in index.iter() {
                    let len = metadatas.len();
                    if len > *n {
                        // 保留最后 n 个，删除前面的
                        for metadata in metadatas.iter().take(len - n) {
                            to_delete.push(metadata.id.clone());
                        }
                    }
                }
            }
            CleanupPolicy::KeepDays(days) => {
                let cutoff = (Utc::now() - chrono::Duration::days(*days)).timestamp();
                for (_, metadatas) in index.iter() {
                    for metadata in metadatas.iter() {
                        if metadata.created_at < cutoff {
                            to_delete.push(metadata.id.clone());
                        }
                    }
                }
            }
            // CleanupPolicy::Custom(predicate) => {
            //     for (_, metadatas) in index.iter() {
            //         for metadata in metadatas.iter() {
            //             if predicate(metadata) {
            //                 to_delete.push(metadata.id.clone());
            //             }
            //         }
            //     }
            // }
            _ => {}
        }

        // 释放锁后再删除
        drop(index);

        let count = 0;
        // for checkpoint_id in to_delete {
        //     self.delete_checkpoint(&checkpoint_id).await?;
        //     count += 1;
        // }

        Ok(count)
    }

    async fn stats(&self, thread_id: Option<&str>) -> Result<CheckpointStats, CheckpointError> {
        let storage = self.storage.read().await;
        let index = self.metadata_index.read().await;

        let mut total_count = 0;
        let mut total_size = 0;
        let mut oldest = None;
        let mut newest = None;

        let thread_ids: Vec<_> = if let Some(tid) = thread_id {
            vec![tid.to_owned()]
        } else {
            index.keys().cloned().collect()
        };

        for tid in thread_ids {
            if let Some(checkpoints) = storage.get(&tid) {
                for data in checkpoints.values() {
                    total_count += 1;
                    total_size += data.len();
                }
            }

            if let Some(metadatas) = index.get(&tid) {
                for metadata in metadatas {
                    if oldest.is_none() || Some(metadata.created_at) < oldest {
                        oldest = Some(metadata.created_at);
                    }
                    if newest.is_none() || Some(metadata.created_at) > newest {
                        newest = Some(metadata.created_at);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn checkpoint_memory_saver_basic_flow() {
        use crate::checkpoint::CheckpointType;
        use uuid::*;

        let saver = MemorySaver::new();

        // 创建检查点
        let checkpoint = Checkpoint {
            metadata: CheckpointMetadata {
                id: Uuid::now_v7().to_string(),
                parent_id: None,
                thread_id: "thread-1".to_owned(),
                created_at: Utc::now().timestamp(),
                step: 1,
                // source_node: "Llm".to_string(),
                tags: HashMap::new(),
                checkpoint_type: CheckpointType::Auto,
            },
            state: 42,
            next_nodes: vec!["Tool".to_owned()],
            pending_interrupt: None,
        };

        // 保存
        saver.put(&checkpoint).await.unwrap();

        // 读取
        let loaded: Option<Checkpoint<i32>> = saver.get("thread-1").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().state, 42);
    }

    #[tokio::test]
    async fn checkpoint_memory_saver_multi_node_process() {
        use crate::checkpoint::CheckpointType;
        use uuid::*;

        let saver = MemorySaver::new();

        // 创建父检查点
        let checkpoint_parent_id = Uuid::now_v7().to_string();
        let checkpoint_parent = Checkpoint {
            metadata: CheckpointMetadata {
                id: checkpoint_parent_id.clone(),
                parent_id: None,
                thread_id: "thread-1".to_owned(),
                created_at: Utc::now().timestamp(),
                step: 1,
                // source_node: "Llm".to_string(),
                tags: HashMap::new(),
                checkpoint_type: CheckpointType::Auto,
            },
            state: 42,
            next_nodes: vec!["Tool".to_owned()],
            pending_interrupt: None,
        };

        // 创建子检查点
        let checkpoint_child_id = Uuid::now_v7().to_string();
        let checkpoint_child = Checkpoint {
            metadata: CheckpointMetadata {
                id: checkpoint_child_id.clone(),
                parent_id: Some(checkpoint_parent_id.clone()),
                thread_id: "thread-1".to_owned(),
                created_at: Utc::now().timestamp(),
                step: 1,
                // source_node: "Llm".to_string(),
                tags: HashMap::new(),
                checkpoint_type: CheckpointType::Auto,
            },
            state: 43,
            next_nodes: vec!["Tool".to_owned()],
            pending_interrupt: None,
        };

        // 保存
        saver.put(&checkpoint_parent).await.unwrap();
        saver.put(&checkpoint_child).await.unwrap();

        // 读取
        let loaded: Option<Checkpoint<i32>> = saver.get("thread-1").await.unwrap();
        assert!(loaded.is_some());
        assert!(loaded.clone().unwrap().metadata.parent_id.clone().is_some());
        assert_eq!(
            loaded.unwrap().metadata.parent_id.unwrap(),
            checkpoint_parent_id.clone()
        );

        // 根据 id 读取父checkpoint
        let checkpoint_parent: Option<Checkpoint<i32>> =
            saver.get_by_id(&checkpoint_parent_id).await.unwrap();
        assert!(checkpoint_parent.is_some());
        assert_eq!(
            checkpoint_parent.unwrap().metadata.id,
            checkpoint_parent_id.clone()
        );
        // 根据 id 读取子checkpoint
        let checkpoint_child: Option<Checkpoint<i32>> =
            saver.get_by_id(&checkpoint_child_id).await.unwrap();
        assert!(checkpoint_child.is_some());
        assert_eq!(
            checkpoint_child
                .clone()
                .unwrap()
                .metadata
                .parent_id
                .unwrap(),
            checkpoint_parent_id.clone()
        );
        assert_eq!(
            checkpoint_child.unwrap().metadata.id,
            checkpoint_child_id.clone()
        );
    }
}
