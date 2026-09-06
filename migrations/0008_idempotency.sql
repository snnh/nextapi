-- M10.2 请求幂等键（PLAN.md §4.2）：
-- 非流式请求与异步图片任务的 Idempotency-Key 去重；
-- 重放不重复计费；过期由周期任务清理（见 main.rs / idempotency.rs）。

CREATE TABLE IF NOT EXISTS idempotency_keys (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    key_id          UUID NOT NULL REFERENCES api_keys(id) ON DELETE CASCADE,
    idem_key        TEXT NOT NULL,
    -- 请求体规范化（键排序）后的 sha256 hex；不一致 → 409
    fingerprint     TEXT NOT NULL,
    -- processing | completed | failed（failed 允许同键接管重试）
    status          TEXT NOT NULL DEFAULT 'processing',
    -- completed 时的缓存响应体（非流式完整 JSON / 异步任务 {"task_id":...}）
    response        JSONB,
    response_status INTEGER,
    -- 异步图片任务 id（media_tasks.id）
    task_id         UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ NOT NULL,
    UNIQUE (key_id, idem_key)
);

-- 过期清理扫描索引
CREATE INDEX IF NOT EXISTS idx_idem_expires ON idempotency_keys (expires_at);
