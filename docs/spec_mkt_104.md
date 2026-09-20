# MKT-104S 制品库与安装闭环规格书

> 状态: 草案 (待 D5 拍板后拆单实施)
> 批次: MKT-104S (规格草案批, 纯文档零代码)
> 基线: master HEAD=56fc3bd
> 编写: GLM (执行方)
> 复核: 千问 (Windows 本机)

---

## S1 现状断链盘点

### 已存在的环

| 环 | 位置 | 语义 | 状态 |
|---|---|---|---|
| 发布者密钥登记 | `storage/migrations/0013_market_releases.sql` L1-4 | `publisher_keys(publisher_id PK, public_key TEXT)` | ✅ 在位 |
| Release 元数据表 | `storage/migrations/0013_market_releases.sql` L6-14 | `releases(id, name, version, publisher_id, package_hash, signature, review_status)` | ✅ 在位 |
| 版本治理 | `storage/migrations/0014_release_governance.sql` L1-3 | `ALTER TABLE releases ADD deprecated, yanked` | ✅ 在位 |
| 入站验签 (publish) | `server/src/routes/market.rs` L259-263 | `signing::verify_package(public_key, package_bytes, signature)` 验 `{name}\n{version}\n{package_hash}` | ✅ 在位 |
| 出站复验 (install) | `server/src/routes/market.rs` L383-410 | `install_signature_recheck()` 从 releases 表取 signature+package_hash+public_key 重验 | ✅ 在位 |
| 发布路由 | `server/src/routes/market.rs` L230 `publish_release` | POST /market/publish, Publisher-Key 头, 验签后 INSERT releases | ✅ 在位 |
| 审核路由 | `server/src/routes/market.rs` L295 `review_release` | POST /market/review, pending→approved/rejected/published | ✅ 在位 |
| 目录列表 | `server/src/routes/market.rs` L343 `list_releases` | GET /market/releases?name=, yanked 不可见 | ✅ 在位 |
| 安装路由 | `server/src/routes/market.rs` L126 `install_capability` | POST /market/install, semver resolve + yanked 检查 + 签名复验 + set_status(Active) | ✅ 在位 |
| 签名工具 | `capability/registry/src/signing.rs` L1-30 | ed25519 签名/验签, 私钥永不入库 | ✅ 在位 |

### 断裂的环 (缺口)

| 缺口 | 现状 | 影响 |
|---|---|---|
| **D-1 制品字节存储** | `releases` 表只有 `package_hash` (TEXT, sha256 hex), 无制品字节列, 无 artifact 存储 | 发布者声称 hash 但无法上传真实制品; 安装者无法获取可运行制品 |
| **D-2 上传接口** | `PublishRequest` (market.rs L219-224) 只有 `name, version, package_hash, signature` 四字段, 无包体数据 | 发布流程只存元数据, 制品无处可放 |
| **D-3 下载接口** | 无 GET /market/releases/{id}/download 或等价路由 | 安装者无法拉取制品字节 |
| **D-4 安装获取制品** | `install_capability` (market.rs L126-205) 只做内存注册表 `set_status(Active)`, 不获取制品字节 | "安装"本质只是改状态标记, 不产生可运行产物 |
| **D-5 hash 复核** | `install_signature_recheck` (market.rs L383-410) 复验的是**元数据签名** (`{name}\n{version}\n{package_hash}`), 不是制品字节本身的 sha256 | 签名覆盖的是 hash 字符串, 非制品内容; 篡改制品字节不会被检测 |
| **D-6 无 PG 降级** | `publish_release` (market.rs L236) 和 `list_releases` (market.rs L348) 在 `pool=None` 时返回 503 | 单机无 PG 时市场功能全禁; 无文件系统降级路径 |

### 断链示意

```
发布者                  Forge Server                 安装者
  │                        │                           │
  │── POST /market/publish ──▶                        │
  │   (name, version,      │                           │
  │    package_hash,       │                           │
  │    signature)          │                           │
  │                        │── 验签 OK ──▶ INSERT      │
  │                        │   releases(...)           │
  │                        │                           │
  │   ✗ 无制品字节上传      │                           │
  │                        │                           │
  │                        │◀── POST /market/install ──│
  │                        │    (name, version)        │
  │                        │── 签名复验 (元数据) ──     │
  │                        │── set_status(Active)      │
  │                        │   ✗ 无制品字节下载         │
  │                        │   ✗ 无制品 hash 复核       │
  │                        │─── {id, installed: true} ─▶│
  │                        │                           │
  │                        │    安装者拿到的是内存状态   │
  │                        │    标记, 非可运行制品       │
```

