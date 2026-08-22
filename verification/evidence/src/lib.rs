//! forge-evidence：不可变证据链存储。
//!
//! 每次验证的原始输出固化为不可变证据，支撑审计与复盘。

pub mod store;

pub use store::{Evidence, EvidenceKind, EvidenceStore, InMemoryEvidenceStore};
