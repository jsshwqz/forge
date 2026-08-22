//! forge-product：产品清单、模板与装配。
//!
//! 第一阶段终点：Manifest + 注册表 → 完整可运行的 ProductAssembly。

pub mod manifest;
pub mod template;
pub mod assembly;

pub use manifest::{check_manifest, CapabilityRef, ProductManifest};
pub use template::{instantiate, ProductTemplate, TemplateParam};
pub use assembly::{assemble, ProductAssembly};
