use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use downcast_rs::{Downcast, impl_downcast};
use futures::Stream;
use langchain_core::store::BaseStore;
use thiserror::Error;

use crate::{checkpoint::RunnableConfig, edge::Edge, label::InternedGraphLabel};

/// 节点执行上下文
pub struct NodeContext<'a> {
    /// 共享存储
    pub store: Option<Arc<dyn BaseStore>>,
    /// 运行时配置
    pub config: &'a RunnableConfig,
}

impl<'a> NodeContext<'a> {
    pub fn from_config(config: &'a RunnableConfig) -> Self {
        Self {
            store: None,
            config,
        }
    }
    pub fn new(store: Option<Arc<dyn BaseStore>>, config: &'a RunnableConfig) -> Self {
        Self { store, config }
    }
}

#[derive(Debug, Error)]
pub enum NodeError {}

#[async_trait]
pub trait EventSink<Ev>: Send + Sync {
    async fn emit(&self, event: Ev);
}

/// 事件流类型
pub struct EventStream<'a, Ev> {
    inner: Pin<Box<dyn Stream<Item = Ev> + Send + 'a>>,
}

impl<'a, Ev> EventStream<'a, Ev> {
    pub fn new(stream: impl Stream<Item = Ev> + Send + 'a) -> Self {
        Self {
            inner: Box::pin(stream),
        }
    }
}

impl<'a, Ev> Stream for EventStream<'a, Ev> {
    type Item = Ev;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

/// 节点 trait，定义了节点的运行行为
///
/// # 类型参数
///
/// * `I` - 节点输入类型
/// * `O` - 节点输出类型
/// * `E` - 节点错误类型
/// * `Ev` - 事件类型
#[async_trait]
pub trait Node<I, O, E, Ev>: Downcast + Send + Sync + 'static {
    async fn run_sync(&self, input: &I, context: NodeContext<'_>) -> Result<O, E>;

    async fn run_stream(
        &self,
        input: &I,
        sink: &dyn EventSink<Ev>,
        context: NodeContext<'_>,
    ) -> Result<O, E>;
}

impl_downcast!(Node<I, O, E, Ev>);

/// 节点状态结构体，包含节点的标签、类型名称等元数据和节点实例
///
/// # 类型参数
///
/// * `I` - 节点输入类型
/// * `O` - 节点输出类型
/// * `E` - 节点错误类型
/// * `Ev` - 事件类型
pub struct NodeState<I, O, E, Ev: Debug> {
    pub label: InternedGraphLabel,
    /// 节点类型名称，用于调试和日志记录
    pub type_name: &'static str,
    pub node: Box<dyn Node<I, O, E, Ev>>,
    pub edges: Vec<Edge<O>>,
}

impl<I, O, E, Ev: Debug> NodeState<I, O, E, Ev> {
    pub fn new<T>(label: InternedGraphLabel, node: T) -> Self
    where
        T: Node<I, O, E, Ev>,
    {
        Self {
            label,
            type_name: core::any::type_name::<T>(),
            node: Box::new(node),
            edges: Vec::new(),
        }
    }
}

impl<I, O, E, Ev: Debug> Debug for NodeState<I, O, E, Ev> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{:?} ({})", self.label, self.type_name)
    }
}
