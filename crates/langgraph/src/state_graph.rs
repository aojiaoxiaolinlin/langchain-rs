use crate::{
    checkpoint::{Checkpoint, Checkpointer, RunnableConfig},
    event::GraphEvent,
    graph::{Graph, GraphError},
    label::{GraphLabel, InternedGraphLabel},
    label_registry::register_label,
    node::{EventStream, Node, NodeContext},
};
use futures::future::join_all;
use langchain_core::store::BaseStore;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

/// Reducer 函数类型：接收当前状态和更新，返回新状态
/// (Current State, Update) -> New State
pub type Reducer<S, U> = Box<dyn Fn(S, U) -> S + Send + Sync>;

/// StateGraph 带 Ev 泛型
/// Ev = (): 不支持流式事件
/// Ev = ChatStreamEvent: 支持 ChatStreamEvent 事件
///
/// S: State (全局状态)
/// U: Update (节点返回的增量更新)
pub struct StateGraph<S, U, E, Ev: Debug = ()> {
    pub graph: Graph<S, U, E, Ev>,
    pub reducer: Reducer<S, U>,
    pub entry: InternedGraphLabel,
    pub checkpointer: Option<Arc<dyn Checkpointer<S>>>,
    pub store: Option<Arc<dyn BaseStore>>,
    pub interrupt_before: Vec<InternedGraphLabel>,
    pub interrupt_after: Vec<InternedGraphLabel>,
}

/// 运行策略枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStrategy {
    /// 遇到非线性分支时停止
    StopAtNonLinear,
    /// 选择第一个分支
    PickFirst,
    /// 选择最后一个分支
    PickLast,
    /// 并行执行
    Parallel,
}

impl<S, U, E: Debug, Ev: Debug> StateGraph<S, U, E, Ev> {
    /// 从入口节点创建 StateGraph
    /// 需要提供一个 reducer 函数来定义如何合并状态
    pub fn new(
        entry: impl GraphLabel,
        reducer: impl Fn(S, U) -> S + Send + Sync + 'static,
    ) -> Self {
        Self {
            graph: Graph {
                nodes: HashMap::new(),
            },
            reducer: Box::new(reducer),
            entry: entry.intern(),
            checkpointer: None,
            store: None,
            interrupt_before: Vec::new(),
            interrupt_after: Vec::new(),
        }
    }

    /// 设置入口节点
    pub fn set_entry(&mut self, entry: impl GraphLabel) {
        self.entry = entry.intern();
    }

    /// 设置检查点保存器
    pub fn with_checkpointer(mut self, checkpointer: impl Checkpointer<S> + 'static) -> Self {
        self.checkpointer = Some(Arc::new(checkpointer));
        self
    }

    /// 设置共享的检查点保存器
    pub fn with_shared_checkpointer(mut self, checkpointer: Arc<dyn Checkpointer<S>>) -> Self {
        self.checkpointer = Some(checkpointer);
        self
    }

    /// 设置 Store（用于跨节点数据共享）
    pub fn with_store(mut self, store: impl BaseStore + 'static) -> Self {
        self.store = Some(Arc::new(store));
        self
    }

