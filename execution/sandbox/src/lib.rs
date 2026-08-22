//! forge-sandbox：权限策略引擎实现。
//!
//! `PermissionPolicy` trait 和 `PolicyContext` 定义在 forge-exec 中，
//! 本 crate 提供具体实现：DenyAllPolicy, AllowListPolicy, PolicyChain。
//! 默认保守（只读），高风险需显式放行。

pub mod policy;

pub use policy::{AllowListPolicy, DefaultPolicy, DenyAllPolicy, PolicyChain};
// 从 forge-exec 重新导出
pub use forge_exec::{PermissionPolicy, PolicyContext};
