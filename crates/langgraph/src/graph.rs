use async_stream::stream;
use smallvec::SmallVec;
use std::collections::HashMap;
use std::fmt::Debug;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::{
    edge::{Edge, EdgeCondition},
    event::GraphEvent,
    label::{GraphLabel, InternedGraphLabel, IntoGraphNodeArray},
    node::{EventStream, Node, NodeContext, NodeState},
};

#[derive(Default)]
pub struct Graph<I, O, E, Ev: std::fmt::Debug> {
    pub nodes: HashMap<InternedGraphLabel, NodeState<I, O, E, Ev>>,
}

impl<I, O, E: std::fmt::Debug, Ev: std::fmt::Debug> Graph<I, O, E, Ev> {
    /// 添加一个节点到图中
    pub fn add_node<T>(&mut self, label: impl GraphLabel, node: T)
    where
        T: Node<I, O, E, Ev>,
    {
        let label = label.intern();
        let node_state = NodeState::new(label, node);
        self.nodes.insert(label, node_state);
    }

    pub fn get_node_state_mut(
        &mut self,
        label: impl GraphLabel,
    ) -> Result<&mut NodeState<I, O, E, Ev>, GraphError<E>> {
        let label = label.intern();
        self.nodes
            .get_mut(&label)
            .ok_or(GraphError::InvalidNode(label))
    }

    pub fn try_add_node_edge(
        &mut self,
        pred_node: impl GraphLabel,
        next_node: impl GraphLabel,
    ) -> Result<(), GraphError<E>> {
        let pred_node = pred_node.intern();
        let next_node = next_node.intern();

        let pred_node_state = self.get_node_state_mut(pred_node)?;

        let exists = pred_node_state
            .edges
            .iter()
            .any(|e| matches!(e, Edge::NodeEdge(n) if *n == next_node));
        if exists {
            return Err(GraphError::EdgeAlreadyExists(next_node));
        }

        let edge = Edge::NodeEdge(next_node);

        pred_node_state.edges.push(edge);
        Ok(())
    }

    /// 添加一个条件边到图中，保证 `pred_node` 是 `next_node` 的前继
    ///
    /// # Arguments
    ///
    /// * `pred_node` - 前继节点的标签
    /// * `branches` - 一个映射，将条件分支子集映射的到后继节点的标签
    /// * `condition` - 一个函数，输入为边的输出，输出为后继节点标签列表
    pub fn try_add_node_condition_edge<F>(
        &mut self,
        pred_node: impl GraphLabel,
        branches: HashMap<InternedGraphLabel, InternedGraphLabel>,
        condition: F,
    ) -> Result<(), GraphError<E>>
    where
        F: Fn(&O) -> SmallVec<[InternedGraphLabel; 2]> + Send + Sync + 'static,
    {
        let branch_keys: std::collections::HashSet<_> = branches.keys().copied().collect();

        // 以后有必要再为每个条件分支设计单独的分支类型，这样做next_nodes的Key就得使用Box<dyn BranchKind>，目前没得必要
        // 检查 condition 是否返回的分支都在 branches 中
        let wrapped: EdgeCondition<O> = Box::new(move |o: &O| {
            let result: SmallVec<[_; 2]> = condition(o);
            assert!(
                result.iter().all(|b| branch_keys.contains(b)),
                "Edge::conditional: condition returned branch not in branches map"
            );
            result
        });

        let pred_node = pred_node.intern();

        let pred_node_state = self.get_node_state_mut(pred_node)?;

        let next_nodes = branches.into_iter().collect();

        let edge = Edge::ConditionalEdge {
            next_nodes,
            condition: wrapped,
        };

        pred_node_state.edges.push(edge);
        Ok(())
    }

    /// 添加一个边到图中，保证 `pred_node` 是 `next_node` 的前继
    pub fn add_node_edge(&mut self, pred_node: impl GraphLabel, next_node: impl GraphLabel) {
        self.try_add_node_edge(pred_node, next_node).unwrap();
    }

    pub fn add_node_edges<const N: usize>(&mut self, edges: impl IntoGraphNodeArray<N>) {
        for window in edges.into_array().windows(2) {
            let [pred_node, next_node] = window else {
                break;
            };
            self.add_node_edge(*pred_node, *next_node);
        }
    }