    /// 设置共享的 Store
    pub fn with_shared_store(mut self, store: Arc<dyn BaseStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// 设置需要在执行前中断的节点
    pub fn with_interrupt_before(mut self, nodes: Vec<impl GraphLabel>) -> Self {
        self.interrupt_before = nodes.into_iter().map(|n| n.intern()).collect();
        self
    }

    /// 设置需要在执行后中断的节点
    pub fn with_interrupt_after(mut self, nodes: Vec<impl GraphLabel>) -> Self {
        self.interrupt_after = nodes.into_iter().map(|n| n.intern()).collect();
        self
    }

    /// 添加节点
    /// 节点的输入是 S (State)，输出是 U (Update)
    pub fn add_node<T>(&mut self, label: impl GraphLabel, node: T)
    where
        T: Node<S, U, E, Ev>,
    {
        let interned = label.intern();
        register_label(interned); // 全局注册以加速 checkpoint 恢复
        self.graph.add_node(label, node);
    }

    /// 添加边
    pub fn add_edge(&mut self, pred: impl GraphLabel, next: impl GraphLabel) {
        self.graph.add_node_edge(pred, next);
    }

    /// 添加条件边
    /// 条件函数的输入是 U (Update)，根据更新内容决定下一步去哪里
    pub fn add_condition_edge<F>(
        &mut self,
        pred: impl GraphLabel,
        branches: HashMap<InternedGraphLabel, InternedGraphLabel>,
        condition: F,
    ) where
        F: Fn(&U) -> Vec<InternedGraphLabel> + Send + Sync + 'static,
    {
        self.graph
            .add_node_condition_edge(pred, branches, condition);
    }
}

impl<S, U, E, Ev: Debug> StateGraph<S, U, E, Ev>
where
    S: Send + Sync + Clone + 'static + Serialize + DeserializeOwned,
    U: Send + Sync + 'static,
    E: Send + Sync + std::fmt::Debug + 'static,
    Ev: Send + Sync + 'static,
{
    /// 同步执行
    pub async fn run(
        &self,
        mut state: S,
        config: &RunnableConfig,
        max_steps: usize,
        strategy: RunStrategy,
    ) -> Result<(S, Vec<InternedGraphLabel>), GraphError<E>> {
        let mut current_nodes = vec![self.entry];

        // 尝试从 Checkpoint 恢复
        // if let Some(config) = config
        //     && let Some(checkpointer) = &self.checkpointer
        //     && let Ok(Some(checkpoint)) = checkpointer.get_state::<S>(config).await
        // {
        //     tracing::info!("Resuming from checkpoint: {:?}", config);
        //     state = checkpoint.state;
        //     // 如果 checkpoint 中有下一步节点，则从那里继续

        //     if !checkpoint.next_nodes.is_empty() {
        //         // 使用标签注册表进行快速查找（O(1) 而不是 O(n)）
        //         use crate::label_registry::str_to_label;

        //         let restored_nodes: Vec<_> = checkpoint
        //             .next_nodes
        //             .into_iter()
        //             .filter_map(|node_str| str_to_label(&node_str))
        //             .collect();

        //         if !restored_nodes.is_empty() {
        //             current_nodes = restored_nodes;
        //         } else {
        //             tracing::warn!("No nodes could be restored from checkpoint");
        //         }
        //     }
        // }

        for _ in 0..max_steps {
            // 如果当前没有活跃节点，图执行结束
            if current_nodes.is_empty() {
                // Save checkpoint at end
                if let Some(thread_id) = &config.thread_id
                    && let Some(checkpointer) = &self.checkpointer
                {
                    let checkpoint = Checkpoint {
                        state: state.clone(),
                        next_nodes: Vec::new(),
                        pending_interrupt: None,
                    };
                    if let Err(e) = checkpointer.put(thread_id, &checkpoint).await {
                        tracing::error!("Failed to save checkpoint: {:?}", e);
                    }
                }
                return Ok((state, current_nodes));
            }

            // check interrupt_before
            // If any of the current_nodes are in interrupt_before list, we stop.
            let should_interrupt = current_nodes
                .iter()
                .any(|n| self.interrupt_before.contains(n));
            if should_interrupt {
                tracing::info!("Interrupting before nodes: {:?}", current_nodes);
                if let Some(thread_id) = &config.thread_id
                    && let Some(checkpointer) = &self.checkpointer
                {
                    // Save checkpoint with pending interrupt
                    let next_node_strs = current_nodes
                        .iter()
                        .map(|n| n.as_str().to_owned())
                        .collect();
                    let checkpoint = Checkpoint {
                        state: state.clone(),
                        next_nodes: next_node_strs,
                        pending_interrupt: Some(crate::interrupt::Interrupt::confirmation(
                            "Interrupt Before",
                        )),
                    };
                    if let Err(e) = checkpointer.put(thread_id, &checkpoint).await {
                        tracing::error!("Failed to save interrupt checkpoint: {:?}", e);
                    }
                }
                return Ok((state, current_nodes));
            }

            // 1. 并行执行当前步骤的所有活跃节点
            // 这是一个 "Super-step"：所有节点并行运行，然后统一同步
            let futures = current_nodes.iter().map(|&node| {
                let context = NodeContext::new(self.store.clone(), config);
                self.graph.run_once(node, &state, context)
            });

            let results = join_all(futures).await;

            // 2. 收集结果并应用 Reducer
            // 注意：虽然执行是并行的，但 Reducer 的应用是顺序的（按节点顺序）
            // 这保证了确定性。如果用户需要特定的合并逻辑，应该在 reducer 内部处理。
            let mut all_next_nodes = Vec::new();

            for result in results {
                let (update, next) = result?;
                // Apply reducer: S' = reducer(S, U)
                state = (self.reducer)(state, update);
                all_next_nodes.extend(next);
            }

            // 3. 决定下一轮的活跃节点
            // 去重，防止同一节点被多次触发
            all_next_nodes.sort_unstable();
            all_next_nodes.dedup();

            if let Some(thread_id) = &config.thread_id
                && let Some(checkpointer) = &self.checkpointer
            {
                let next_node_strs = all_next_nodes
                    .iter()
                    .map(|n| n.as_str().to_owned())
                    .collect();
                let checkpoint = Checkpoint {
                    state: state.clone(),
                    next_nodes: next_node_strs,
                    pending_interrupt: None,
                };
                if let Err(e) = checkpointer.put(thread_id, &checkpoint).await {
                    tracing::error!("Failed to save checkpoint: {:?}", e);
                }
            }

            // check interrupt_after
            // Check if any of the EXECUTED nodes (current_nodes) are in interrupt_after
            let should_interrupt_after = current_nodes
                .iter()
                .any(|n| self.interrupt_after.contains(n));
            if should_interrupt_after {
                tracing::info!("Interrupting after nodes: {:?}", current_nodes);
                if let Some(thread_id) = &config.thread_id
                    && let Some(checkpointer) = &self.checkpointer
                {
                    // Save checkpoint with pending interrupt.
                    // The 'next_nodes' are already calculated in 'all_next_nodes'.
                    // We strictly want to pause BEFORE the next step.
                    let next_node_strs = all_next_nodes
                        .iter()
                        .map(|n| n.as_str().to_owned())
                        .collect();

                    let checkpoint = Checkpoint {
                        state: state.clone(),
                        next_nodes: next_node_strs,
                        pending_interrupt: Some(crate::interrupt::Interrupt::confirmation(
                            "Interrupt After",
                        )),
                    };
                    if let Err(e) = checkpointer.put(thread_id, &checkpoint).await {
                        tracing::error!("Failed to save interrupt checkpoint: {:?}", e);
                    }
                }
                // Return with the NEW state and NEXT nodes (so user knows where it's going)
                return Ok((state, all_next_nodes));
            }

            if all_next_nodes.is_empty() {
                return Ok((state, Vec::new()));
            }

            match strategy {
                RunStrategy::StopAtNonLinear => {
                    if all_next_nodes.len() > 1 {
                        return Ok((state, current_nodes));
                    }
                    current_nodes = all_next_nodes;
                }
                RunStrategy::PickFirst => {
                    current_nodes = vec![all_next_nodes[0]];
                }
                RunStrategy::PickLast => {
                    current_nodes = vec![all_next_nodes[all_next_nodes.len() - 1]];
                }
                RunStrategy::Parallel => {
                    // 在 Parallel 模式下，保留所有分支作为下一轮的活跃节点
                    current_nodes = all_next_nodes;
                }
            }
        }

        Ok((state, current_nodes))
    }

    pub fn stream<'a>(
        &'a self,
        mut state: S,
        config: &'a RunnableConfig,
        max_steps: usize,
        strategy: RunStrategy,
    ) -> EventStream<'a, Ev> {
        use futures::StreamExt;

