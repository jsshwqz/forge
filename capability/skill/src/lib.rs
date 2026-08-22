//! forge-skill：技能加载器。
//!
//! 从目录加载 skill.json 并注册为能力。

pub mod loader;

pub use loader::{load_skill_into, load_skill_manifest, SkillManifest};
