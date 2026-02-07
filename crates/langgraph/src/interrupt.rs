// Human-in-the-Loop (HITL) 中断机制
//
// 这个模块提供了人类介入工作流的支持，允许图执行过程中中断并等待人类输入。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 中断原因
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InterruptReason {
    /// 需要人工确认
    Confirmation { message: String },
    /// 需要人工输入
    Input {
        prompt: String,
        input_type: InputType,
    },
    /// 需要人工审核
    Review {
        content: String,
        approve_message: String,
        reject_message: String,
    },
}

impl InterruptReason {
    /// 获取用于显示的提示文本
    pub fn as_prompt(&self) -> String {
        match self {
            InterruptReason::Confirmation { message } => {
                format!("确认操作: {} (同意/拒绝)", message)
            }
            InterruptReason::Input { prompt, input_type } => match input_type {
                InputType::Text => {
                    format!("请输入: {}", prompt)
                }
                InputType::SingleChoice { options } => {
                    format!(
                        "请选择 (输入选项编号):\n{}\n你的选择:",
                        options
                            .iter()
                            .enumerate()
                            .map(|(i, opt)| format!("  {}. {}", i + 1, opt))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                }
                InputType::MultipleChoice { options } => {
                    format!(
                        "请选择 (可多选，输入选项编号用逗号分隔):\n{}\n你的选择:",
                        options
                            .iter()
                            .enumerate()
                            .map(|(i, opt)| format!("  {}. {}", i + 1, opt))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                }
                InputType::Boolean => {
                    format!("是否同意 (yes/no): {}", prompt)
                }
                InputType::Number => {
                    format!("请输入数字: {}", prompt)
                }
            },
            InterruptReason::Review {
                content,
                approve_message,
                reject_message,
            } => {
                format!(
                    "审核内容:\n{}\n\n同意: {}\n拒绝: {}",
                    content, approve_message, reject_message
                )
            }
        }
    }
}

/// 输入类型（类型安全的中断）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputType {
    /// 文本输入
    Text,
    /// 选择题（单选）
    SingleChoice { options: Vec<String> },
    /// 选择题（多选）
    MultipleChoice { options: Vec<String> },
    /// 布尔值（是/否）
    Boolean,
    /// 数字输入
    Number,
}

/// 中断数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interrupt {
    /// 唯一标识符
    pub id: String,
    /// 中断原因
    pub reason: InterruptReason,
    /// 时间戳（Unix 时间戳，秒）
    pub timestamp: u64,
}

impl Interrupt {
    /// 创建新的中断
    pub fn new(reason: InterruptReason) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            reason,
            timestamp: now,
        }
    }

    /// 获取用于显示的提示文本
    pub fn as_prompt(&self) -> String {
        self.reason.as_prompt()
    }

    /// 创建确认中断
    pub fn confirmation(message: impl Into<String>) -> Self {
        Self::new(InterruptReason::Confirmation {
            message: message.into(),
        })
    }

    /// 创建输入中断
    pub fn input(prompt: impl Into<String>, input_type: InputType) -> Self {
        Self::new(InterruptReason::Input {
            prompt: prompt.into(),
            input_type,
        })
    }

    /// 创建审核中断
    pub fn review(
        content: impl Into<String>,
        approve_message: impl Into<String>,
        reject_message: impl Into<String>,
    ) -> Self {
        Self::new(InterruptReason::Review {
            content: content.into(),
            approve_message: approve_message.into(),
            reject_message: reject_message.into(),
        })
    }
}

/// 中断响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InterruptResponse {
    /// 确认继续
    Confirm,
    /// 提供输入
    Input { value: String },
    /// 审核通过
    Approve,
    /// 审核拒绝（带原因）
    Reject { reason: String },
    /// 取消执行
    Cancel,
}

/// Human-in-the-Loop 错误
#[derive(Debug, thiserror::Error)]
pub enum InterruptError {
    #[error("no pending interrupt")]
    NoPendingInterrupt,

    #[error("interrupt not found: {0}")]
    NotFound(String),

    #[error("invalid response: {0}")]
    InvalidResponse(String),

    #[error("interrupt timeout")]
    Timeout,
}

/// 中断管理器 trait
///
/// 定义了管理中断生命周期的接口，包括创建中断、等待响应、查询待处理中断等。
#[async_trait]
pub trait InterruptManager: Send + Sync {
    /// 触发中断
    async fn interrupt(&self, interrupt: Interrupt) -> Result<(), InterruptError>;

