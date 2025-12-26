use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

/// 运行配置，用于标识 Checkpoint 的唯一性（如线程ID）
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunnableConfig {
    /// 线程 ID，用于隔离不同的对话或执行流
    pub thread_id: String,
    /// 检查点 ID，可选。如果提供，则加载特定版本的检查点
    pub checkpoint_id: Option<String>,
}

/// 检查点数据结构，包含业务状态和执行流位置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint<S> {
    /// 业务状态 (State)
    pub state: S,
    /// 下一步需要执行的节点 ID 列表
    /// 由于 InternedGraphLabel 无法直接序列化，这里存储字符串形式的 Label
    pub next_nodes: Vec<String>,
}

/// 序列化后的检查点数据（底层存储格式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointBlob {
    /// 序列化后的状态数据（JSON 字符串或字节流）
    /// 这里为了通用性使用 String (JSON)，也可以改为 Vec<u8>
    pub state: String,
    /// 下一步节点列表
    pub next_nodes: Vec<String>,
}

/// 检查点保存器接口 (Trait)
/// 负责持久化存储和加载图的执行状态
#[async_trait]
pub trait Checkpointer: Send + Sync {
    /// 获取最新的检查点
    ///
    /// # 参数
    /// * `config` - 运行配置，包含 thread_id
    ///
    /// # 返回
    /// * `Option<CheckpointBlob>` - 如果存在则返回序列化后的检查点，否则返回 None
    async fn get(&self, config: &RunnableConfig) -> Result<Option<CheckpointBlob>, anyhow::Error>;

    /// 保存检查点
    ///
    /// # 参数
    /// * `config` - 运行配置
    /// * `checkpoint` - 序列化后的检查点数据
    async fn put(
        &self,
        config: &RunnableConfig,
        checkpoint: &CheckpointBlob,
    ) -> Result<(), anyhow::Error>;
}

/// 内存实现的检查点保存器 (MemorySaver)
/// 仅用于开发阶段测试或非持久化场景
#[derive(Debug, Default, Clone)]
pub struct MemorySaver {
    /// 存储结构：thread_id -> CheckpointBlob
    storage: Arc<Mutex<HashMap<String, CheckpointBlob>>>,
}

impl MemorySaver {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl Checkpointer for MemorySaver {
    async fn get(&self, config: &RunnableConfig) -> Result<Option<CheckpointBlob>, anyhow::Error> {
        let storage = self.storage.lock().await;
        // 目前只支持获取最新版，忽略 checkpoint_id
        Ok(storage.get(&config.thread_id).cloned())
    }

    async fn put(
        &self,
        config: &RunnableConfig,
        checkpoint: &CheckpointBlob,
    ) -> Result<(), anyhow::Error> {
        let mut storage = self.storage.lock().await;
        storage.insert(config.thread_id.clone(), checkpoint.clone());
        Ok(())
    }
}

/// 扩展方法：方便在 Checkpointer 和具体的 Checkpoint<S> 之间转换
#[async_trait]
pub trait CheckpointerExt {
    async fn get_state<S: DeserializeOwned + Send>(
        &self,
        config: &RunnableConfig,
    ) -> Result<Option<Checkpoint<S>>, anyhow::Error>;

    async fn put_state<S: Serialize + Send + Sync>(
        &self,
        config: &RunnableConfig,
        checkpoint: &Checkpoint<S>,
    ) -> Result<(), anyhow::Error>;
}

#[async_trait]
impl<T: Checkpointer + ?Sized> CheckpointerExt for T {
    async fn get_state<S: DeserializeOwned + Send>(
        &self,
        config: &RunnableConfig,
    ) -> Result<Option<Checkpoint<S>>, anyhow::Error> {
        let blob = self.get(config).await?;
        match blob {
            Some(blob) => {
                let state: S = serde_json::from_str(&blob.state)?;
                Ok(Some(Checkpoint {
                    state,
                    next_nodes: blob.next_nodes,
                }))
            }
            None => Ok(None),
        }
    }

    async fn put_state<S: Serialize + Send + Sync>(
        &self,
        config: &RunnableConfig,
        checkpoint: &Checkpoint<S>,
    ) -> Result<(), anyhow::Error> {
        let state_json = serde_json::to_string(&checkpoint.state)?;
        let blob = CheckpointBlob {
            state: state_json,
            next_nodes: checkpoint.next_nodes.clone(),
        };
        self.put(config, &blob).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct TestState {
        count: i32,
        messages: Vec<String>,
    }

    #[tokio::test]
    async fn test_memory_saver_flow() {
        let saver = MemorySaver::new();
        let config = RunnableConfig {
            thread_id: "thread-1".to_owned(),
            checkpoint_id: None,
        };

        let state = TestState {
            count: 42,
            messages: vec!["hello".to_owned(), "world".to_owned()],
        };

        let checkpoint = Checkpoint {
            state: state.clone(),
            next_nodes: vec!["node_b".to_owned()],
        };

        // Save
        saver.put_state(&config, &checkpoint).await.unwrap();

        // Load
        let loaded: Option<Checkpoint<TestState>> = saver.get_state(&config).await.unwrap();

        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.state, state);
        assert_eq!(loaded.next_nodes, vec!["node_b".to_owned()]);
    }

    #[tokio::test]
    async fn test_memory_saver_isolation() {
        let saver = MemorySaver::new();
        let config1 = RunnableConfig {
            thread_id: "thread-1".to_owned(),
            checkpoint_id: None,
        };
        let config2 = RunnableConfig {
            thread_id: "thread-2".to_owned(),
            checkpoint_id: None,
        };

        saver
            .put_state(
                &config1,
                &Checkpoint {
                    state: 1,
                    next_nodes: vec![],
                },
            )
            .await
            .unwrap();

        let loaded2: Option<Checkpoint<i32>> = saver.get_state(&config2).await.unwrap();
        assert!(loaded2.is_none());
    }
}
