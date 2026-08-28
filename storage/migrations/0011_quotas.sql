-- TEN-003: 租户配额表
CREATE TABLE IF NOT EXISTS quotas (
    tenant_id TEXT PRIMARY KEY REFERENCES tenants(id),
    max_concurrent INT NOT NULL DEFAULT 4,
    daily_tasks INT NOT NULL DEFAULT 100
);

-- 种子化默认配额
INSERT INTO quotas (tenant_id, max_concurrent, daily_tasks)
VALUES ('default', 4, 100)
ON CONFLICT (tenant_id) DO NOTHING;
