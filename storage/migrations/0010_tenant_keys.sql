-- TEN-002: 多租户密钥表
CREATE TABLE IF NOT EXISTS tenant_keys (
    tenant_id TEXT NOT NULL REFERENCES tenants(id),
    key_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, key_hash)
);
