//! forge-cap：能力资产注册中心。
//!
//! 一切 Skill/Tool/MCP/API 先注册、后使用。

pub mod registry;

pub use registry::{
    Capability, CapabilityKind, CapabilityRegistry, CapabilityStatus,
    InMemoryCapabilityRegistry,
};
