//! forge-product-instance：产品实例生命周期 + 模板库（V4.0，PROD-001/002）。

pub mod instance;
pub mod templates;

pub use instance::{
    new_product_instance_id, ProductInstance, ProductInstanceStore, ProductState,
    InMemoryProductInstanceStore,
};
pub use templates::{
    TemplateRecord, TemplateRegistry, InMemoryTemplateRegistry,
};

#[cfg(test)]
mod tests {
    use super::*;
    use forge_agent::AgentRole;
    use forge_core::ProductId;
    use forge_product::{CapabilityRef, ProductManifest, ProductTemplate};

    fn sample_template(id: &str) -> ProductTemplate {
        ProductTemplate {
            id: id.into(),
            name: format!("template {id}"),
            parameters: vec![],
            manifest_skeleton: ProductManifest {
                id: ProductId::new_product_id(),
                name: "demo".into(),
                version: "0.1.0".into(),
                description: "demo product".into(),
                capabilities: vec![CapabilityRef {
                    capability_name: "echo".into(),
                    version: "0.1.0".into(),
                    required: true,
                }],
                entry_agent_role: AgentRole::Orchestrator,
            },
        }
    }

    fn sample_instance() -> ProductInstance {
        ProductInstance {
            id: new_product_instance_id(),
            template_id: "tpl.demo".into(),
            template_version: "1.0.0".into(),
            name: "demo-1".into(),
            params: Default::default(),
            state: ProductState::Draft,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    // ---- PROD-001 状态机 ----

    #[test]
    fn happy_path_lifecycle() {
        let mut i = sample_instance();
        i.transition(ProductState::Active).unwrap();
        assert_eq!(i.state, ProductState::Active);
        i.transition(ProductState::Stopped).unwrap();
        i.transition(ProductState::Active).unwrap(); // restart 合法
        i.transition(ProductState::Stopped).unwrap();
        i.transition(ProductState::Deprecated).unwrap();
    }

    #[test]
    fn illegal_transitions_rejected() {
        let mut i = sample_instance();
        assert!(i.transition(ProductState::Stopped).is_err()); // Draft→Stopped
        assert!(i.transition(ProductState::Deprecated).is_ok()); // Draft→Deprecated 允许（弃用草稿）

        let mut a = sample_instance();
        a.transition(ProductState::Active).unwrap();
        assert!(a.transition(ProductState::Deprecated).is_err()); // Active→Deprecated

        let mut d = sample_instance();
        d.transition(ProductState::Deprecated).unwrap();
        assert!(d.transition(ProductState::Active).is_err()); // 终态不可复活
    }

    #[tokio::test]
    async fn store_crud_and_conflict() {
        let store = InMemoryProductInstanceStore::new();
        let i = sample_instance();
        store.insert(i.clone()).await.unwrap();
        assert!(store.insert(i.clone()).await.is_err(), "重复 insert 必须拒绝");
        assert_eq!(store.get(&i.id).await.unwrap().name, "demo-1");
        assert_eq!(store.list().await.unwrap().len(), 1);
        assert!(store.get("ghost").await.is_err());
    }

    // ---- PROD-002 模板库 ----

    fn rec(id: &str, ver: &str, verdict: &str) -> TemplateRecord {
        TemplateRecord {
            template: sample_template(id),
            version: ver.into(),
            review_verdict: verdict.into(),
            published_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn publish_requires_pass_or_concern() {
        let reg = InMemoryTemplateRegistry::new();
        assert!(reg.publish(rec("t", "1.0.0", "Reject")).await.is_err());
        assert!(reg.publish(rec("t", "1.0.0", "")).await.is_err());
        reg.publish(rec("t", "1.0.0", "Pass")).await.unwrap();
        reg.publish(rec("t", "1.1.0", "Concern")).await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_version_rejected_and_versions_listed() {
        let reg = InMemoryTemplateRegistry::new();
        reg.publish(rec("t", "1.0.0", "Pass")).await.unwrap();
        assert!(reg.publish(rec("t", "1.0.0", "Pass")).await.is_err());
        reg.publish(rec("t", "2.0.0", "Pass")).await.unwrap();

        let vs = reg.versions("t").await.unwrap();
        assert_eq!(vs.len(), 2);
        assert_eq!(vs[0].version, "1.0.0");
        assert_eq!(vs[1].version, "2.0.0");

        let got = reg.get("t", "2.0.0").await.unwrap();
        assert_eq!(got.review_verdict, "Pass");
        assert_eq!(reg.list().await.unwrap().len(), 2);
        assert!(reg.get("ghost", "1.0.0").await.is_err());
    }
}
