-- M6 媒体任务：media_tasks（PLAN §6 / 契约 contracts/m6-media.md §2）。
-- M6-A 归属。异步图片任务闭环（poller 轮询 + 计费幂等）；视频本期占位，schema 保留。

CREATE TABLE IF NOT EXISTS media_tasks (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  media_type TEXT NOT NULL,                  -- 'image'|'video'（video 本期不产生行）
  gateway_key_id UUID NOT NULL,
  model TEXT NOT NULL,                       -- 网关模型 ID
  upstream_id UUID NOT NULL,
  provider_task_id TEXT,                     -- 供应商任务 ID（同步响应无）
  status TEXT NOT NULL DEFAULT 'pending',    -- pending|processing|succeeded|failed|timeout
  resolution TEXT, duration_seconds NUMERIC(10,2), task_type TEXT,
  image_count INT, image_size TEXT,
  cost_cny NUMERIC(20,10), cost_usd NUMERIC(20,10),
  billing_key TEXT,                          -- 计费幂等键：{upstream_id}:{provider_task_id}
  error TEXT,
  raw JSONB,                                 -- 上游原始响应（任务查询结果）
  request_id TEXT,                           -- 创建请求网关 request_id（溯源）
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  finished_at TIMESTAMPTZ,
  UNIQUE (upstream_id, provider_task_id),
  UNIQUE (billing_key),
  CHECK (media_type IN ('image','video')),
  CHECK (status IN ('pending','processing','succeeded','failed','timeout'))
);
CREATE INDEX IF NOT EXISTS idx_media_tasks_status ON media_tasks (status) WHERE status IN ('pending','processing');
CREATE INDEX IF NOT EXISTS idx_media_tasks_key ON media_tasks (gateway_key_id, created_at DESC);
