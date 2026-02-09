use crate::interrupt::Interrupt;
use async_trait::async_trait;
use langchain_core::request::ResponseFormat;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

/// 运行配置，用于标识 Checkpoint 的唯一性（如线程ID）
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunnableConfig {
    /// 线程 ID，用于隔离不同的对话或执行流
    pub thread_id: Option<String>,
    pub response_format: Option<ResponseFormat>,
}

/// 检查点数据结构，包含业务状态和执行流位置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint<S> {
    /// 业务状态 (State)
    pub state: S,
    /// 下一步需要执行的节点 ID 列表
    /// 由于 InternedGraphLabel 无法直接序列化，这里存储字符串形式的 Label
    pub next_nodes: Vec<String>,
    /// 待处理的中断（如果有）
    pub pending_interrupt: Option<Interrupt>,
}

/// 检查点保存器接口 (Trait)
/// 负责持久化存储和加载图的执行状态
///
/// S: 状态类型
#[async_trait]
pub trait Checkpointer<S>: Send + Sync {
    /// 获取最新的检查点
    ///
    /// # 参数
    /// * `config` - 运行配置，包含 thread_id
    ///
    /// # 返回
    /// * `Option<Checkpoint<S>>` - 如果存在则返回检查点，否则返回 None
    async fn get(&self, thread_id: &str) -> Result<Option<Checkpoint<S>>, anyhow::Error>;

    /// 保存检查点
    ///
    /// # 参数
    /// * `config` - 运行配置
    /// * `checkpoint` - 检查点数据
    async fn put(&self, thread_id: &str, checkpoint: &Checkpoint<S>) -> Result<(), anyhow::Error>;
}

/// 内存实现的检查点保存器 (MemorySaver)
/// 仅用于开发阶段测试或非持久化场景
///
/// 内部存储使用序列化后的 Vec<u8> 以模拟持久化存储的行为，
/// 同时也允许它支持任何可序列化的状态类型。
#[derive(Debug, Default, Clone)]
pub struct MemorySaver {
    /// 存储结构：thread_id -> serialized_checkpoint (Vec<u8>)
    storage: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl MemorySaver {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl<S> Checkpointer<S> for MemorySaver
where
    S: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    async fn get(&self, thread_id: &str) -> Result<Option<Checkpoint<S>>, anyhow::Error> {
        let storage = self.storage.lock().await;
        if let Some(data) = storage.get(thread_id) {
            let checkpoint: Checkpoint<S> = serde_json::from_slice(data)?;
            Ok(Some(checkpoint))
        } else {
            Ok(None)
        }
    }

    async fn put(&self, thread_id: &str, checkpoint: &Checkpoint<S>) -> Result<(), anyhow::Error> {
        let mut storage = self.storage.lock().await;
        let data = serde_json::to_vec(checkpoint)?;
        storage.insert(thread_id.to_owned(), data);

        Ok(())
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
            thread_id: Some("thread-1".to_owned()),
            response_format: None,
        };

        let state = TestState {
            count: 42,
            messages: vec!["hello".to_owned(), "world".to_owned()],
        };

        let checkpoint = Checkpoint {
            state: state.clone(),
            next_nodes: vec!["node_b".to_owned()],
            pending_interrupt: None,
        };

        // Save
        Checkpointer::put(&saver, config.thread_id.as_ref().unwrap(), &checkpoint)
            .await
            .unwrap();

        // Load
        let loaded: Option<Checkpoint<TestState>> =
            Checkpointer::get(&saver, config.thread_id.as_ref().unwrap())
                .await
                .unwrap();

        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.state, state);
        assert_eq!(loaded.next_nodes, vec!["node_b".to_owned()]);
    }

    #[tokio::test]
    async fn test_memory_saver_isolation() {
        let saver = MemorySaver::new();
        let config1 = RunnableConfig {
            thread_id: Some("thread-1".to_owned()),
            response_format: None,
        };
        let config2 = RunnableConfig {
            thread_id: Some("thread-2".to_owned()),
            response_format: None,
        };

        Checkpointer::<i32>::put(
            &saver,
            config1.thread_id.as_ref().unwrap(),
            &Checkpoint {
                state: 1,
                next_nodes: vec![],
                pending_interrupt: None,
            },
        )
        .await
        .unwrap();

        let loaded2: Option<Checkpoint<i32>> =
            Checkpointer::get(&saver, config2.thread_id.as_ref().unwrap())
                .await
                .unwrap();
        assert!(loaded2.is_none());
    }
}
