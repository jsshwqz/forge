-- BILL-002: 费率与账单（build_v60b.md AF-BP-V60B；同时内嵌于 storage::MIGRATIONS）

CREATE TABLE IF NOT EXISTS rates (
    tenant_id         TEXT NOT NULL,
    kind              TEXT NOT NULL,
    unit_price_micros BIGINT NOT NULL,
    currency          TEXT NOT NULL DEFAULT 'CNY',
    PRIMARY KEY (tenant_id, kind)
);

CREATE TABLE IF NOT EXISTS bills (
    id           BIGSERIAL PRIMARY KEY,
    tenant_id    TEXT NOT NULL,
    period_from  TIMESTAMPTZ NOT NULL,
    period_to    TIMESTAMPTZ NOT NULL,
    doc          JSONB NOT NULL,
    doc_hash     TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, period_from, period_to)
);
