//! forge-sandbox：权限策略引擎。
//!
//! 在执行前裁决"该工具此刻是否被允许"。
//! 默认保守（只读），高风险需显式放行。

pub mod policy;

pub use policy::{
    AllowListPolicy, DefaultPolicy, DenyAllPolicy, PolicyChain, PolicyContext, PermissionPolicy,
};
