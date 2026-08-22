//! 四级权限模型，对应执行纪律的风险分级。

use serde::{Deserialize, Serialize};

/// 权限级别，按风险从低到高排列。
///
/// - `ReadOnly`：只读操作，无副作用
/// - `WorkspaceWrite`：写入工作区文件
/// - `External`：访问外部资源（网络、系统调用等）
/// - `Irreversible`：不可逆操作（删除、发布等）
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PermissionLevel {
    ReadOnly,
    WorkspaceWrite,
    External,
    Irreversible,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ordering() {
        assert!(PermissionLevel::ReadOnly < PermissionLevel::WorkspaceWrite);
        assert!(PermissionLevel::WorkspaceWrite < PermissionLevel::External);
        assert!(PermissionLevel::External < PermissionLevel::Irreversible);
    }

    #[test]
    fn test_serde() {
        let level = PermissionLevel::Irreversible;
        let json = serde_json::to_string(&level).unwrap();
        let back: PermissionLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(level, back);
    }
}
