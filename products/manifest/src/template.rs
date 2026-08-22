//! 产品模板实例化。
//!
//! 参数化的产品骨架 + 取值填充 = 具体 Manifest。

use crate::manifest::ProductManifest;
use forge_core::{ForgeError, ForgeResult, ProductId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 模板参数。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemplateParam {
    /// 参数名，如 "product_name"。
    pub name: String,
    /// 是否必填。
    pub required: bool,
    /// 默认值。
    pub default: Option<String>,
}

/// 产品模板。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProductTemplate {
    /// 模板 ID。
    pub id: String,
    /// 模板名称。
    pub name: String,
    /// 参数列表。
    pub parameters: Vec<TemplateParam>,
    /// 清单骨架（name/description 内含占位符 `{{param}}`）。
    pub manifest_skeleton: ProductManifest,
}

/// 占位符替换：`{{param}}` 语法。
fn replace_placeholders(text: &str, values: &HashMap<String, String>) -> String {
    let mut result = text.to_string();
    for (key, val) in values {
        let placeholder = format!("{{{{{}}}}}", key);
        result = result.replace(&placeholder, val);
    }
    result
}

/// 实例化模板。
///
/// - 占位符语法：`{{param}}`
/// - 缺必填参数且无默认 → `InvalidState(参数名)`
/// - 多余参数忽略
/// - 替换发生在 manifest 的 name/description 字段
/// - 替换后必须重新生成新的 ProductId
pub fn instantiate(
    tpl: &ProductTemplate,
    values: &HashMap<String, String>,
) -> ForgeResult<ProductManifest> {
    let mut resolved_values: HashMap<String, String> = HashMap::new();

    for param in &tpl.parameters {
        match values.get(&param.name) {
            Some(v) => {
                resolved_values.insert(param.name.clone(), v.clone());
            }
            None => match &param.default {
                Some(d) => {
                    resolved_values.insert(param.name.clone(), d.clone());
                }
                None => {
                    if param.required {
                        return Err(ForgeError::InvalidState(format!(
                            "missing required parameter: {}",
                            param.name
                        )));
                    }
                }
            },
        }
    }

    let mut manifest = tpl.manifest_skeleton.clone();
    manifest.id = ProductId::new_product_id();
    manifest.name = replace_placeholders(&manifest.name, &resolved_values);
    manifest.description = replace_placeholders(&manifest.description, &resolved_values);

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_agent::AgentRole;

    fn make_template() -> ProductTemplate {
        ProductTemplate {
            id: "tpl-1".into(),
            name: "test-template".into(),
            parameters: vec![
                TemplateParam {
                    name: "product_name".into(),
                    required: true,
                    default: None,
                },
                TemplateParam {
                    name: "description".into(),
                    required: false,
                    default: Some("default desc".into()),
                },
            ],
            manifest_skeleton: ProductManifest {
                id: ProductId::new_product_id(),
                name: "Product: {{product_name}}".into(),
                version: "1.0.0".into(),
                description: "{{description}}".into(),
                capabilities: vec![],
                entry_agent_role: AgentRole::Orchestrator,
            },
        }
    }

    #[test]
    fn test_full_instantiation() {
        let tpl = make_template();
        let mut values = HashMap::new();
        values.insert("product_name".into(), "MyApp".into());
        values.insert("description".into(), "A great app".into());

        let manifest = instantiate(&tpl, &values).unwrap();
        assert_eq!(manifest.name, "Product: MyApp");
        assert_eq!(manifest.description, "A great app");
    }

    #[test]
    fn test_missing_required() {
        let tpl = make_template();
        let values = HashMap::new();

        let result = instantiate(&tpl, &values);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("product_name"));
    }

    #[test]
    fn test_default_value() {
        let tpl = make_template();
        let mut values = HashMap::new();
        values.insert("product_name".into(), "MyApp".into());
        // 不提供 description，使用默认值

        let manifest = instantiate(&tpl, &values).unwrap();
        assert_eq!(manifest.description, "default desc");
    }

    #[test]
    fn test_extra_params_ignored() {
        let tpl = make_template();
        let mut values = HashMap::new();
        values.insert("product_name".into(), "MyApp".into());
        values.insert("extra_param".into(), "ignored".into());

        let manifest = instantiate(&tpl, &values).unwrap();
        assert_eq!(manifest.name, "Product: MyApp");
    }

    #[test]
    fn test_two_instantiations_different_ids() {
        let tpl = make_template();
        let mut values = HashMap::new();
        values.insert("product_name".into(), "App1".into());

        let m1 = instantiate(&tpl, &values).unwrap();
        let m2 = instantiate(&tpl, &values).unwrap();
        assert_ne!(m1.id, m2.id);
    }
}
