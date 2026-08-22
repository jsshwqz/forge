//! forge-gates：质量门禁。
//!
//! 所有验收标准通过才允许任务完成。
//! 这是整个体系"验证即完成条件"的强制点。

pub mod gate;

pub use gate::{Gate, GateDecision, GatePolicy, GateSpec};
