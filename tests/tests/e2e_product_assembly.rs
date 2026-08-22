//! M4 里程碑门禁：产品工厂端到端集成测试。
//!
//! 测试剧本：
//! 1. 临时目录写入合法 skill.json → load_skill_into 注册成功
//! 2. 再注册一个 Tool 能力，注册表共 2 项
//! 3. 模板实例化出 Manifest（引用这 2 项能力）
//! 4. check_manifest 返回空缺失列表
//! 5. assemble 成功，resolved 实体数=2
//! 6. 移除 1 个必填能力后重新装配 → DependencyMissing

use forge_agent::AgentRole;
use forge_cap::{
    Capability, CapabilityKind, CapabilityRegistry, CapabilityStatus,
    InMemoryCapabilityRegistry,
};
use forge_exec::PermissionLevel;
use forge_product::{
    assemble, check_manifest, instantiate, CapabilityRef, ProductManifest, ProductTemplate,
    TemplateParam,
};
use forge_skill::load_skill_into;
use std::collections::HashMap;
use std::io::Write;

#[tokio::test]
async fn e2e_product_assembly() {
    // === 步骤 1：临时目录写入合法 skill.json → load_skill_into 注册成功 ===
    let dir = tempfile::tempdir().unwrap();
    let skill_json = serde_json::json!({
        "name": "my-skill",
        "version": "1.0.0",
        "description": "test skill",
        "entry": "main.rs",
        "permission": "ReadOnly"
    });
    let mut f = std::fs::File::create(dir.path().join("skill.json")).unwrap();
    f.write_all(skill_json.to_string().as_bytes()).unwrap();

    let registry = InMemoryCapabilityRegistry::default();
    load_skill_into(dir.path(), &registry).await.unwrap();

    // === 步骤 2：再注册一个 Tool 能力 ===
    let tool_cap = Capability {
        id: forge_core::new_capability_id(),
        name: "echo-tool".into(),
        kind: CapabilityKind::Tool,
        version: "1.0.0".into(),
        entry: "/bin/echo".into(),
        status: CapabilityStatus::Registered,
        permission: PermissionLevel::ReadOnly,
    };
    registry.register(tool_cap).await.unwrap();

    // 注册表共 2 项
    let skills = registry.list_by_kind(CapabilityKind::Skill).await.unwrap();
    let tools = registry.list_by_kind(CapabilityKind::Tool).await.unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(tools.len(), 1);

    // === 步骤 3：模板实例化出 Manifest（引用这 2 项能力）===
    let template = ProductTemplate {
        id: "tpl-1".into(),
        name: "test-template".into(),
        parameters: vec![TemplateParam {
            name: "product_name".into(),
            required: true,
            default: None,
        }],
        manifest_skeleton: ProductManifest {
            id: forge_core::new_product_id(),
            name: "Product: {{product_name}}".into(),
            version: "1.0.0".into(),
            description: "test product".into(),
            capabilities: vec![
                CapabilityRef {
                    capability_name: "my-skill".into(),
                    version: "1.0.0".into(),
                    required: true,
                },
                CapabilityRef {
                    capability_name: "echo-tool".into(),
                    version: "1.0.0".into(),
                    required: true,
                },
            ],
            entry_agent_role: AgentRole::Orchestrator,
        },
    };

    let mut values = HashMap::new();
    values.insert("product_name".into(), "MyProduct".into());
    let manifest = instantiate(&template, &values).unwrap();
    assert_eq!(manifest.name, "Product: MyProduct");

    // === 步骤 4：check_manifest 返回空缺失列表 ===
    let missing = check_manifest(&manifest, &registry).await.unwrap();
    assert!(missing.is_empty(), "all capabilities should be satisfied");

    // === 步骤 5：assemble 成功，resolved 实体数=2 ===
    let assembly = assemble(&manifest, &registry).await.unwrap();
    assert_eq!(assembly.resolved_capabilities.len(), 2);

    // === 步骤 6：移除 1 个必填能力后重新装配 → DependencyMissing ===
    // 创建一个不含 echo-tool 的新清单
    let mut manifest_missing = manifest.clone();
    manifest_missing.id = forge_core::new_product_id(); // 新 ID
    manifest_missing.capabilities.retain(|c| c.capability_name != "echo-tool");

    // 这个清单只引用 my-skill，应该能装配成功
    let assembly2 = assemble(&manifest_missing, &registry).await.unwrap();
    assert_eq!(assembly2.resolved_capabilities.len(), 1);

    // 而原始清单现在仍然可以装配（因为能力仍在注册表中）
    // 测试真正的缺失：创建引用不存在能力的清单
    let manifest_bad = ProductManifest {
        id: forge_core::new_product_id(),
        name: "bad-product".into(),
        version: "1.0.0".into(),
        description: "test".into(),
        capabilities: vec![
            CapabilityRef {
                capability_name: "my-skill".into(),
                version: "1.0.0".into(),
                required: true,
            },
            CapabilityRef {
                capability_name: "nonexistent".into(),
                version: "1.0.0".into(),
                required: true,
            },
        ],
        entry_agent_role: AgentRole::Orchestrator,
    };

    let result = assemble(&manifest_bad, &registry).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("nonexistent"));
}
