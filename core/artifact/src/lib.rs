//! forge-artifact：产物（Artifact）模型与存储。
//!
//! 第一阶段内存实现，第二阶段换 MinIO 只需新增 ArtifactStore 实现。

pub mod store;

pub use store::{Artifact, ArtifactKind, ArtifactStore, InMemoryArtifactStore};
