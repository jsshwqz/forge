-- FED-001: 多副本任务队列（build_v60.md AF-BP-V60A；同时内嵌于 storage::MIGRATIONS，
-- 见 R7-010 教训：独立迁移文件不会被任何路径应用）

CREATE TABLE IF NOT EXISTS task_queue (
    id           BIGSERIAL PRIMARY KEY,
    task_id      TEXT NOT NULL,
    tenant_id    TEXT NOT NULL DEFAULT 'default',
    payload      JSONB NOT NULL,
    status       TEXT NOT NULL DEFAULT 'pending',  -- pending|claimed|done|failed
    claimed_by   TEXT,
    claimed_at   TIMESTAMPTZ,
    lease_expires_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_task_queue_claim
    ON task_queue(status, lease_expires_at) WHERE status IN ('pending','claimed');
