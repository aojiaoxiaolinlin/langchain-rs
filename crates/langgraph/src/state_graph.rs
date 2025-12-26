use crate::{
    event::GraphEvent,
    graph::{Graph, GraphError},
    label::{GraphLabel, InternedGraphLabel},
    node::{EventStream, Node},
};
use futures::future::join_all;
use std::collections::HashMap;
use std::fmt::Debug;

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
    /// 并行执行（当前简化为选择第一个）
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
        }
    }

    /// 设置入口节点
    pub fn set_entry(&mut self, entry: impl GraphLabel) {
        self.entry = entry.intern();
    }

    /// 添加节点
    /// 节点的输入是 S (State)，输出是 U (Update)
    pub fn add_node<T>(&mut self, label: impl GraphLabel, node: T)
    where
        T: Node<S, U, E, Ev>,
    {
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
    S: Send + Sync + Clone + 'static,
    U: Send + Sync + 'static,
    E: Send + Sync + std::fmt::Debug + 'static,
    Ev: Send + Sync + 'static,
{
    /// 同步执行
    pub async fn run(
        &self,
        mut state: S,
        max_steps: usize,
        strategy: RunStrategy,
    ) -> Result<(S, InternedGraphLabel), GraphError<E>> {
        let mut current = self.entry;

        for _ in 0..max_steps {
            // 运行节点，得到 update
            let (update, next) = self.graph.run_once(current, &state).await?;

            // 使用 reducer 合并状态
            state = (self.reducer)(state, update);

            match next.len() {
                0 => return Ok((state, current)),
                1 => current = next[0],
                _ => match strategy {
                    RunStrategy::StopAtNonLinear => return Ok((state, current)),
                    RunStrategy::PickFirst => current = next[0],
                    RunStrategy::PickLast => current = next[next.len() - 1],
                    RunStrategy::Parallel => {
                        // Parallel execution logic
                        let next_nodes = next;
                        let mut futures = Vec::new();

                        // Execute all branches in parallel
                        for &node in &next_nodes {
                            futures.push(self.graph.run_once(node, &state));
                        }

                        let results = join_all(futures).await;

                        // Collect updates and determine next nodes
                        let mut all_next_nodes = Vec::new();
                        for result in results {
                            let (update, next) = result?;
                            state = (self.reducer)(state, update);
                            all_next_nodes.extend(next);
                        }

                        // For simplicity, if multiple branches converge or produce multiple next nodes,
                        // we need a strategy to handle the next step.
                        // Here we just take the first one if available to continue the loop.
                        // In a real graph execution engine, this would need a more complex scheduler.
                        if all_next_nodes.is_empty() {
                            return Ok((state, current));
                        }

                        // Unique next nodes to avoid redundant execution if branches converge
                        all_next_nodes.sort_unstable();
                        all_next_nodes.dedup();

                        if all_next_nodes.is_empty() {
                            return Ok((state, current));
                        }

                        // If we still have multiple next nodes after parallel execution,
                        // we continue with the first one for now as the loop structure assumes single current pointer.
                        // Ideally StateGraph should support maintaining a set of current nodes.
                        // TODO: 以后有必要再实现维护多个当前节点的功能
                        current = all_next_nodes[0];
                    }
                },
            }
        }

        Ok((state, current))
    }

    pub fn stream<'a>(
        &'a self,
        mut state: S,
        max_steps: usize,
        strategy: RunStrategy,
    ) -> EventStream<'a, Ev> {
        use futures::StreamExt;

        let mut current = self.entry;
        let graph = &self.graph;
        let reducer = &self.reducer;

        let stream = async_stream::stream! {
            for _ in 0..max_steps {
                let mut node_stream = match graph.run_stream(current, &state).await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("Error in node stream: {:?}", e);
                        break;
                    }
                };

                let mut next = Vec::new();
                let mut update_output = None;

                while let Some(event_result) = node_stream.next().await {
                    match event_result {
                        Ok(event) => match event {
                            GraphEvent::NodeEnd {
                                output, next_nodes, ..
                            } => {
                                update_output = Some(output);
                                next = next_nodes;
                                break;
                            }
                            GraphEvent::Streaming { event, .. } => {
                                tracing::debug!("Streaming event: {:?}", event);
                                yield event;
                            }
                            _ => {}
                        },
                        Err(e) => {
                            tracing::error!("Error in node execution: {:?}", e);
                            break;
                        }
                    }
                }
                drop(node_stream);

                if let Some(update) = update_output {
                    // 使用 reducer 合并状态
                    state = (reducer)(state, update);
                }

                match next.len() {
                    0 => {
                        tracing::debug!("No next nodes, graph completed");
                        break;
                    }
                    1 => {
                        current = next[0];
                    }
                    _ => {
                        match strategy {
                            RunStrategy::StopAtNonLinear => {
                                tracing::debug!("Non-linear branch, stopping");
                                break;
                            }
                            RunStrategy::PickFirst => {
                                current = next[0];
                            }
                            RunStrategy::PickLast => {
                                current = next[next.len() - 1];
                            }
                            RunStrategy::Parallel => {
                                current = next[0];
                            }
                        }
                    }
                }
            }
        };

        EventStream::new(stream)
    }

    /// 执行直到卡住
    pub async fn run_until_stuck(
        &self,
        mut state: S,
        max_steps: usize,
    ) -> Result<(S, InternedGraphLabel), GraphError<E>> {
        let mut current = self.entry;

        for _ in 0..max_steps {
            let (update, next) = self.graph.run_once(current, &state).await?;
            state = (self.reducer)(state, update);

            if next.len() != 1 {
                return Ok((state, current));
            }

            current = next[0];
        }

        Ok((state, current))
    }
}