        let mut current_nodes = vec![self.entry];
        let graph = &self.graph;
        let reducer = &self.reducer;
        let checkpointer = &self.checkpointer;
        let store = &self.store;

        let stream = async_stream::stream! {
            // 尝试恢复 (逻辑同 run)
            if let Some(thread_id) = &config.thread_id && let Some(checkpointer) = checkpointer {
                    // async block in stream is tricky, but here we are in async_stream! macro
                    // Note: get_state needs S: DeserializeOwned.
                    // S is bound in impl block.
                    if let Ok(Some(checkpoint)) = checkpointer.get(thread_id).await {
                        tracing::info!("Resuming from checkpoint: {:?}", thread_id);
                        state = checkpoint.state;
                        if !checkpoint.next_nodes.is_empty() {
                            // 使用标签注册表进行快速查找（O(1) 而不是 O(n)）
                            use crate::label_registry::str_to_label;

                            let restored_nodes: Vec<_> = checkpoint
                                .next_nodes
                                .into_iter()
                                .filter_map(|node_str| str_to_label(node_str.as_str()))
                                .collect();

                            if !restored_nodes.is_empty() {
                                current_nodes = restored_nodes;
                            }
                        }
                    }
                }

            for _ in 0..max_steps {
                if current_nodes.is_empty() {
                    // End of graph, save final state
                    if let Some(thread_id) = &config.thread_id && let Some(checkpointer) = checkpointer {
                            let checkpoint = Checkpoint {
                                state: state.clone(),
                                next_nodes: Vec::new(),
                                pending_interrupt: None,
                            };
                            let _ = checkpointer.put(thread_id, &checkpoint).await;
                        }
                    break;
                }

                // check interrupt_before
                let should_interrupt = current_nodes.iter().any(|n| self.interrupt_before.contains(n));
                if should_interrupt {
                    tracing::info!("Interrupting before nodes: {:?}", current_nodes);
                    if let Some(thread_id) = &config.thread_id && let Some(checkpointer) = checkpointer {
                        let next_node_strs = current_nodes.iter().map(|n| n.as_str().to_owned()).collect();
                        let checkpoint = Checkpoint {
                            state: state.clone(),
                            next_nodes: next_node_strs,
                            pending_interrupt: Some(crate::interrupt::Interrupt::confirmation("Interrupt Before")),
                        };
                        let _ = checkpointer.put(thread_id, &checkpoint).await;
                    }
                    break;
                }

                // 1. 并行启动所有节点的流
                // 我们需要同时消费多个流，这里稍微复杂一点
                // 使用 futures::stream::select_all 来合并所有节点的事件流

                let mut streams = Vec::new();
                for &node in &current_nodes {
                    let context = NodeContext::new(store.clone(), config);
                    match graph.run_stream(node, &state, context).await {
                        Ok(s) => streams.push(s),
                        Err(e) => {
                            tracing::error!("Error starting node stream {:?}: {:?}", node, e);
                            return;
                        }
                    }
                }

                // 合并所有流
                let mut combined_stream = futures::stream::select_all(streams);

                let mut all_next_nodes = Vec::new();
                let mut updates = Vec::new();

                while let Some(event_result) = combined_stream.next().await {
                    match event_result {
                        Ok(event) => match event {
                            GraphEvent::NodeEnd { output, next_nodes, .. } => {
                                updates.push(output);
                                all_next_nodes.extend(next_nodes);
                            }
                            GraphEvent::Streaming { event, .. } => {
                                yield event;
                            }
                            _ => {} // NodeStart 等忽略
                        },
                        Err(e) => {
                            tracing::error!("Error in node execution: {:?}", e);
                            return;
                        }
                    }
                }

                // 必须显式 drop combined_stream，因为它持有 state 的借用
                drop(combined_stream);

                // 2. 本轮结束，应用所有 updates
                for update in updates {
                    state = (reducer)(state, update);
                }

                // 3. 准备下一轮
                all_next_nodes.sort_unstable();
                all_next_nodes.dedup();

                // Save Checkpoint
                if let Some(thread_id) = &config.thread_id && let Some(checkpointer) = checkpointer {
                        let next_node_strs = all_next_nodes.iter().map(|n| n.as_str().to_owned()).collect();
                        let checkpoint = Checkpoint {
                            state: state.clone(),
                            next_nodes: next_node_strs,
                            pending_interrupt: None,
                        };
                        let _ = checkpointer.put(thread_id, &checkpoint).await;
                    }

                // check interrupt_after
                let should_interrupt_after = current_nodes.iter().any(|n| self.interrupt_after.contains(n));
                if should_interrupt_after {
                    tracing::info!("Interrupting after nodes: {:?}", current_nodes);
                    if let Some(thread_id) = &config.thread_id && let Some(checkpointer) = checkpointer {
                        let next_node_strs = all_next_nodes.iter().map(|n| n.as_str().to_owned()).collect();
                        let checkpoint = Checkpoint {
                            state: state.clone(),
                            next_nodes: next_node_strs,
                            pending_interrupt: Some(crate::interrupt::Interrupt::confirmation("Interrupt After")),
                        };
                        let _ = checkpointer.put(thread_id, &checkpoint).await;
                    }
                    break;
                }

                if all_next_nodes.is_empty() {
                    break;
                }

                match strategy {
                    RunStrategy::StopAtNonLinear => {
                        if all_next_nodes.len() > 1 {
                             break;
                        }
                        current_nodes = all_next_nodes;
                    }
                    RunStrategy::PickFirst => {
                        current_nodes = vec![all_next_nodes[0]];
                    }
                    RunStrategy::PickLast => {
                        current_nodes = vec![all_next_nodes[all_next_nodes.len() - 1]];
                    }
                    RunStrategy::Parallel => {
                        current_nodes = all_next_nodes;
                    }
                }
            }
        };

