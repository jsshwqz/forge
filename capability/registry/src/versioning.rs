//! V6.0 MKT-102：版本治理（build_v60.md AF-BP-V60A 契约）。
//!
//! R3：约束语法仅支持 semver crate 原生 `VersionReq`，禁止自造语法。
//! R1：yanked 对所有路径不可见（resolve 与列表）；deprecated 可见但 resolve 排除。

use semver::{Version, VersionReq};

/// 版本及治理标记（yanked/deprecated 标记来自 releases 表）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionMeta {
    pub version: Version,
    pub yanked: bool,
    pub deprecated: bool,
}

impl VersionMeta {
    pub fn clean(version: Version) -> Self {
        Self { version, yanked: false, deprecated: false }
    }
}

/// 从可用版本中按约束解析出最高兼容版本（契约薄形态：调用方保证 available
/// 已排除 yanked/deprecated）。
pub fn resolve_version(constraint: &str, available: &[Version]) -> Option<Version> {
    let metas: Vec<VersionMeta> = available.iter().map(|v| VersionMeta::clean(v.clone())).collect();
    resolve_version_meta(constraint, &metas)
}

/// 带治理标记的解析：yanked 一律不可见；deprecated 参与列表但 resolve 排除。
pub fn resolve_version_meta(constraint: &str, available: &[VersionMeta]) -> Option<Version> {
    let req = VersionReq::parse(constraint).ok()?;
    available
        .iter()
        .filter(|m| !m.yanked && !m.deprecated)
        .filter(|m| req.matches(&m.version))
        .map(|m| m.version.clone())
        .max()
}

/// 升级判定：current 钉版（constraint 精确相等）→ 永不升级（None）；
/// caret/tilde/star 等范围约束 → 返回解析结果（调用方与 current 比较决定行动）。
pub fn should_upgrade(
    current: &Version,
    constraint: &str,
    available: &[VersionMeta],
) -> Option<Version> {
    // 钉版判定：constraint 可解析为精确 Version 且等于 current
    if let Ok(pinned) = constraint.parse::<Version>() {
        if &pinned == current {
            return None;
        }
    }
    resolve_version_meta(constraint, available)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(list: &[&str]) -> Vec<Version> {
        list.iter().map(|s| Version::parse(s).unwrap()).collect()
    }

    /// 冻结测试（MKT-102）：[0.1.0,0.1.2,0.2.0] 约束 "^0.1" → 0.1.2。
    #[test]
    fn highest_compatible_wins() {
        let available = v(&["0.1.0", "0.1.2", "0.2.0"]);
        let got = resolve_version("^0.1", &available).unwrap();
        assert_eq!(got, Version::parse("0.1.2").unwrap());
    }

    /// 冻结测试（MKT-102）：yanked 版本不出现在 resolve 与列表（meta 形态）。
    #[test]
    fn yanked_excluded_everywhere() {
        let available = vec![
            VersionMeta { version: Version::parse("0.1.0").unwrap(), yanked: false, deprecated: false },
            VersionMeta { version: Version::parse("0.1.2").unwrap(), yanked: true, deprecated: false },
            VersionMeta { version: Version::parse("0.1.9").unwrap(), yanked: false, deprecated: true },
        ];
        // ^0.1 的最高可用是 0.1.9（deprecated）与 0.1.2（yanked）——均排除 → 0.1.0
        let got = resolve_version_meta("^0.1", &available).unwrap();
        assert_eq!(got, Version::parse("0.1.0").unwrap());
        // 精确钉 0.1.2（yanked）→ resolve 不返回它
        assert!(resolve_version_meta("0.1.2", &available).is_none());
    }

    /// 冻结测试（MKT-102）：钉 "0.1.0" 且存在 0.1.2 → should_upgrade 返回 None。
    #[test]
    fn pinned_version_not_upgraded() {
        let current = Version::parse("0.1.0").unwrap();
        let available = vec![
            VersionMeta::clean(Version::parse("0.1.0").unwrap()),
            VersionMeta::clean(Version::parse("0.1.2").unwrap()),
        ];
        assert!(should_upgrade(&current, "0.1.0", &available).is_none());
    }

    /// 冻结测试（MKT-102）："*" → 最高非 yanked 版。
    #[test]
    fn star_constraint_resolves_latest() {
        let available = vec![
            VersionMeta::clean(Version::parse("0.1.0").unwrap()),
            VersionMeta { version: Version::parse("0.2.0").unwrap(), yanked: true, deprecated: false },
            VersionMeta::clean(Version::parse("0.1.5").unwrap()),
        ];
        let got = resolve_version_meta("*", &available).unwrap();
        assert_eq!(got, Version::parse("0.1.5").unwrap(), "yanked 的 0.2.0 必须被跳过");
    }

    #[test]
    fn invalid_constraint_is_none() {
        let available = v(&["0.1.0"]);
        assert!(resolve_version("not-a-req!!", &available).is_none());
    }
}