---

## S2 D5 存储后端对比决策表 (呈给项目所有人拍板, 禁代拍)

### 方案 A: PG large object / bytea

| 维度 | 评价 |
|---|---|
| 事务一致性 | ✅ 制品字节与 release 元数据同一事务, INSERT 成功即制品可查, 失败即全回滚 |
| 部署依赖 | ⚠️ 单机无 PG 不可用; 现有 publish/list/install 已 503 降级, 行为一致 |
| 大小上限 | bytea 理论 1GB, 实际受 `max_allowed_packet` / shared_buffers 约束; large object 理论 4TB 但管理复杂 |
| 备份恢复 | ✅ 随 PG 逻辑备份/物理备份一体完成, 无独立备份面 |
| 与 PgEvidenceStore 复用度 | 中: PgEvidenceStore (storage/src/pg_evidence.rs L33) 用 TEXT 列存 content (证据是文本); 制品是二进制, 需 bytea 而非 TEXT, 模式不完全复用 |
| 已有先例 | roadmap_v7.md 原建议倾向 (工单注明) |
| 实施成本 | 低-中: 加 bytea 列 + 上传/下载 SQL; 无新 crate 依赖 (sqlx 已支持 bytea) |

### 方案 B: 本地文件系统 + content-hash 目录

| 维度 | 评价 |
|---|---|
| 事务一致性 | ⚠️ 文件写入与 PG 元数据不在同一事务; 需补偿策略 (先写文件后插 PG, 失败时清理孤儿文件) |
| 部署依赖 | ✅ 单机无 PG 也可用 (对齐 FileKnowledgeBase 的 env 门控先例, `FORGE_KNOWLEDGE_FILE`) |
| 大小上限 | ✅ 受磁盘空间约束, 无 PG 参数限制 |
| 备份恢复 | ⚠️ 需独立备份制品目录; PG 备份不含制品字节 |
| 与 FileKnowledgeBase 复用度 | 高: 同 env 门控 + 路径收敛模式 (`$FORGE_ARTIFACT_DIR` 对齐 `$FORGE_KNOWLEDGE_FILE`); resolve_in_root 沙箱可复用 |
| 已有先例 | FileKnowledgeBase (knowledge/src/failures_file.rs), WriteFileTool (execution/runtime/src/tools_file.rs resolve_in_root) |
| 实施成本 | 低: 新建 ArtifactStore trait + 文件系统实现; 无新 crate 依赖 (std::fs) |

### 执行方推荐

**推荐方案 B (本地文件系统 + content-hash 目录)**, 理由:

1. 与 KNOW-001A 刚落地的 `FileKnowledgeBase` / `FORGE_KNOWLEDGE_PERSIST` env 门控先例完全对齐, 模式成熟
2. 单机无 PG 可用——对齐项目 "缺省单机 serve" 定位 (KNOW-001A 已翻转缺省为文件持久)
3. 复用 `resolve_in_root` 路径沙箱 (execution/runtime/src/tools_file.rs), 安全面已验证
4. 无新依赖 (std::fs), 实施成本低

**但标注: 待 D5 拍板。** 若项目所有人选 A, 上面的实施细节按 bytea 调整, 接口形状不变。

---

## S3 接口冻结草案

### 上传

```
POST /market/publish
Headers: Publisher-Key: <publisher_id>, Content-Type: application/octet-stream
Body: 制品字节 (raw binary)

Query: ?name=<name>&version=<version>&signature=<hex_sig>
```

或保持 JSON + base64:

```
POST /market/publish
Headers: Publisher-Key: <publisher_id>
Body: { "name", "version", "package_hash", "signature", "package_data": "<base64>" }
```

**建议**: 采用 JSON + base64 (与现有 PublishRequest 结构兼容, 加一个 optional `package_data` 字段), 避免二进制路由的 Content-Type 复杂性。大小上限由 `FORGE_PACKAGE_MAX_BYTES` (默认 16MB) 控制。

