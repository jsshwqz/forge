-- MKT-104: 制品库——存储制品字节引用 (D5=方案B 文件系统)
-- 制品字节不入库 (存于 $FORGE_ARTIFACT_DIR), PG 只存引用元数据。
-- artifact_path 存储 ArtifactId 字符串 (通过 ArtifactStore trait 的 read 方法检索制品字节)。

ALTER TABLE releases ADD COLUMN IF NOT EXISTS artifact_path TEXT;
ALTER TABLE releases ADD COLUMN IF NOT EXISTS artifact_size BIGINT;
ALTER TABLE releases ADD COLUMN IF NOT EXISTS artifact_sha256 TEXT;
