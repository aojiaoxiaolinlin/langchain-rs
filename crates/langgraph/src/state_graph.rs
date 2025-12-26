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
    ) -> Result<(S, Vec<InternedGraphLabel>), GraphError<E>> {
        let mut current_nodes = vec![self.entry];

        for _ in 0..max_steps {
            // 如果当前没有活跃节点，图执行结束
            if current_nodes.is_empty() {
                return Ok((state, current_nodes));
            }

            // 1. 并行执行当前步骤的所有活跃节点
            // 这是一个 "Super-step"：所有节点并行运行，然后统一同步
            let futures = current_nodes
                .iter()
                .map(|&node| self.graph.run_once(node, &state));

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
            if all_next_nodes.is_empty() {
                return Ok((state, current_nodes));
            }

            // 去重，防止同一节点被多次触发
            all_next_nodes.sort_unstable();
            all_next_nodes.dedup();

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
        max_steps: usize,
        strategy: RunStrategy,
    ) -> EventStream<'a, Ev> {
        use futures::StreamExt;

        let mut current_nodes = vec![self.entry];
        let graph = &self.graph;
        let reducer = &self.reducer;

        let stream = async_stream::stream! {
            for _ in 0..max_steps {
                if current_nodes.is_empty() {
                    break;
                }

                // 1. 并行启动所有节点的流
                // 我们需要同时消费多个流，这里稍微复杂一点
                // 使用 futures::stream::select_all 来合并所有节点的事件流

                let mut streams = Vec::new();
                for &node in &current_nodes {
                    match graph.run_stream(node, &state).await {
                        Ok(s) => streams.push(s),
                        Err(e) => {
                            tracing::error!("Error starting node stream {:?}: {:?}", node, e);
                            // 这里我们选择忽略启动失败的节点，或者应该直接中止整个图？
                            // 目前选择中止
                            return;
                        }
                    }
                }

                // 合并所有流
                let mut combined_stream = futures::stream::select_all(streams);

                let mut all_next_nodes = Vec::new();
                // 暂存 updates，确保在本轮所有事件处理完后统一 apply
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
                if all_next_nodes.is_empty() {
                    break;
                }

                all_next_nodes.sort_unstable();
                all_next_nodes.dedup();

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

    /// 执行直到卡住
    pub async fn run_until_stuck(
        &self,
        mut state: S,
        max_steps: usize,
    ) -> Result<(S, Vec<InternedGraphLabel>), GraphError<E>> {
        let mut current_nodes = vec![self.entry];

        for _ in 0..max_steps {
            // Parallel execution
            let futures = current_nodes
                .iter()
                .map(|&node| self.graph.run_once(node, &state));
            let results = join_all(futures).await;

            let mut all_next_nodes = Vec::new();
            for result in results {
                let (update, next) = result?;
                state = (self.reducer)(state, update);
                all_next_nodes.extend(next);
            }

            all_next_nodes.sort_unstable();
            all_next_nodes.dedup();

            if all_next_nodes.is_empty() {
                return Ok((state, current_nodes));
            }

            // 如果不是线性流（即产生了分支），run_until_stuck 的定义可能比较模糊
            // 这里我们假设它总是尽可能往下走，支持并行
            current_nodes = all_next_nodes;
        }

        Ok((state, current_nodes))
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

    /// 执行一步（使用 Sync 模式）
    pub async fn step(&mut self) -> Result<Vec<InternedGraphLabel>, GraphError<E>>
    where
        S: Send + Sync + Clone + 'static,
        U: Send + Sync + 'static,
        E: Send + Sync + 'static,
        Ev: Send + Sync + 'static,
    {
        if self.current_nodes.is_empty() {
            return Ok(Vec::new());
        }

        let futures = self
            .current_nodes
            .iter()
            .map(|&node| self.state_graph.graph.run_once(node, &self.state));

        let results = join_all(futures).await;

        let mut all_next_nodes = Vec::new();

        // 这里的 state 更新需要注意：
        // step() 可能会被多次调用，我们需要更新 self.state
        let mut new_state = self.state.clone();

        for result in results {
            let (update, next) = result?;
            new_state = (self.state_graph.reducer)(new_state, update);
            all_next_nodes.extend(next);
        }

        self.state = new_state;

        all_next_nodes.sort_unstable();
        all_next_nodes.dedup();

        if !all_next_nodes.is_empty() {
            self.current_nodes = all_next_nodes.clone();
        }

        Ok(all_next_nodes)
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

        let (final_state, final_nodes) = sg.run(0, 10, RunStrategy::PickFirst).await.unwrap();

        assert_eq!(final_state, 3);
        assert_eq!(final_nodes, vec![TestLabel::C.intern()]);
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

        let (final_state, final_nodes) = sg.run(0, 10, RunStrategy::PickFirst).await.unwrap();

        assert_eq!(final_state, 2);
        assert_eq!(final_nodes, vec![TestLabel::B.intern()]);
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

        let (final_state, final_nodes) = sg.run(-1, 10, RunStrategy::PickFirst).await.unwrap();

        assert_eq!(final_state, 0);
        assert_eq!(final_nodes, vec![TestLabel::A.intern()]);
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
        assert_eq!(runner.current_nodes, vec![TestLabel::B.intern()]);
        assert_eq!(next1, vec![TestLabel::B.intern()]);

        let next2 = runner.step().await.unwrap();
        assert_eq!(runner.state, 2);
        assert_eq!(runner.current_nodes, vec![TestLabel::C.intern()]);
        assert_eq!(next2, vec![TestLabel::C.intern()]);

        let next3 = runner.step().await.unwrap();
        assert_eq!(runner.state, 3);
        assert_eq!(runner.current_nodes, vec![TestLabel::C.intern()]);
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

        let (final_state, final_nodes) = sg.run(0, 10, RunStrategy::StopAtNonLinear).await.unwrap();

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

        let (final_state, final_nodes) = sg.run(0, 10, RunStrategy::PickFirst).await.unwrap();

        // B and C are candidates. PickFirst picks B (because B comes first in sort order or insertion order?)
        // HashMap iteration order is random. But we sort_unstable() before dedup.
        // Label B vs C. B < C. So B is first.
        assert_eq!(final_state, 2);
        assert_eq!(final_nodes, vec![TestLabel::B.intern()]);
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

        let (final_state, final_nodes) = sg.run(0, 10, RunStrategy::PickLast).await.unwrap();

        assert_eq!(final_state, 2);
        // Sorted: B, C. Last is C.
        assert_eq!(final_nodes, vec![TestLabel::C.intern()]);
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

        let (final_state, final_nodes) = sg.run(0, 10, RunStrategy::Parallel).await.unwrap();

        assert_eq!(final_state, 5);

        // Both B and C finished. They have no outgoing edges.
        // So all_next_nodes is empty.
        // The loop terminates. current_nodes is [B, C].
        let mut expected = vec![TestLabel::B.intern(), TestLabel::C.intern()];
        expected.sort();
        assert_eq!(final_nodes, expected);
    }

    #[tokio::test]
    async fn state_graph_parallel_multistep() {
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
            async fn run_sync(&self, _: &Vec<String>) -> Result<String, NodeError> {
                Ok(self.0.to_owned())
            }
            async fn run_stream(
                &self,
                _: &Vec<String>,
                _: &mut dyn EventSink<()>,
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

        let (final_state, final_nodes) =
            sg.run(Vec::new(), 10, RunStrategy::Parallel).await.unwrap();

        assert_eq!(final_state, vec!["A", "B", "C", "D", "E"]);

        let mut expected_nodes = vec![Label::D.intern(), Label::E.intern()];
        expected_nodes.sort();
        assert_eq!(final_nodes, expected_nodes);
    }
}