/// StateGraph 运行器（用于逐步执行）
pub struct StateGraphRunner<'g, S, U, E, Ev: Debug> {
    pub state_graph: &'g StateGraph<S, U, E, Ev>,
    pub current: InternedGraphLabel,
    pub state: S,
}

impl<'g, S, U, E: Debug, Ev: Debug> StateGraphRunner<'g, S, U, E, Ev> {
    /// 创建新的运行器
    pub fn new(state_graph: &'g StateGraph<S, U, E, Ev>, initial_state: S) -> Self {
        Self {
            state_graph,
            current: state_graph.entry,
            state: initial_state,
        }
    }

    /// 执行一步（使用 Sync 模式）
    pub async fn step(&mut self) -> Result<Vec<InternedGraphLabel>, GraphError<E>>
    where
        S: Send + Sync + Clone + 'static,
        U: Send + Sync + 'static,
        E: Send + Sync + 'static,
        Ev: Send + Sync + 'static,
    {
        let (update, next) = self
            .state_graph
            .graph
            .run_once(self.current, &self.state)
            .await?;

        self.state = (self.state_graph.reducer)(self.state.clone(), update);

        if next.len() == 1 {
            self.current = next[0];
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::label::GraphLabel;
    use crate::node::{EventSink, Node, NodeError};

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
        async fn run_sync(&self, input: &i32) -> Result<i32, NodeError> {
            Ok(*input + 1)
        }

        async fn run_stream(
            &self,
            input: &i32,
            _sink: &mut dyn EventSink<()>,
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

        let (final_state, final_label) = sg.run(0, 10, RunStrategy::PickFirst).await.unwrap();

        assert_eq!(final_state, 3);
        assert_eq!(final_label, TestLabel::C.intern());
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

        let (final_state, final_label) = sg.run(0, 10, RunStrategy::PickFirst).await.unwrap();

        assert_eq!(final_state, 2);
        assert_eq!(final_label, TestLabel::B.intern());
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

        let (final_state, final_label) = sg.run(-1, 10, RunStrategy::PickFirst).await.unwrap();

        assert_eq!(final_state, 0);
        assert_eq!(final_label, TestLabel::A.intern());
    }

    #[tokio::test]
    async fn state_graph_runner_steps_through_linear_chain() {
        let mut sg: StateGraph<i32, i32, NodeError, ()> =
            StateGraph::new(TestLabel::A, |_, update| update);

        sg.add_node(TestLabel::A, AddOne);
        sg.add_node(TestLabel::B, AddOne);
        sg.add_node(TestLabel::C, AddOne);

        sg.add_edge(TestLabel::A, TestLabel::B);
        sg.add_edge(TestLabel::B, TestLabel::C);

        let mut runner = StateGraphRunner::new(&sg, 0);

        let next1 = runner.step().await.unwrap();
        assert_eq!(runner.state, 1);
        assert_eq!(runner.current, TestLabel::B.intern());
        assert_eq!(next1, vec![TestLabel::B.intern()]);

        let next2 = runner.step().await.unwrap();
        assert_eq!(runner.state, 2);
        assert_eq!(runner.current, TestLabel::C.intern());
        assert_eq!(next2, vec![TestLabel::C.intern()]);

        let next3 = runner.step().await.unwrap();
        assert_eq!(runner.state, 3);
        assert_eq!(runner.current, TestLabel::C.intern());
        assert!(next3.is_empty());
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

        let (final_state, final_label) = sg.run(0, 10, RunStrategy::StopAtNonLinear).await.unwrap();

        assert_eq!(final_state, 1);
        assert_eq!(final_label, TestLabel::A.intern());
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

        let (final_state, final_label) = sg.run(0, 10, RunStrategy::PickFirst).await.unwrap();

        assert_eq!(final_state, 2);
        assert_eq!(final_label, TestLabel::B.intern());
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

        let (final_state, final_label) = sg.run(0, 10, RunStrategy::PickLast).await.unwrap();

        assert_eq!(final_state, 2);
        assert_eq!(final_label, TestLabel::C.intern());
    }

    #[tokio::test]
    async fn state_graph_works_with_non_error_type() {
        #[derive(Debug)]
        struct StringNode;
        #[async_trait]
        impl Node<i32, String, String, ()> for StringNode {
            async fn run_sync(&self, input: &i32) -> Result<String, String> {
                Ok(format!("{}", input + 1))
            }
            async fn run_stream(
                &self,
                input: &i32,
                _sink: &mut dyn EventSink<()>,
            ) -> Result<String, String> {
                Ok(format!("{}", input + 1))
            }
        }

        let mut sg: StateGraph<i32, String, String, ()> =
            StateGraph::new(TestLabel::A, |_, update: String| {
                update.parse::<i32>().unwrap()
            });
        sg.add_node(TestLabel::A, StringNode);
        let (final_state, _) = sg.run(0, 1, RunStrategy::PickFirst).await.unwrap();
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

        let (final_state, _) = sg.run(0, 10, RunStrategy::Parallel).await.unwrap();

        assert_eq!(final_state, 5);
    }
}
