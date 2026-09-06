-- BILL-001: 计量流水（build_v60b.md AF-BP-V60B；同时内嵌于 storage::MIGRATIONS）

CREATE TABLE IF NOT EXISTS usage_events (
    id        BIGSERIAL PRIMARY KEY,
    tenant_id TEXT NOT NULL DEFAULT 'default',
    kind      TEXT NOT NULL,
    quantity  BIGINT NOT NULL,
    meta      JSONB NOT NULL DEFAULT '{}'::jsonb,
    at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_usage_tenant_at ON usage_events(tenant_id, at);
