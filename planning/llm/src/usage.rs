//! Token 用量与成本账本（V3.2 成本分层路由的计量基础，门禁 G-V3.2）。
//!
//! - [`TokenUsage`]：单次调用的 token 计数（OpenAI 兼容 /usage 字段）
//! - [`CostEntry`]：一条可序列化的调用记录（模型 + 用途 + 计数）
//! - [`UsageLedger`]：线程安全的进程内账本；LlmPlanner/LlmReplanner/Reviewer
//!   每次成功 LLM 调用后记账，编排方在收尾时统一落盘（如写进 Session payload）

use serde::Serialize;
use std::sync::Mutex;

/// 单次 LLM 调用的 token 用量。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct TokenUsage {
    /// 提示词 token 数。
    pub prompt_tokens: u64,
    /// 补全 token 数。
    pub completion_tokens: u64,
}

impl TokenUsage {
    /// 总 token 数。
    pub fn total(&self) -> u64 {
        self.prompt_tokens.saturating_add(self.completion_tokens)
    }
}

/// 一条成本记录。
#[derive(Clone, Debug, Serialize)]
pub struct CostEntry {
    /// 调用所用模型。
    pub model: String,
    /// 用途标签（plan / replan / review ...）。
    pub purpose: String,
    /// 提示词 token。
    pub prompt_tokens: u64,
    /// 补全 token。
    pub completion_tokens: u64,
}

/// 进程内用量账本（线程安全）。
#[derive(Default)]
pub struct UsageLedger {
    entries: Mutex<Vec<CostEntry>>,
}

impl UsageLedger {
    /// 新建空账本。
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一次调用。
    pub fn record(&self, entry: CostEntry) {
        if let Ok(mut g) = self.entries.lock() {
            g.push(entry);
        }
    }

    /// 取全部记录快照（不清空）。
    pub fn snapshot(&self) -> Vec<CostEntry> {
        self.entries.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// 取走全部记录（清空）。编排方在收尾时用。
    pub fn drain(&self) -> Vec<CostEntry> {
        self.entries.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
    }
}
