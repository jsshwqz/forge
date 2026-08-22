//! Skill 目录装载器。

use forge_cap::{
    Capability, CapabilityKind, CapabilityRegistry, CapabilityStatus,
};
use forge_core::{CapabilityId, ForgeError, ForgeResult};
use forge_exec::PermissionLevel;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 技能清单。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillManifest {
    /// 名称。
    pub name: String,
    /// 版本。
    pub version: String,
    /// 描述。
    pub description: String,
    /// 入口文件（相对技能目录）。
    pub entry: String,
    /// 所需权限。
    pub permission: PermissionLevel,
}

/// 读取 `<dir>/skill.json`。
///
/// 文件缺失 → `NotFound`；解析失败 → `InvalidState(含文件路径)`。
///
/// 第一阶段不做签名/校验和验证（已知风险，见文档注释）。
pub fn load_skill_manifest(dir: &Path) -> ForgeResult<SkillManifest> {
    let manifest_path = dir.join("skill.json");
    if !manifest_path.exists() {
        return Err(ForgeError::NotFound(format!(
            "skill.json not found in: {}",
            dir.display()
        )));
    }
    let content = std::fs::read_to_string(&manifest_path).map_err(|e| {
        ForgeError::InvalidState(format!("failed to read {}: {}", manifest_path.display(), e))
    })?;
    let manifest: SkillManifest = serde_json::from_str(&content).map_err(|e| {
        ForgeError::InvalidState(format!(
            "failed to parse {}: {}",
            manifest_path.display(),
            e
        ))
    })?;

    // 必填字段校验
    if manifest.name.is_empty() {
        return Err(ForgeError::InvalidState(
            "skill.json: field 'name' is required".into(),
        ));
    }
    if manifest.version.is_empty() {
        return Err(ForgeError::InvalidState(
            "skill.json: field 'version' is required".into(),
        ));
    }
    if manifest.entry.is_empty() {
        return Err(ForgeError::InvalidState(
            "skill.json: field 'entry' is required".into(),
        ));
    }

    Ok(manifest)
}

/// 装载 = 解析 + 必填字段校验 + 注册进 registry，kind=Skill。
pub async fn load_skill_into(
    dir: &Path,
    registry: &dyn CapabilityRegistry,
) -> ForgeResult<CapabilityId> {
    let manifest = load_skill_manifest(dir)?;

    let cap = Capability {
        id: CapabilityId::new_capability_id(),
        name: manifest.name,
        kind: CapabilityKind::Skill,
        version: manifest.version,
        entry: manifest.entry,
        status: CapabilityStatus::Registered,
        permission: manifest.permission,
    };

    registry.register(cap).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_cap::InMemoryCapabilityRegistry;
    use std::io::Write;

    fn make_skill_json(name: &str, version: &str, entry: &str) -> String {
        serde_json::json!({
            "name": name,
            "version": version,
            "description": "test skill",
            "entry": entry,
            "permission": "ReadOnly"
        })
        .to_string()
    }

    #[tokio::test]
    async fn test_load_valid_skill() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("skill.json")).unwrap();
        f.write_all(make_skill_json("my-skill", "1.0.0", "main.rs").as_bytes())
            .unwrap();

        let registry = InMemoryCapabilityRegistry::default();
        let id = load_skill_into(dir.path(), &registry).await.unwrap();
        let cap = registry.get(&id).await.unwrap();
        assert_eq!(cap.name, "my-skill");
        assert_eq!(cap.kind, CapabilityKind::Skill);
    }

    #[tokio::test]
    async fn test_missing_name_field() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("skill.json")).unwrap();
        f.write_all(
            serde_json::json!({
                "version": "1.0.0",
                "description": "test",
                "entry": "main.rs",
                "permission": "ReadOnly"
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();

        let result = load_skill_manifest(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("name"));
    }

    #[tokio::test]
    async fn test_bad_json() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("skill.json")).unwrap();
        f.write_all(b"not valid json").unwrap();

        let result = load_skill_manifest(dir.path());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_skill_manifest(dir.path());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_duplicate_load_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("skill.json")).unwrap();
        f.write_all(make_skill_json("dup", "1.0.0", "main.rs").as_bytes())
            .unwrap();

        let registry = InMemoryCapabilityRegistry::default();
        load_skill_into(dir.path(), &registry).await.unwrap();
        let result = load_skill_into(dir.path(), &registry).await;
        assert!(result.is_err());
    }
}
