-- MKT-101: 发布者生态与签名（build_v60.md AF-BP-V60A；同时内嵌于 storage::MIGRATIONS）

CREATE TABLE IF NOT EXISTS publisher_keys (
    publisher_id TEXT PRIMARY KEY,
    public_key   TEXT NOT NULL,              -- hex 编码 32 字节
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS releases (
    id           BIGSERIAL PRIMARY KEY,
    name         TEXT NOT NULL,
    version      TEXT NOT NULL,
    publisher_id TEXT NOT NULL REFERENCES publisher_keys(publisher_id),
    package_hash TEXT NOT NULL,              -- sha256 hex
    signature    TEXT NOT NULL,              -- hex 编码 64 字节
    review_status TEXT NOT NULL DEFAULT 'pending',  -- pending|approved|rejected|published
    UNIQUE(name, version, publisher_id)
);
