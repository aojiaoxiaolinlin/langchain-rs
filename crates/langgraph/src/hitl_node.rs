// Human-in-the-Loop 节点
//
// 这个模块提供了支持人类介入工作流的节点实现。

use crate::{
    interrupt::{InputType, Interrupt, InterruptError, InterruptManager, InterruptResponse},
    node::{EventSink, Node, NodeContext},
};
use langchain_core::{
    message::{Content, Message},
    state::MessagesState,
};
use std::sync::Arc;

/// Human-in-the-Loop 节点
///
/// 在图执行过程中触发中断，等待人类输入后继续执行。
pub struct HumanInTheLoopNode {
    /// 中断管理器
    pub interrupt_manager: Arc<dyn InterruptManager>,
    /// 中断原因
    pub interrupt: Interrupt,
}

impl HumanInTheLoopNode {
    /// 创建新的 HITL 节点
    pub fn new(interrupt_manager: Arc<dyn InterruptManager>, interrupt: Interrupt) -> Self {
        Self {
            interrupt_manager,
            interrupt,
        }
    }

    /// 创建确认节点（需要 yes/no 确认）
    pub fn confirm(
        interrupt_manager: Arc<dyn InterruptManager>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(interrupt_manager, Interrupt::confirmation(message))
    }

    /// 创建文本输入节点
    pub fn input(interrupt_manager: Arc<dyn InterruptManager>, prompt: impl Into<String>) -> Self {
        Self::new(interrupt_manager, Interrupt::input(prompt, InputType::Text))
    }

    /// 创建选择节点（单选）
    pub fn single_choice(
        interrupt_manager: Arc<dyn InterruptManager>,
        prompt: impl Into<String>,
        options: Vec<String>,
    ) -> Self {
        Self::new(
            interrupt_manager,
            Interrupt::input(prompt, InputType::SingleChoice { options }),
        )
    }

    /// 创建布尔确认节点
    pub fn boolean(
        interrupt_manager: Arc<dyn InterruptManager>,
        prompt: impl Into<String>,
    ) -> Self {
        Self::new(
            interrupt_manager,
            Interrupt::input(prompt, InputType::Boolean),
        )
    }

    /// 创建审核节点
    pub fn review(
        interrupt_manager: Arc<dyn InterruptManager>,
        content: impl Into<String>,
        approve_message: impl Into<String>,
        reject_message: impl Into<String>,
    ) -> Self {
        Self::new(
            interrupt_manager,
            Interrupt::review(content, approve_message, reject_message),
        )
    }
}

#[async_trait::async_trait]
impl Node<MessagesState, MessagesState, InterruptError, ()> for HumanInTheLoopNode {
    async fn run_sync(
        &self,
        _input: &MessagesState,
        _context: NodeContext<'_>,
    ) -> Result<MessagesState, InterruptError> {
        // 1. 触发中断
        tracing::info!(
            "Triggering interrupt: id={}, prompt={}",
            self.interrupt.id,
            self.interrupt.as_prompt()
        );

        self.interrupt_manager
            .interrupt(self.interrupt.clone())
            .await?;

        // 2. 等待响应（超时 1 小时）
        let response = self
            .interrupt_manager
            .wait_for_response(&self.interrupt.id, Some(3_600_000))
            .await?;

        tracing::info!(
            "Received response for interrupt {}: {:?}",
            self.interrupt.id,
            response
        );

        // 3. 处理响应
        let mut delta = MessagesState::default();

        match response {
            InterruptResponse::Confirm => {
                // 确认操作，添加系统消息
                delta.push_message_owned(Message::Assistant {
                    content: "操作已确认".to_owned(),
                    tool_calls: None,
                    name: None,
                });
            }
            InterruptResponse::Input { value } => {
                // 用户输入，转换为用户消息
                delta.push_message_owned(Message::User {
                    content: Content::Text(value),
                    name: None,
                });
            }
            InterruptResponse::Approve => {
                // 审核通过
                delta.push_message_owned(Message::Assistant {
                    content: "审核已通过".to_owned(),
                    tool_calls: None,
                    name: None,
                });
            }
            InterruptResponse::Reject { reason } => {
                // 审核拒绝
                delta.push_message_owned(Message::Assistant {
                    content: format!("审核已拒绝: {}", reason),
                    tool_calls: None,
                    name: None,
                });
            }
            InterruptResponse::Cancel => {
                // 用户取消操作
                return Err(InterruptError::InvalidResponse("用户取消了操作".to_owned()));
            }
        }

        Ok(delta)
    }