    /// 等待响应
    async fn wait_for_response(
        &self,
        interrupt_id: &str,
        timeout_ms: Option<u64>,
    ) -> Result<InterruptResponse, InterruptError>;

    /// 获取待处理的中断列表
    async fn get_pending(&self, thread_id: &str) -> Result<Vec<Interrupt>, InterruptError>;

    /// 提交响应
    async fn respond(
        &self,
        interrupt_id: &str,
        response: InterruptResponse,
    ) -> Result<(), InterruptError>;

    /// 取消中断
    async fn cancel(&self, interrupt_id: &str) -> Result<bool, InterruptError>;
}

/// 内存中断管理器实现
///
/// 使用内存存储中断状态和响应，适用于开发和测试环境。
/// 生产环境应使用持久化存储（如数据库）。
#[derive(Debug, Default, Clone)]
pub struct InMemoryInterruptManager {
    /// 待处理的中断: thread_id -> Vec<Interrupt>
    pending: Arc<RwLock<HashMap<String, Vec<Interrupt>>>>,
    /// 响应: interrupt_id -> InterruptResponse
    responses: Arc<RwLock<HashMap<String, InterruptResponse>>>,
    /// 通知通道（用于唤醒等待的任务）
    notify: Arc<(tokio::sync::Notify, tokio::sync::Notify)>,
}

impl InMemoryInterruptManager {
    /// 创建新的内存中断管理器
    pub fn new() -> Self {
        Self::default()
    }

    /// 生成默认的 thread_id（如果未提供）
    pub fn default_thread_id() -> String {
        "default".to_owned()
    }
}

#[async_trait]
impl InterruptManager for InMemoryInterruptManager {
    async fn interrupt(&self, interrupt: Interrupt) -> Result<(), InterruptError> {
        let mut pending = self.pending.write().await;
        let thread_id = Self::default_thread_id();

        pending
            .entry(thread_id)
            .or_insert_with(Vec::new)
            .push(interrupt.clone());

        // 通知等待的响应者
        self.notify.0.notify_waiters();

        tracing::info!(
            "Interrupt created: id={}, reason={:?}",
            interrupt.id,
            interrupt.reason
        );

        Ok(())
    }