        EventStream::new(stream)
    }
}

/// StateGraph 运行器（用于逐步执行）
pub struct StateGraphRunner<'g, S, U, E, Ev: Debug> {
    pub state_graph: &'g StateGraph<S, U, E, Ev>,
    pub current_nodes: Vec<InternedGraphLabel>,
    pub state: S,
}

impl<'g, S, U, E: Debug, Ev: Debug> StateGraphRunner<'g, S, U, E, Ev> {
    /// 创建新的运行器
    pub fn new(state_graph: &'g StateGraph<S, U, E, Ev>, initial_state: S) -> Self {
        Self {
            state_graph,
            current_nodes: vec![state_graph.entry],
            state: initial_state,
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::label::GraphLabel;
    use crate::node::{EventSink, Node, NodeContext, NodeError};

    #[derive(Debug, Clone, PartialEq, Eq, Hash, GraphLabel)]
    enum TestLabel {
        A,
        B,
        C,
    }

    #[derive(Debug)]
    struct AddOne;

    #[async_trait]
    impl Node<i32, i32, NodeError, ()> for AddOne {
        async fn run_sync(&self, input: &i32, _context: NodeContext<'_>) -> Result<i32, NodeError> {
            Ok(*input + 1)
        }

        async fn run_stream(
            &self,
            input: &i32,
            _sink: &mut dyn EventSink<()>,
            _context: NodeContext<'_>,
        ) -> Result<i32, NodeError> {
            Ok(*input + 1)
        }
    }

    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, GraphLabel)]
    enum TestBranch {
        Default,
        #[expect(unused)]
        Alt,
    }

