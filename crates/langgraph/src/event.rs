use crate::label::InternedGraphLabel;

/// 图执行期间的事件
///
/// # 类型参数
///
/// * `Ev` - 事件类型
#[derive(Debug, Clone)]
pub enum GraphEvent<Ev, O, NS> {
    /// 节点开始执行
    NodeStart {
        /// 节点标签
        label: InternedGraphLabel,
    },
    /// 节点执行结束
    NodeEnd {
        /// 节点标签
        label: InternedGraphLabel,
        /// 节点输出
        output: O,
        /// 当前节点状态
        node_state: NS,
    },
    /// 流式事件（如 LLM token）
    Streaming {
        /// 节点标签
        label: InternedGraphLabel,
        /// 流式事件
        event: Ev,
    },
    /// 图执行完成
    GraphComplete {
        /// 最终节点标签
        final_label: InternedGraphLabel,
    },
    /// 执行错误
    GraphError {
        /// 错误信息
        error: String,
    },
}

impl<Ev, O, NS> GraphEvent<Ev, O, NS> {
    /// 创建节点开始事件
    pub fn node_start(label: InternedGraphLabel) -> Self {
        Self::NodeStart { label }
    }

    /// 创建节点结束事件
    pub fn node_end(label: InternedGraphLabel, output: O, node_state: NS) -> Self {
        Self::NodeEnd {
            label,
            output,
            node_state,
        }
    }

    /// 创建流式事件
    pub fn streaming(label: InternedGraphLabel, event: Ev) -> Self {
        Self::Streaming { label, event }
    }

    /// 创建图完成事件
    pub fn graph_complete(final_label: InternedGraphLabel) -> Self {
        Self::GraphComplete { final_label }
    }

    /// 创建错误事件
    pub fn graph_error(error: String) -> Self {
        Self::GraphError { error }
    }
}