### 下载

```
GET /market/releases/{name}/{version}/download
→ 200 application/octet-stream (制品字节)
→ 404 (release 不存在)
→ 409 (yanked)
```

**权限**: 匿名可下载 (与 list_releases 公开只读对齐); 发布和安装需鉴权。

### 安装双校验

安装流程在 `install_capability` 中增加制品获取 + 双校验:

1. **hash 复核** (先): 下载制品字节 → sha256 → 与 releases.package_hash 比对 → 不匹配 → 409 "package hash mismatch"
2. **验签** (后): 现有 `install_signature_recheck` 继续复验元数据签名 → 失败 → 403

**顺序理由**: hash 是快速本地计算, 验签涉及 ed25519 公钥查找; 先快后慢, 快速拒绝篡改。

**失败语义**: hash 不匹配 = 篡改 (409); 验签失败 = 伪造 (403); 两者均不 install。

### 删除

```
DELETE /market/releases/{name}/{version}
Headers: Publisher-Key: <publisher_id>
→ 204 (删除成功, 制品字节 + 元数据同删)
→ 403 (非本人 release)
→ 404 (不存在)
```

### env 逃生阀

对齐 `FORGE_KNOWLEDGE_PERSIST` 模式:

| env | 默认 | 含义 |
|---|---|---|
| `FORGE_ARTIFACT_DIR` | `~/.aion-forge/artifacts/` | 制品存储目录 (方案 B) |
| `FORGE_PACKAGE_MAX_BYTES` | `16777216` (16MB) | 单制品大小上限 |
| `FORGE_ARTIFACT_BACKEND` | `file` (方案 B 时) | 存储后端选择: `file` / `pg` (未来扩展) |

---

## S4 数据模型

### 新 migration: 0017_release_artifacts.sql

```sql
-- MKT-104: 制品库——存储制品字节引用 (方案 B: 文件系统)
-- 制品字节不入库 (存于 $FORGE_ARTIFACT_DIR), PG 只存引用元数据。

ALTER TABLE releases ADD COLUMN IF NOT EXISTS artifact_path TEXT;
-- 方案 B: 相对 $FORGE_ARTIFACT_DIR 的路径 (如 "ab/cd/abcd1234....jsonl")
-- 方案 A (若 D5 选 A): 替换为 artifact_data BYTEA

ALTER TABLE releases ADD COLUMN IF NOT EXISTS artifact_size BIGINT;
-- 制品字节数, 用于下载 Content-Length

ALTER TABLE releases ADD COLUMN IF NOT EXISTS artifact_sha256 TEXT;
-- 冗余存储: 与 package_hash 一致 (package_hash 是发布者声称的 hash,
-- artifact_sha256 是服务端存储时实测的 hash, 二者比对 = D-5 修复)
```

**编号查重**: `ls storage/migrations/` 确认最大号 = 0016_billing.sql, 下一个 = 0017, 无撞号。

**禁改 0013 已有语义**: 新列全部 ADD COLUMN IF NOT EXISTS, 不改已有列定义。

---

## S5 测试矩阵设计

### 用例名冻结清单

| # | 用例名 | 类型 | 覆盖 |
|---|---|---|---|
| 1 | `upload_then_download_bytes_match` | async + timeout | 上传制品 → 下载 → 字节一致 |
| 2 | `upload_exceeds_max_bytes_rejected` | async + timeout | 超过 FORGE_PACKAGE_MAX_BYTES → 413 |
| 3 | `download_tampered_package_hash_mismatch` | async + timeout | 篡改制品文件 → 下载 hash 复核 → 409 |
| 4 | `install_with_hash_recheck_passes` | async + timeout | 正常制品 → hash 复核通过 → install 成功 |
| 5 | `install_with_hash_mismatch_rejected` | async + timeout | 制品被篡改 → hash 复核失败 → 409 |
| 6 | `install_with_bad_signature_rejected` | async + timeout | 验签失败 → 403 |
| 7 | `delete_removes_artifact_and_metadata` | async + timeout | DELETE → 制品文件 + PG 行同删 |
| 8 | `download_yanked_returns_409` | async + timeout | yanked release → 下载 → 409 |
| 9 | `upload_without_publisher_key_rejected` | async + timeout | 缺 Publisher-Key → 403 |
| 10 | `no_pg_fallback_to_file_system` | async + timeout | pool=None → 文件系统仍可上传/下载 (方案 B) |