    #[tokio::test]
    async fn state_graph_runs_linear_chain() {
        let mut sg: StateGraph<i32, i32, NodeError, ()> =
            StateGraph::new(TestLabel::A, |_, update| update);

        sg.add_node(TestLabel::A, AddOne);
        sg.add_node(TestLabel::B, AddOne);
        sg.add_node(TestLabel::C, AddOne);

        sg.add_edge(TestLabel::A, TestLabel::B);
        sg.add_edge(TestLabel::B, TestLabel::C);
        let config = RunnableConfig::default();
        let (final_state, _) = sg
            .run(0, &config, 10, RunStrategy::PickFirst)
            .await
            .unwrap();

        assert_eq!(final_state, 3);
    }

    #[tokio::test]
    async fn state_graph_runs_conditional_branch_taken() {
        let mut sg: StateGraph<i32, i32, NodeError, ()> =
            StateGraph::new(TestLabel::A, |_, update| update);

        sg.add_node(TestLabel::A, AddOne);
        sg.add_node(TestLabel::B, AddOne);

        let mut branches = HashMap::new();
        branches.insert(TestBranch::Default.intern(), TestLabel::B.intern());

        sg.add_condition_edge(TestLabel::A, branches, |state: &i32| {
            if *state >= 0 {
                vec![TestBranch::Default.intern()]
            } else {
                Vec::new()
            }
        });

        let config = RunnableConfig::default();
        let (final_state, _) = sg
            .run(0, &config, 10, RunStrategy::PickFirst)
            .await
            .unwrap();

        assert_eq!(final_state, 2);
    }

