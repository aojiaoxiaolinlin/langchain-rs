use crate::{
    checkpoint::RunnableConfig,
    graph::{Graph, GraphError},
    label::InternedGraphLabel,
    node::NodeContext,
};
use std::fmt::Debug;

pub struct Executor<'g, I, O, E: std::error::Error, Ev: Debug = ()> {
    pub graph: &'g Graph<I, O, E, Ev>,
    pub current: InternedGraphLabel,
}

impl<'g, I, O, E: std::error::Error, Ev: Debug> Executor<'g, I, O, E, Ev> {
    pub fn new(graph: &'g Graph<I, O, E, Ev>, start: InternedGraphLabel) -> Self {
        Self {
            graph,
            current: start,
        }
    }

    /// 执行一步（使用 Sync 模式）
    pub async fn step(&mut self, input: &I) -> Result<(O, Vec<InternedGraphLabel>), GraphError<E>>
    where
        I: Send + Sync + 'static,
        O: Send + Sync + 'static,
        E: Send + Sync + 'static,
        Ev: Send + Sync + 'static,
    {
        let config = RunnableConfig::default();

        let (output, next) = self
            .graph
            .run_once(self.current, input, NodeContext::from_config(&config))
            .await?;
        if next.len() == 1 {
            self.current = next[0];
        }
        Ok((output, next))
    }

    /// 执行直到卡住（使用 Sync 模式）
    pub async fn run_until_stuck(
        &mut self,
        input: &I,
        max_steps: usize,
    ) -> Result<Vec<(O, Vec<InternedGraphLabel>)>, GraphError<E>>
    where
        I: Send + Sync + 'static,
        O: Send + Sync + 'static,
        E: Send + Sync + 'static,
        Ev: Send + Sync + 'static,
    {
        let mut steps = Vec::new();

        for _ in 0..max_steps {
            let (output, next) = self.step(input).await?;
            let should_stop = next.len() != 1;
            steps.push((output, next));
            if should_stop {
                break;
            }
        }

        Ok(steps)
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use async_trait::async_trait;

    use crate::{
        executor::Executor,
        graph::Graph,
        label::GraphLabel,
        node::{EventSink, Node, NodeContext, NodeError},
    };

    #[derive(Debug, Clone, PartialEq, Eq, Hash, GraphLabel)]
    enum TestLabel {
        A,
        B,
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
            _sink: &mut dyn EventSink<()>,
            _context: NodeContext<'_>,
        ) -> Result<i32, NodeError> {
            Ok(*input + 1)
        }
    }

    #[tokio::test]
    async fn executor_drives_linear_flow() {
        let mut graph: Graph<i32, i32, NodeError, ()> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, IncNode);
        graph.add_node(TestLabel::B, IncNode);

        graph.add_node_edge(TestLabel::A, TestLabel::B);

        let a_label = TestLabel::A.intern();
        let mut executor = Executor::new(&graph, a_label);

        let (out1, next1) = executor.step(&0).await.unwrap();
        assert_eq!(out1, 1);
        assert_eq!(next1, vec![TestLabel::B.intern()]);
        assert_eq!(executor.current, TestLabel::B.intern());

        let (out2, next2) = executor.step(&1).await.unwrap();
        assert_eq!(out2, 2);
        assert!(next2.is_empty());
        assert_eq!(executor.current, TestLabel::B.intern());
    }

    #[tokio::test]
    async fn run_until_stuck_executes_multiple_steps() {
        let mut graph: Graph<i32, i32, NodeError, ()> = Graph {
            nodes: HashMap::new(),
        };

        graph.add_node(TestLabel::A, IncNode);
        graph.add_node(TestLabel::B, IncNode);

        graph.add_node_edge(TestLabel::A, TestLabel::B);

        let a_label = TestLabel::A.intern();
        let mut executor = Executor::new(&graph, a_label);

        let steps = executor.run_until_stuck(&0, 10).await.unwrap();

        assert_eq!(steps.len(), 2);
        assert_eq!(executor.current, TestLabel::B.intern());

        assert_eq!(steps[0].1, vec![TestLabel::B.intern()]);
        assert!(steps[1].1.is_empty());
    }
}