### 纪律

- 异步用例全程 `tokio::time::timeout` 护栏 (R1-094 纪律)
- 文件类断言全程 tempfile 隔离, 禁写真 home (G4 探针口径)
- hash 复核测试用真实 sha256 计算, 不 mock
- 篡改测试: 上传后手动改文件内容, 再下载, 验证 hash 不匹配

---

## S6 门禁口径

自 HYGIENE-001 批起全量口径永久为:

```bash
# G1 clippy
cargo clippy --workspace --all-targets --features forge-mcp/server-bin -j 2 -- -D warnings

# G2 全量测试
cargo test --workspace --features forge-mcp/server-bin --no-fail-fast -j 2
```

本批 (MKT-104S) 为纯文档批, 无代码改动, 门禁数字应与基线一致 (不增不减)。实施工包 (104A/104B...) 下限 = 实跑基线 + 新增测试数, 基线数字由复核方本机测定为准。

---

## S7 拆单建议

| 单 | 名称 | 改动面 | 依赖 |
|---|---|---|---|
| MKT-104A | 装配面: 制品存储 + 上传/下载路由 | storage/ 新建 ArtifactStore trait + 文件实现; server/routes/market.rs 加 upload/download handler; migration 0017 | D5 拍板 |
| MKT-104B | 数据面: 安装双校验 + 删除 | server/routes/market.rs install_capability 加 hash 复核; 加 DELETE 路由 | 104A |
| MKT-104C | 验收: 测试矩阵 + e2e | server/tests/market_artifact.rs 10 用例; 全量门禁 | 104A + 104B |

**改动面互不重叠**: 104A 只加新路由 + 新 trait, 不改 install; 104B 只改 install + 加 DELETE, 不改 upload/download; 104C 只加测试, 不改 src/。

---

## S8 风险与待拍板清单

| # | 待决策项 | 说明 | 默认建议 |
|---|---|---|---|
| D5 | 存储后端: PG bytea (A) vs 文件系统 (B) | 见 S2 对比表 | 方案 B (文件系统) |
| D6 | 制品大小上限 | FORGE_PACKAGE_MAX_BYTES 默认值 | 16MB |
| D7 | 匿名下载是否允许 | 与 list_releases 公开只读对齐? | 允许 |
| D8 | 上传方式: JSON+base64 vs octet-stream | base64 与现有 PublishRequest 兼容 | JSON+base64 |
| D9 | 孤儿文件清理策略 (方案 B) | 文件写入成功但 PG INSERT 失败时的补偿 | 启动时扫描 + 定期清理 (低优先级) |
| D10 | 配额: 单 publisher 发布数量上限 | 当前无限制 | 暂不限制, 后续按需加 |
| D11 | 制品格式: 是否限定特定格式 (如 .tar.gz) | 当前规格不限制 | 不限制, hash 复核与格式无关 |

---

## 引用文件行号验证记录

所有引用的行号均在基线 `56fc3bd` 上实测核对:

| 文件 | 引用行号 | 核对结果 |
|---|---|---|
| `storage/migrations/0013_market_releases.sql` | L1-4 (publisher_keys), L6-14 (releases) | ✅ |
| `storage/migrations/0014_release_governance.sql` | L1-3 (deprecated, yanked) | ✅ |
| `server/src/routes/market.rs` | L41 (InstallRequest), L126 (install_capability), L219 (PublishRequest), L230 (publish_release), L259-263 (验签), L295 (review_release), L343 (list_releases), L383-410 (install_signature_recheck) | ✅ |
| `capability/registry/src/signing.rs` | L1-30 (ed25519 签名/验签) | ✅ |
| `storage/src/pg_evidence.rs` | L15 (PgEvidenceStore struct), L33 (TEXT 列存 content) | ✅ |
| `storage/src/s3.rs` | L1-10 (MinIO/S3 配置) | ✅ |
| `server/src/lib.rs` | L1333-1338 (market 路由注册) | ✅ |
| `storage/migrations/` 最大号 | 0016_billing.sql → 下一个 0017 | ✅ |