    #[tokio::test]
    async fn state_graph_runs_conditional_branch_not_taken_stops_at_predicate() {
        let mut sg: StateGraph<i32, i32, NodeError, ()> =
            StateGraph::new(TestLabel::A, |_, update| update);

        sg.add_node(TestLabel::A, AddOne);

        let mut branches = HashMap::new();
        branches.insert(TestBranch::Default.intern(), TestLabel::B.intern());

        sg.add_condition_edge(TestLabel::A, branches, |state: &i32| {
            if *state > 0 {
                vec![TestBranch::Default.intern()]
            } else {
                Vec::new()
            }
        });
        let config = RunnableConfig::default();
        let (final_state, _) = sg
            .run(-1, &config, 10, RunStrategy::PickFirst)
            .await
            .unwrap();

        assert_eq!(final_state, 0);
    }

    #[tokio::test]
    async fn state_graph_run_strategy_stop_at_non_linear() {
        let mut sg: StateGraph<i32, i32, NodeError, ()> =
            StateGraph::new(TestLabel::A, |_, update| update);

        sg.add_node(TestLabel::A, AddOne);
        sg.add_node(TestLabel::B, AddOne);
        sg.add_node(TestLabel::C, AddOne);

        sg.add_edge(TestLabel::A, TestLabel::B);
        sg.add_edge(TestLabel::A, TestLabel::C);

        let config = RunnableConfig::default();
        let (final_state, final_nodes) = sg
            .run(0, &config, 10, RunStrategy::StopAtNonLinear)
            .await
            .unwrap();

        assert_eq!(final_state, 1);
        // Returns [B, C] but since we stop, it returns the next nodes that caused the stop?
        // Wait, logic says: if all_next_nodes.len() > 1 { return Ok((state, current_nodes)); }
        // So it returns the *previous* current_nodes (which is A)
        assert_eq!(final_nodes, vec![TestLabel::A.intern()]);
    }

    #[tokio::test]
    async fn state_graph_run_strategy_pick_first() {
        let mut sg: StateGraph<i32, i32, NodeError, ()> =
            StateGraph::new(TestLabel::A, |_, update| update);

        sg.add_node(TestLabel::A, AddOne);
        sg.add_node(TestLabel::B, AddOne);
        sg.add_node(TestLabel::C, AddOne);

        sg.add_edge(TestLabel::A, TestLabel::B);
        sg.add_edge(TestLabel::A, TestLabel::C);

        let config = RunnableConfig::default();

        let (final_state, _) = sg
            .run(0, &config, 10, RunStrategy::PickFirst)
            .await
            .unwrap();

        // B and C are candidates. PickFirst picks B (because B comes first in sort order or insertion order?)
        // HashMap iteration order is random. But we sort_unstable() before dedup.
        // Label B vs C. B < C. So B is first.
        assert_eq!(final_state, 2);
    }

    #[tokio::test]
    async fn state_graph_run_strategy_pick_last() {
        let mut sg: StateGraph<i32, i32, NodeError, ()> =
            StateGraph::new(TestLabel::A, |_, update| update);

        sg.add_node(TestLabel::A, AddOne);
        sg.add_node(TestLabel::B, AddOne);
        sg.add_node(TestLabel::C, AddOne);

        sg.add_edge(TestLabel::A, TestLabel::B);
        sg.add_edge(TestLabel::A, TestLabel::C);

        let config = RunnableConfig::default();
        let (final_state, _) = sg.run(0, &config, 10, RunStrategy::PickLast).await.unwrap();

        assert_eq!(final_state, 2);
        // Sorted: B, C. Last is C.
    }

