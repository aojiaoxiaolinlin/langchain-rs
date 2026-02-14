use crate::label::InternedGraphLabel;
use smallvec::SmallVec;

/// 条件边的条件函数，输入为边的输出，输出为后继节点标签列表
pub type EdgeCondition<O> = Box<dyn Fn(&O) -> SmallVec<[InternedGraphLabel; 2]> + Send + Sync>;

pub enum Edge<O> {
    /// 普通边，直接连接两个节点
    NodeEdge(InternedGraphLabel),
    /// 条件边，根据条件判断是否执行下一个节点
    ConditionalEdge {
        /// 条件边的可能的输出节点列表，根据条件判断执行哪个节点
        // 使用 Vec 而不是 HashMap，因为分支数量通常很少，线性查找更快且内存更紧凑
        // (branch_key, target_node_label)
        // 使用 SmallVec 优化小规模数据的内存分配，默认内联存储4个元素
        next_nodes: SmallVec<[(InternedGraphLabel, InternedGraphLabel); 4]>,
        condition: EdgeCondition<O>,
    },
}
