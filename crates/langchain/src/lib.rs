use std::collections::HashMap;

use async_trait::async_trait;
use langgraph::{
    graph::Graph,
    label::GraphLabel,
    node::{Node, NodeError},
    state_graph::StateGraph,
};
