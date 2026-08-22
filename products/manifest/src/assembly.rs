//! 产品装配：Manifest + 注册表 → 完整可运行的 ProductAssembly。
//!
//! 落实 AP-015：装配只组合已有能力，绝不修改能力本身或核心对象。

use crate::manifest::{check_manifest, ProductManifest};
use forge_cap::{Capability, CapabilityRegistry};
use forge_core::{ForgeError, ForgeResult};
use forge_gates::{GatePolicy, GateSpec};
use serde::{Deserialize, Serialize};

/// 产品装配结果。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProductAssembly {
    /// 产品清单。
    pub manifest: ProductManifest,
    /// 全部解析成功的能力实体。
    pub resolved_capabilities: Vec<Capability>,
    /// 以 manifest 验收生成的门禁规格。
    pub gate_spec: GateSpec,
}

/// 装配产品。
///
/// 流程：check_manifest → 解析全部能力实体 → 组装。
///
/// - 缺必填能力 → `DependencyMissing(列出缺失项)`
/// - 装配产物是只读快照：后续注册表变化不影响已产出的 Assembly
pub async fn assemble(
    manifest: &ProductManifest,
    registry: &dyn CapabilityRegistry,
) -> ForgeResult<ProductAssembly> {
    // 校验清单
    let missing = check_manifest(manifest, registry).await?;
    if !missing.is_empty() {
        let names: Vec<String> = missing
            .iter()
            .map(|m| format!("{} v{}", m.capability_name, m.version))
            .collect();
        return Err(ForgeError::DependencyMissing(format!(
            "missing required capabilities: {}",
            names.join(", ")
        )));
    }

    // 解析全部能力实体
    let mut resolved = Vec::new();
    for cap_ref in &manifest.capabilities {
        let found = registry
            .find_by_name(&cap_ref.capability_name)
            .await
            .unwrap_or_default();
        let matched = found.into_iter().find(|c| c.version == cap_ref.version);
        match matched {
            Some(cap) => resolved.push(cap),
            None => {
                if cap_ref.required {
                    return Err(ForgeError::DependencyMissing(format!(
                        "failed to resolve capability: {} v{}",
                        cap_ref.capability_name, cap_ref.version
                    )));
                }
                // 可选能力缺失，跳过
            }
        }
    }

    // 构建门禁规格（以 manifest 中的能力引用 ID 作为验收标准）
    let gate_spec = GateSpec {
        task_id: forge_core::TaskId(manifest.id.0.clone()),
        required_criterion_ids: manifest
            .capabilities
            .iter()
            .filter(|c| c.required)
            .map(|c| c.capability_name.clone())
            .collect(),
        policy: GatePolicy::AllPass,
    };

    Ok(ProductAssembly {
        manifest: manifest.clone(),
        resolved_capabilities: resolved,
        gate_spec,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::CapabilityRef;
    use forge_agent::AgentRole;
    use forge_cap::{
        Capability, CapabilityKind, CapabilityStatus, InMemoryCapabilityRegistry,
    };
    use forge_core::ProductId;
    use forge_exec::PermissionLevel;

    fn make_cap(name: &str, version: &str) -> Capability {
        Capability {
            id: forge_core::new_capability_id(),
            name: name.into(),
            kind: CapabilityKind::Tool,
            version: version.into(),
            entry: "/path".into(),
            status: CapabilityStatus::Registered,
            permission: PermissionLevel::ReadOnly,
        }
    }

    fn make_manifest(caps: Vec<CapabilityRef>) -> ProductManifest {
        ProductManifest {
            id: ProductId::new_product_id(),
            name: "test-product".into(),
            version: "1.0.0".into(),
            description: "test".into(),
            capabilities: caps,
            entry_agent_role: AgentRole::Orchestrator,
        }
    }

    #[tokio::test]
    async fn test_assemble_success() {
        let reg = InMemoryCapabilityRegistry::default();
        reg.register(make_cap("echo", "1.0.0")).await.unwrap();
        reg.register(make_cap("write", "1.0.0")).await.unwrap();

        let manifest = make_manifest(vec![
            CapabilityRef {
                capability_name: "echo".into(),
                version: "1.0.0".into(),
                required: true,
            },
            CapabilityRef {
                capability_name: "write".into(),
                version: "1.0.0".into(),
                required: true,
            },
        ]);

        let assembly = assemble(&manifest, &reg).await.unwrap();
        assert_eq!(assembly.resolved_capabilities.len(), 2);
    }

    #[tokio::test]
    async fn test_assemble_missing_required() {
        let reg = InMemoryCapabilityRegistry::default();
        reg.register(make_cap("echo", "1.0.0")).await.unwrap();

        let manifest = make_manifest(vec![
            CapabilityRef {
                capability_name: "echo".into(),
                version: "1.0.0".into(),
                required: true,
            },
            CapabilityRef {
                capability_name: "missing".into(),
                version: "1.0.0".into(),
                required: true,
            },
        ]);

        let result = assemble(&manifest, &reg).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing"));
    }

    #[tokio::test]
    async fn test_assemble_snapshot_immutable() {
        let reg = InMemoryCapabilityRegistry::default();
        reg.register(make_cap("echo", "1.0.0")).await.unwrap();

        let manifest = make_manifest(vec![CapabilityRef {
            capability_name: "echo".into(),
            version: "1.0.0".into(),
            required: true,
        }]);

        let assembly = assemble(&manifest, &reg).await.unwrap();
        let original_count = assembly.resolved_capabilities.len();

        // 向注册表追加新能力
        reg.register(make_cap("new-cap", "1.0.0")).await.unwrap();

        // Assembly 内容不变
        assert_eq!(assembly.resolved_capabilities.len(), original_count);
    }
}