    async fn run_stream(
        &self,
        input: &MessagesState,
        _sink: &dyn EventSink<()>,
        context: NodeContext<'_>,
    ) -> Result<MessagesState, InterruptError> {
        self.run_sync(input, context).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{checkpoint::RunnableConfig, interrupt::InMemoryInterruptManager};

    #[tokio::test]
    async fn test_hitl_node_confirm() {
        let manager = Arc::new(InMemoryInterruptManager::new());
        let node = HumanInTheLoopNode::confirm(manager.clone(), "是否继续？");

        // 在后台任务中运行节点
        let manager_clone = manager.clone();
        let handle = tokio::spawn(async move {
            // 等待中断出现
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            let pending = manager_clone
                .get_pending(&InMemoryInterruptManager::default_thread_id())
                .await
                .unwrap();

            assert_eq!(pending.len(), 1);

            // 提交确认响应
            manager_clone
                .respond(&pending[0].id, InterruptResponse::Confirm)
                .await
                .unwrap();

            // 节点应该返回 Ok
            Ok::<(), InterruptError>(())
        });

        let config = RunnableConfig::default();
        // 执行节点
        let result = node
            .run_sync(&MessagesState::default(), NodeContext::from_config(&config))
            .await;

        let _ = handle.await.unwrap();

        assert!(result.is_ok());

        if let Ok(state) = result {
            assert_eq!(state.messages.len(), 1);
            if let Message::Assistant { content, .. } = &state.messages[0].as_ref() {
                assert!(content.contains("已确认"));
            }
        }
    }

    #[tokio::test]
    async fn test_hitl_node_input() {
        let manager = Arc::new(InMemoryInterruptManager::new());
        let node = HumanInTheLoopNode::input(manager.clone(), "请输入你的名字");

        // 在后台任务中模拟用户输入
        let manager_clone = manager.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            let pending = manager_clone
                .get_pending(&InMemoryInterruptManager::default_thread_id())
                .await
                .unwrap();

            manager_clone
                .respond(
                    &pending[0].id,
                    InterruptResponse::Input {
                        value: "Alice".to_owned(),
                    },
                )
                .await
                .unwrap();
        });

        let config = RunnableConfig::default();
        let result = node
            .run_sync(&MessagesState::default(), NodeContext::from_config(&config))
            .await;
        handle.await.unwrap();

        assert!(result.is_ok());