    pub fn add_node_condition_edge<F>(
        &mut self,
        pred_node: impl GraphLabel,
        branches: HashMap<InternedGraphLabel, InternedGraphLabel>,
        condition: F,
    ) where
        F: Fn(&O) -> SmallVec<[InternedGraphLabel; 2]> + Send + Sync + 'static,
    {
        self.try_add_node_condition_edge(pred_node, branches, condition)
            .unwrap();
    }

    pub async fn run_once(
        &self,
        current: InternedGraphLabel,
        input: &I,
        context: NodeContext<'_>,
    ) -> Result<(O, Vec<InternedGraphLabel>), GraphError<E>>
    where
        I: Send + Sync + 'static,
        O: Send + Sync + 'static,
        E: Send + Sync + 'static,
        Ev: Send + Sync + 'static,
    {
        let state = self
            .nodes
            .get(&current)
            .ok_or_else(|| GraphError::InvalidNode(current))?;

        let output = state
            .node
            .run_sync(input, context)
            .await
            .map_err(GraphError::NodeRunError)?;

        let next_nodes = self.get_next_nodes(state, &output);

        Ok((output, next_nodes))
    }

    pub async fn run_stream<'a>(
        &'a self,
        current: InternedGraphLabel,
        input: &'a I,
        context: NodeContext<'a>,
    ) -> Result<EventStream<'a, Result<GraphEvent<Ev, O>, GraphError<E>>>, GraphError<E>>
    where
        I: Send + Sync + 'static,
        O: Send + Sync + 'static,
        E: Send + Sync + 'static,
        Ev: Send + 'static,
    {
        let state = self
            .nodes
            .get(&current)
            .ok_or_else(|| GraphError::InvalidNode(current))?;

        let label = state.label;

        struct ChannelSink<Ev> {
            tx: mpsc::Sender<Ev>,
        }

        #[async_trait::async_trait]
        impl<Ev: Send> crate::node::EventSink<Ev> for ChannelSink<Ev> {
            async fn emit(&self, event: Ev) {
                let _ = self.tx.send(event).await;
            }
        }

        let stream = stream! {
            yield Ok(GraphEvent::node_start(label));

            let (tx, mut rx) = mpsc::channel(100);
            let sink = ChannelSink { tx };

            let mut run_future = state.node.run_stream(input, &sink, context);

            let output_result;

            loop {
                tokio::select! {
                    result = &mut run_future => {
                        output_result = Some(result);
                        break;
                    }
                    Some(ev) = rx.recv() => {
                        yield Ok(GraphEvent::streaming(label, ev));
                    }
                }
            }

            // Drop future and sink to close the channel
            drop(run_future);
            drop(sink);

            // Drain remaining events
            while let Some(ev) = rx.recv().await {
                yield Ok(GraphEvent::streaming(label, ev));
            }

            if let Some(result) = output_result {
                match result {
                    Ok(output) => {
                        tracing::debug!("node {:?} output", label);
                        let next_nodes = self.get_next_nodes(state, &output);
                        yield Ok(GraphEvent::node_end(label, output, next_nodes));
                    }
                    Err(e) => {
                        tracing::error!("node {:?} error", label);
                        yield Err(GraphError::NodeRunError(e));
                    }
                }
            }
        };

        Ok(EventStream::new(stream))
    }

    fn get_next_nodes(
        &self,
        state: &NodeState<I, O, E, Ev>,
        output: &O,
    ) -> Vec<InternedGraphLabel> {
        let mut next_nodes = Vec::new();
        for edge in &state.edges {
            match edge {
                Edge::NodeEdge(label) => next_nodes.push(*label),
                Edge::ConditionalEdge {
                    next_nodes: branches,
                    condition,
                } => {
                    let branches_to_take = (condition)(output);
                    for branch in branches_to_take {
                        // 在 Vec 中查找对应的分支
                        if let Some((_, label)) = branches.iter().find(|(b, _)| *b == branch) {
                            next_nodes.push(*label);
                        }
                    }
                }
            }
        }
        next_nodes
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphError<E> {
    /// 无效的节点标签
    #[error("node {0:?} does not exist")]
    InvalidNode(InternedGraphLabel),

    /// 边已存在
    #[error("edge {0:?} already exists")]
    EdgeAlreadyExists(InternedGraphLabel),

    /// Node run Error
    #[error("node {0:?} run error")]
    NodeRunError(E),

    /// 事件不存在
    #[error("no event")]
    NoEvent,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::Configuration;
    use crate::label::GraphLabel;
    use crate::node::{EventSink, Node, NodeContext, NodeError};
    use async_trait::async_trait;

    #[derive(Debug, Clone, PartialEq, Eq, Hash, GraphLabel)]
    enum TestLabel {
        A,
        B,
        C,
    }

    #[derive(Debug)]
    struct IncNode;

    #[async_trait]
    impl Node<i32, i32, NodeError, ()> for IncNode {
        async fn run_sync(&self, input: &i32, _context: NodeContext<'_>) -> Result<i32, NodeError> {
            Ok(*input + 1)
        }

        async fn run_stream(
            &self,
            input: &i32,
            _sink: &dyn EventSink<()>,
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

    #[derive(Debug)]
    struct StreamNode;

    #[async_trait]
    impl Node<i32, i32, NodeError, i32> for StreamNode {
        async fn run_sync(&self, input: &i32, _context: NodeContext<'_>) -> Result<i32, NodeError> {
            Ok(*input + 1)
        }

        async fn run_stream(
            &self,
            input: &i32,
            sink: &dyn EventSink<i32>,
            _context: NodeContext<'_>,
        ) -> Result<i32, NodeError> {
            let value = *input + 1;
            sink.emit(1).await;
            sink.emit(2).await;
            sink.emit(3).await;
            Ok(value)
        }
    }

    #[test]
    fn add_node_and_edge_should_link_successor() {
        let mut graph: Graph<i32, i32, NodeError, ()> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, IncNode);
        graph.add_node(TestLabel::B, IncNode);

        graph.add_node_edge(TestLabel::A, TestLabel::B);

        let a_label = TestLabel::A.intern();
        let b_label = TestLabel::B.intern();

        let node_state = graph.nodes.get(&a_label).expect("node A must exist");
        assert_eq!(node_state.edges.len(), 1);

        match node_state.edges[0] {
            Edge::NodeEdge(next) => assert_eq!(next, b_label),
            _ => panic!("expected NodeEdge"),
        }
    }

    #[test]
    fn add_node_edges_should_link_all_successors_in_chain() {
        let mut graph: Graph<i32, i32, NodeError, ()> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, IncNode);
        graph.add_node(TestLabel::B, IncNode);
        graph.add_node(TestLabel::C, IncNode);

        graph.add_node_edges((TestLabel::A, TestLabel::B, TestLabel::C));

        let a_label = TestLabel::A.intern();
        let b_label = TestLabel::B.intern();
        let c_label = TestLabel::C.intern();

        let a_state = graph.nodes.get(&a_label).unwrap();
        let b_state = graph.nodes.get(&b_label).unwrap();
        let c_state = graph.nodes.get(&c_label).unwrap();

        assert_eq!(a_state.edges.len(), 1);
        assert_eq!(b_state.edges.len(), 1);
        assert_eq!(c_state.edges.len(), 0);

        match a_state.edges[0] {
            Edge::NodeEdge(next) => assert_eq!(next, b_label),
            _ => panic!("expected NodeEdge from A to B"),
        }

        match b_state.edges[0] {
            Edge::NodeEdge(next) => assert_eq!(next, c_label),
            _ => panic!("expected NodeEdge from B to C"),
        }
    }

    #[test]
    fn duplicate_edge_returns_error() {
        let mut graph: Graph<i32, i32, NodeError, ()> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, IncNode);
        graph.add_node(TestLabel::B, IncNode);

        let first = graph.try_add_node_edge(TestLabel::A, TestLabel::B);
        assert!(first.is_ok());

        let second = graph.try_add_node_edge(TestLabel::A, TestLabel::B);
        assert!(second.is_err());
    }

    #[test]
    fn add_node_edges_with_single_node_creates_no_edges() {
        let mut graph: Graph<i32, i32, NodeError, ()> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, IncNode);

        graph.add_node_edges((TestLabel::A,));

        let a_label = TestLabel::A.intern();
        let a_state = graph.nodes.get(&a_label).unwrap();
        assert!(a_state.edges.is_empty());
    }

    #[test]
    fn add_condition_edge_stores_branches_and_condition() {
        let mut graph: Graph<i32, i32, NodeError, ()> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, IncNode);
        graph.add_node(TestLabel::B, IncNode);

        let mut branches = HashMap::new();
        branches.insert(TestBranch::Default.intern(), TestLabel::B.intern());

        graph
            .try_add_node_condition_edge(TestLabel::A, branches.clone(), |output: &i32| {
                if *output > 0 {
                    smallvec::smallvec![TestBranch::Default.intern()]
                } else {
                    smallvec::smallvec![]
                }
            })
            .unwrap();

        let a_label = TestLabel::A.intern();
        let node_state = graph.nodes.get(&a_label).unwrap();
        assert_eq!(node_state.edges.len(), 1);

        match &node_state.edges[0] {
            Edge::ConditionalEdge {
                next_nodes,
                condition,
            } => {
                assert_eq!(next_nodes.len(), branches.len());
                // 验证 Vec 中包含正确的分支
                let branch_key = TestBranch::Default.intern();
                let target_node = branches.get(&branch_key).unwrap();
                assert!(next_nodes.contains(&(branch_key, *target_node)));

                let result = (condition)(&1);
                let small_vec: SmallVec<[InternedGraphLabel; 2]> =
                    smallvec::smallvec![TestBranch::Default.intern()];
                assert_eq!(result, small_vec);
            }
            _ => panic!("expected ConditionalEdge"),
        }
    }

    #[tokio::test]
    async fn run_stream_emits_events_and_collects_successors() {
        use futures::StreamExt;

        let mut graph: Graph<i32, i32, NodeError, i32> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, StreamNode);
        graph.add_node(TestLabel::B, StreamNode);
        graph.add_node_edge(TestLabel::A, TestLabel::B);

        let a_label = TestLabel::A.intern();
        let config = Configuration::default();
        let mut stream = graph
            .run_stream(a_label, &0, NodeContext::from_config(&config))
            .await
            .unwrap();

        let mut events = Vec::new();
        let mut next = Vec::new();
        while let Some(event_result) = stream.next().await {
            let event = event_result.unwrap();
            if let GraphEvent::NodeEnd { next_nodes, .. } = &event {
                next = next_nodes.clone();
            }
            events.push(event);
        }

        assert_eq!(next, vec![TestLabel::B.intern()]);
        assert!(!events.is_empty());

        match &events[0] {
            GraphEvent::NodeStart { label } => assert_eq!(*label, a_label),
            _ => panic!("expected first event to be NodeStart"),
        }

        let streaming_values: Vec<i32> = events
            .iter()
            .filter_map(|e| match e {
                GraphEvent::Streaming { event, .. } => Some(*event),
                _ => None,
            })
            .collect();
        assert_eq!(streaming_values, vec![1, 2, 3]);

        match events.last().unwrap() {
            GraphEvent::NodeEnd {
                label,
                output,
                next_nodes,
            } => {
                assert_eq!(*label, a_label);
                assert_eq!(*output, 1);
                assert_eq!(*next_nodes, vec![TestLabel::B.intern()]);
            }
            _ => panic!("expected last event to be NodeEnd"),
        }
    }

    #[tokio::test]
    async fn run_once_executes_and_collects_successors() {
        let mut graph: Graph<i32, i32, NodeError, ()> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, IncNode);
        graph.add_node(TestLabel::B, IncNode);

        graph.add_node_edge(TestLabel::A, TestLabel::B);

        let a_label = TestLabel::A.intern();
        let config = Configuration::default();
        let (output, next) = graph
            .run_once(a_label, &0, NodeContext::from_config(&config))
            .await
            .unwrap();

        assert_eq!(output, 1);
        assert_eq!(next, vec![TestLabel::B.intern()]);
    }

    #[tokio::test]
    async fn run_once_with_mode_sync_executes_correctly() {
        let mut graph: Graph<i32, i32, NodeError, ()> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, IncNode);
        graph.add_node(TestLabel::B, IncNode);

        graph.add_node_edge(TestLabel::A, TestLabel::B);

        let a_label = TestLabel::A.intern();
        let config = Configuration::default();
        let (output, next) = graph
            .run_once(a_label, &0, NodeContext::from_config(&config))
            .await
            .unwrap();

        assert_eq!(output, 1);
        assert_eq!(next, vec![TestLabel::B.intern()]);
    }
}