    #[tokio::test]
    async fn state_graph_works_with_non_error_type() {
        #[derive(Debug)]
        struct StringNode;
        #[async_trait]
        impl Node<i32, String, String, ()> for StringNode {
            async fn run_sync(
                &self,
                input: &i32,
                _context: NodeContext<'_>,
            ) -> Result<String, String> {
                Ok(format!("{}", input + 1))
            }
            async fn run_stream(
                &self,
                input: &i32,
                _sink: &mut dyn EventSink<()>,
                _context: NodeContext<'_>,
            ) -> Result<String, String> {
                Ok(format!("{}", input + 1))
            }
        }

        let mut sg: StateGraph<i32, String, String, ()> =
            StateGraph::new(TestLabel::A, |_, update: String| {
                update.parse::<i32>().unwrap()
            });
        sg.add_node(TestLabel::A, StringNode);
        let config = RunnableConfig::default();
        let (final_state, _) = sg.run(0, &config, 1, RunStrategy::PickFirst).await.unwrap();
        assert_eq!(final_state, 1);
    }

    #[tokio::test]
    async fn state_graph_run_strategy_parallel() {
        let mut sg: StateGraph<i32, i32, NodeError, ()> =
            StateGraph::new(TestLabel::A, |state, update| state + update);

        sg.add_node(TestLabel::A, AddOne);
        sg.add_node(TestLabel::B, AddOne);
        sg.add_node(TestLabel::C, AddOne);

        // A -> B
        // A -> C
        sg.add_edge(TestLabel::A, TestLabel::B);
        sg.add_edge(TestLabel::A, TestLabel::C);

        // A(0) -> updates +1 -> state 1
        // Branches B and C run in parallel with state 1
        // B(1) -> updates +1
        // C(1) -> updates +1
        // Total state: 1 (from A) + 1 (from B) + 1 (from C) = 3
        // BUT:
        // reducer is state + update.
        // Step 1: A runs. input 0. returns 1. reducer(0, 1) -> 1.
        // Step 2: B and C run in parallel with input 1.
        // B returns 2. C returns 2.
        // Parallel strategy applies reducer sequentially for updates.
        // reducer(1, 2) -> 3.
        // reducer(3, 2) -> 5.
        let config = RunnableConfig::default();
        let (final_state, _) = sg.run(0, &config, 10, RunStrategy::Parallel).await.unwrap();

        assert_eq!(final_state, 5);
    }

    #[tokio::test]
    async fn state_graph_parallel_multi_step() {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, GraphLabel)]
        enum Label {
            A,
            B,
            C,
            D,
            E,
        }

        let mut sg: StateGraph<Vec<String>, String, NodeError, ()> =
            StateGraph::new(Label::A, |mut state: Vec<String>, update: String| {
                state.push(update);
                state.sort(); // Sort to make deterministic comparison easy
                state
            });

        #[derive(Debug)]
        struct NameNode(&'static str);
        #[async_trait]
        impl Node<Vec<String>, String, NodeError, ()> for NameNode {
            async fn run_sync(
                &self,
                _: &Vec<String>,
                _context: NodeContext<'_>,
            ) -> Result<String, NodeError> {
                Ok(self.0.to_owned())
            }
            async fn run_stream(
                &self,
                _: &Vec<String>,
                _: &mut dyn EventSink<()>,
                _context: NodeContext<'_>,
            ) -> Result<String, NodeError> {
                Ok(self.0.to_owned())
            }
        }

        sg.add_node(Label::A, NameNode("A"));
        sg.add_node(Label::B, NameNode("B"));
        sg.add_node(Label::C, NameNode("C"));
        sg.add_node(Label::D, NameNode("D"));
        sg.add_node(Label::E, NameNode("E"));

        // A -> B, A -> C
        sg.add_edge(Label::A, Label::B);
        sg.add_edge(Label::A, Label::C);

        // B -> D, C -> E
        sg.add_edge(Label::B, Label::D);
        sg.add_edge(Label::C, Label::E);

        // Execution flow:
        // Step 1: A runs. Output "A". State ["A"]. Next [B, C].
        // Step 2: B, C run. Output "B", "C". State ["A", "B", "C"]. Next [D, E].
        // Step 3: D, E run. Output "D", "E". State ["A", "B", "C", "D", "E"]. Next [].
        let config = RunnableConfig::default();
        let (final_state, _) = sg
            .run(Vec::new(), &config, 10, RunStrategy::Parallel)
            .await
            .unwrap();

        assert_eq!(final_state, vec!["A", "B", "C", "D", "E"]);
    }
}