    async fn wait_for_response(
        &self,
        interrupt_id: &str,
        timeout_ms: Option<u64>,
    ) -> Result<InterruptResponse, InterruptError> {
        let start = std::time::Instant::now();

        loop {
            // 检查是否已有响应
            {
                let responses = self.responses.read().await;
                if let Some(response) = responses.get(interrupt_id) {
                    return Ok(response.clone());
                }
            }

            // 检查超时
            if let Some(timeout) = timeout_ms
                && start.elapsed().as_millis() > timeout as u128
            {
                return Err(InterruptError::Timeout);
            }

            // 等待通知（使用短超时以避免阻塞太久）
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {}
                _ = self.notify.1.notified() => {}
            }
        }
    }

    async fn get_pending(&self, thread_id: &str) -> Result<Vec<Interrupt>, InterruptError> {
        let pending = self.pending.read().await;
        Ok(pending.get(thread_id).cloned().unwrap_or_default())
    }

    async fn respond(
        &self,
        interrupt_id: &str,
        response: InterruptResponse,
    ) -> Result<(), InterruptError> {
        // 存储响应
        {
            let mut responses = self.responses.write().await;
            responses.insert(interrupt_id.to_owned(), response);
        }

        // 从待处理列表中移除
        {
            let mut pending = self.pending.write().await;
            for (_, interrupts) in pending.iter_mut() {
                if let Some(pos) = interrupts.iter().position(|i| i.id == interrupt_id) {
                    interrupts.remove(pos);
                    break;
                }
            }
        }

        // 通知等待的任务
        self.notify.1.notify_waiters();

        tracing::info!("Interrupt response submitted: id={}", interrupt_id);

        Ok(())
    }

    async fn cancel(&self, interrupt_id: &str) -> Result<bool, InterruptError> {
        let mut pending = self.pending.write().await;

        for (_, interrupts) in pending.iter_mut() {
            if let Some(pos) = interrupts.iter().position(|i| i.id == interrupt_id) {
                interrupts.remove(pos);
                return Ok(true);
            }
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_interrupt_creation() {
        let manager = InMemoryInterruptManager::new();
        let interrupt = Interrupt::confirmation("测试确认");

        manager.interrupt(interrupt).await.unwrap();
    }

    #[tokio::test]
    async fn test_interrupt_and_response() {
        let manager = InMemoryInterruptManager::new();
        let interrupt = Interrupt::input("请输入你的名字", InputType::Text);

        // 创建中断
        manager.interrupt(interrupt.clone()).await.unwrap();

        // 获取待处理中断
        let pending = manager
            .get_pending(&InMemoryInterruptManager::default_thread_id())
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, interrupt.id);

        // 在后台任务中等待响应
        let manager_clone = manager.clone();
        let interrupt_id_clone = interrupt.id.clone();
        let handle = tokio::spawn(async move {
            manager_clone
                .wait_for_response(&interrupt_id_clone, Some(5000))
                .await
        });

        // 稍微延迟后提交响应
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        manager
            .respond(
                &interrupt.id,
                InterruptResponse::Input {
                    value: "Alice".to_owned(),
                },
            )
            .await
            .unwrap();

        // 等待结果
        let response = handle.await.unwrap().unwrap();
        match response {
            InterruptResponse::Input { value } => {
                assert_eq!(value, "Alice");
            }
            _ => panic!("Unexpected response"),
        }
    }

    #[tokio::test]
    async fn test_interrupt_timeout() {
        let manager = InMemoryInterruptManager::new();
        let interrupt = Interrupt::confirmation("测试");

        manager.interrupt(interrupt.clone()).await.unwrap();

        // 等待响应但超时（不提交响应）
        let result = manager.wait_for_response(&interrupt.id, Some(100)).await;

        assert!(matches!(result, Err(InterruptError::Timeout)));
    }

    #[tokio::test]
    async fn test_interrupt_cancel() {
        let manager = InMemoryInterruptManager::new();
        let interrupt = Interrupt::confirmation("测试");

        manager.interrupt(interrupt.clone()).await.unwrap();

        // 取消中断
        let cancelled = manager.cancel(&interrupt.id).await.unwrap();
        assert!(cancelled);

        // 再次取消应返回 false
        let cancelled = manager.cancel(&interrupt.id).await.unwrap();
        assert!(!cancelled);

        // 验证已从待处理列表中移除
        let pending = manager
            .get_pending(&InMemoryInterruptManager::default_thread_id())
            .await
            .unwrap();
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn test_multiple_interrupts() {
        let manager = InMemoryInterruptManager::new();

        // 创建多个中断
        let int1 = Interrupt::confirmation("中断1");
        let int2 = Interrupt::input("中断2", InputType::Text);
        let int3 = Interrupt::confirmation("中断3");

        manager.interrupt(int1).await.unwrap();
        manager.interrupt(int2).await.unwrap();
        manager.interrupt(int3).await.unwrap();

        // 获取所有待处理中断
        let pending = manager
            .get_pending(&InMemoryInterruptManager::default_thread_id())
            .await
            .unwrap();
        assert_eq!(pending.len(), 3);

        // 响应其中一个
        manager
            .respond(&pending[1].id, InterruptResponse::Confirm)
            .await
            .unwrap();

        // 验证数量减少
        let pending = manager
            .get_pending(&InMemoryInterruptManager::default_thread_id())
            .await
            .unwrap();
        assert_eq!(pending.len(), 2);
    }

    #[tokio::test]
    async fn test_interrupt_reason_display() {
        let confirm = Interrupt::confirmation("是否继续执行？");
        assert!(confirm.as_prompt().contains("确认操作"));
        assert!(confirm.as_prompt().contains("是否继续执行？"));

        let input = Interrupt::input("请输入年龄", InputType::Number);
        assert!(input.as_prompt().contains("请输入数字"));
        assert!(input.as_prompt().contains("请输入年龄"));

        let review = Interrupt::review("操作内容", "批准", "拒绝");
        assert!(review.as_prompt().contains("审核内容"));
        assert!(review.as_prompt().contains("操作内容"));
    }

    #[tokio::test]
    async fn test_input_type_choice() {
        let input = Interrupt::input(
            "选择颜色",
            InputType::SingleChoice {
                options: vec!["红色".to_owned(), "绿色".to_owned(), "蓝色".to_owned()],
            },
        );

        let prompt = input.as_prompt();
        assert!(prompt.contains("请选择"));
        assert!(prompt.contains("1. 红色"));
        assert!(prompt.contains("2. 绿色"));
        assert!(prompt.contains("3. 蓝色"));
    }
}
