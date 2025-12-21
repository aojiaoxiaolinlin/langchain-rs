use std::collections::HashMap;

use thiserror::Error;

use crate::{
    edge::{BranchKind, Edge, EdgeCondition},
    label::{GraphLabel, InternedGraphLabel, IntoGraphNodeArray},
    node::{Node, NodeState},
};

#[derive(Default)]
pub struct Graph<I, O, E, B: BranchKind> {
    pub nodes: HashMap<InternedGraphLabel, NodeState<I, O, E, B>>,
}

impl<I, O, E, B: BranchKind> Graph<I, O, E, B> {
    /// 添加一个节点到图中
    pub fn add_node<T>(&mut self, label: impl GraphLabel, node: T)
    where
        T: Node<I, O, E>,
    {
        let label = label.intern();
        let node_state = NodeState::new(label, node);
        self.nodes.insert(label, node_state);
    }

    pub fn get_node_state_mut(
        &mut self,
        label: impl GraphLabel,
    ) -> Result<&mut NodeState<I, O, E, B>, GraphError> {
        let label = label.intern();
        self.nodes
            .get_mut(&label)
            .ok_or(GraphError::InvalidNode(label))
    }

    pub fn try_add_node_edge(
        &mut self,
        pred_node: impl GraphLabel,
        next_node: impl GraphLabel,
    ) -> Result<(), GraphError> {
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
        branches: HashMap<B, InternedGraphLabel>,
        condition: F,
    ) -> Result<(), GraphError>
    where
        F: Fn(&O) -> Vec<B> + Send + Sync + 'static,
    {
        let branch_keys: std::collections::HashSet<_> = branches.keys().copied().collect();

        // 以后有必要再为每个条件分支设计单独的分支类型，这样做next_nodes的Key就得使用Box<dyn BranchKind>，目前没得必要
        // 检查 condition 是否返回的分支都在 branches 中
        let wrapped: EdgeCondition<O, B> = Box::new(move |o: &O| {
            let result = condition(o);
            assert!(
                result.iter().all(|b| branch_keys.contains(b)),
                "Edge::conditional: condition returned branch not in branches map"
            );
            result
        });

        let pred_node = pred_node.intern();

        let pred_node_state = self.get_node_state_mut(pred_node)?;

        let edge = Edge::ConditionalEdge {
            next_nodes: branches,
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
        branches: HashMap<B, InternedGraphLabel>,
        condition: F,
    ) where
        F: Fn(&O) -> Vec<B> + Send + Sync + 'static,
    {
        self.try_add_node_condition_edge(pred_node, branches, condition)
            .unwrap();
    }
}

#[derive(Debug)]
pub enum GraphStepError<E> {
    Graph(GraphError),
    Node(E),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphError {
    /// 无效的节点标签
    #[error("node {0:?} dose not exist")]
    InvalidNode(InternedGraphLabel),

    /// 边已存在
    #[error("edge {0:?} already exists")]
    EdgeAlreadyExists(InternedGraphLabel),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::label::GraphLabel;
    use crate::node::NodeError;
    use async_trait::async_trait;

    #[derive(Debug, Clone, PartialEq, Eq, Hash, GraphLabel)]
    enum TestLabel {
        A,
        B,
    }

    #[derive(Debug)]
    struct IncNode;

    #[async_trait]
    impl Node<i32, i32, NodeError> for IncNode {
        async fn run(&self, input: &i32) -> Result<i32, NodeError> {
            Ok(*input + 1)
        }
    }

    #[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
    enum TestBranch {
        Default,
        #[expect(unused)]
        Alt,
    }

    #[test]
    fn add_node_and_edge_should_link_successor() {
        let mut graph: Graph<i32, i32, NodeError, TestBranch> = Graph {
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
    fn duplicate_edge_returns_error() {
        let mut graph: Graph<i32, i32, NodeError, TestBranch> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, IncNode);
        graph.add_node(TestLabel::B, IncNode);

        let first = graph.try_add_node_edge(TestLabel::A, TestLabel::B);
        assert!(first.is_ok());

        let second = graph.try_add_node_edge(TestLabel::A, TestLabel::B);
        let b_label = TestLabel::B.intern();
        assert_eq!(second, Err(GraphError::EdgeAlreadyExists(b_label)));
    }

    #[test]
    fn add_condition_edge_stores_branches_and_condition() {
        let mut graph: Graph<i32, i32, NodeError, TestBranch> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, IncNode);
        graph.add_node(TestLabel::B, IncNode);

        let mut branches = HashMap::new();
        branches.insert(TestBranch::Default, TestLabel::B.intern());

        graph
            .try_add_node_condition_edge(TestLabel::A, branches.clone(), |output: &i32| {
                if *output > 0 {
                    vec![TestBranch::Default]
                } else {
                    Vec::new()
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
                assert_eq!(
                    next_nodes.get(&TestBranch::Default),
                    branches.get(&TestBranch::Default)
                );

                let result = (condition)(&1);
                assert_eq!(result, vec![TestBranch::Default]);
            }
            _ => panic!("expected ConditionalEdge"),
        }
    }

    #[tokio::test]
    async fn run_once_executes_and_collects_successors() {
        let mut graph: Graph<i32, i32, NodeError, TestBranch> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, IncNode);
        graph.add_node(TestLabel::B, IncNode);

        graph.add_node_edge(TestLabel::A, TestLabel::B);

        let a_label = TestLabel::A.intern();
        let (output, next) = graph.run_once(a_label, &0).await.unwrap();

        assert_eq!(output, 1);
        assert_eq!(next, vec![TestLabel::B.intern()]);
    }
}