        if let Ok(state) = result {
            assert_eq!(state.messages.len(), 1);
            if let Message::User { content, .. } = &state.messages[0].as_ref()
                && let Content::Text(text) = content
            {
                assert_eq!(text, "Alice");
            }
        }
    }

    #[tokio::test]
    async fn test_hitl_node_cancel() {
        let manager = Arc::new(InMemoryInterruptManager::new());
        let node = HumanInTheLoopNode::confirm(manager.clone(), "测试取消");

        let manager_clone = manager.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            let pending = manager_clone
                .get_pending(&InMemoryInterruptManager::default_thread_id())
                .await
                .unwrap();

            manager_clone
                .respond(&pending[0].id, InterruptResponse::Cancel)
                .await
                .unwrap();
        });

        let config = RunnableConfig::default();
        let result = node
            .run_sync(&MessagesState::default(), NodeContext::from_config(&config))
            .await;
        handle.await.unwrap();

        assert!(result.is_err());
        assert!(matches!(result, Err(InterruptError::InvalidResponse(_))));
    }

    #[tokio::test]
    async fn test_hitl_node_timeout() {
        let manager = Arc::new(InMemoryInterruptManager::new());

        // 创建一个短超时的节点（100ms）
        let node = HumanInTheLoopNode {
            interrupt_manager: manager.clone(),
            interrupt: Interrupt::confirmation("测试超时"),
        };

        // 不提交响应，应该超时
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        // 再次尝试等待响应（使用非常短的超时）
        let config = RunnableConfig::default();
        let result = tokio::time::timeout(
            tokio::time::Duration::from_millis(50),
            node.run_sync(&MessagesState::default(), NodeContext::from_config(&config)),
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_hitl_node_review() {
        let manager = Arc::new(InMemoryInterruptManager::new());
        let node = HumanInTheLoopNode::review(manager.clone(), "执行敏感操作", "批准", "拒绝");

        let manager_clone = manager.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            let pending = manager_clone
                .get_pending(&InMemoryInterruptManager::default_thread_id())
                .await
                .unwrap();

            // 拒绝操作
            manager_clone
                .respond(
                    &pending[0].id,
                    InterruptResponse::Reject {
                        reason: "安全风险".to_owned(),
                    },
                )
                .await
                .unwrap();
        });

        let config = RunnableConfig::default();
        let result = node
            .run_sync(&MessagesState::default(), NodeContext::from_config(&config))
            .await;
        handle.await.unwrap();

        assert!(result.is_ok());

        if let Ok(state) = result {
            assert_eq!(state.messages.len(), 1);
            if let Message::Assistant { content, .. } = &state.messages[0].as_ref() {
                assert!(content.contains("审核已拒绝"));
                assert!(content.contains("安全风险"));
            }
        }
    }

    #[tokio::test]
    async fn test_hitl_node_single_choice() {
        let manager = Arc::new(InMemoryInterruptManager::new());
        let node = HumanInTheLoopNode::single_choice(
            manager.clone(),
            "选择颜色",
            vec!["红色".to_owned(), "绿色".to_owned(), "蓝色".to_owned()],
        );

        let manager_clone = manager.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            let pending = manager_clone
                .get_pending(&InMemoryInterruptManager::default_thread_id())
                .await
                .unwrap();

            // 选择第一个选项
            manager_clone
                .respond(
                    &pending[0].id,
                    InterruptResponse::Input {
                        value: "红色".to_owned(),
                    },
                )
                .await
                .unwrap();
        });

        let config = RunnableConfig::default();
        let result = node
            .run_sync(&MessagesState::default(), NodeContext::from_config(&config))
            .await;
        handle.await.unwrap();

        assert!(result.is_ok());

        if let Ok(state) = result {
            assert_eq!(state.messages.len(), 1);
            if let Message::User { content, .. } = &state.messages[0].as_ref()
                && let Content::Text(text) = content
            {
                assert_eq!(text, "红色");
            }
        }
    }

    #[tokio::test]
    async fn test_hitl_node_boolean() {
        let manager = Arc::new(InMemoryInterruptManager::new());
        let node = HumanInTheLoopNode::boolean(manager.clone(), "是否同意？");

        let manager_clone = manager.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            let pending = manager_clone
                .get_pending(&InMemoryInterruptManager::default_thread_id())
                .await
                .unwrap();

            // 这里我们用 Input 模拟 "yes" 响应
            manager_clone
                .respond(
                    &pending[0].id,
                    InterruptResponse::Input {
                        value: "yes".to_owned(),
                    },
                )
                .await
                .unwrap();
        });
        let config = RunnableConfig::default();
        let result = node
            .run_sync(&MessagesState::default(), NodeContext::from_config(&config))
            .await;
        handle.await.unwrap();

        assert!(result.is_ok());

        if let Ok(state) = result {
            assert_eq!(state.messages.len(), 1);
            // 输入 "yes" 会被作为用户消息存储
            assert!(matches!(state.messages[0].as_ref(), Message::User { .. }));
        }
    }
}
