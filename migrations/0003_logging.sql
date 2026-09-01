-- M4 日志统计：usage_logs（声明式分区）+ usage_hourly（PLAN §6）。
-- M4-A 归属，契约 contracts/m4-logging.md §2。
--
-- 分区方案：idx = floor(unix_days / partition_days)，边界 = 1970-01-01 + idx*days 天，
-- 分区名 usage_logs_p{idx}。迁移内按默认 30 天预建「当前窗口 + 下一窗口」两个分区。

-- usage_logs：声明式分区（PARTITION BY RANGE (ts)），分区粒度 = 30 天，epoch 对齐。
CREATE SEQUENCE IF NOT EXISTS usage_logs_id_seq;   -- 全局共享序列

CREATE TABLE IF NOT EXISTS usage_logs (
  id BIGINT NOT NULL DEFAULT nextval('usage_logs_id_seq'),
  request_id TEXT NOT NULL,
  ts TIMESTAMPTZ NOT NULL,
  key_id UUID NULL,
  model TEXT NOT NULL DEFAULT '',
  upstream_id UUID NULL,
  protocol_in TEXT NOT NULL,
  protocol_out TEXT NOT NULL,
  convert_mode TEXT NOT NULL,                -- passthrough|convert|passthrough_fallback
  stream BOOL NOT NULL DEFAULT FALSE,
  prompt_tokens BIGINT,
  completion_tokens BIGINT,
  cache_write_tokens BIGINT,
  cache_read_tokens BIGINT,
  images INT,
  image_size TEXT,
  video_seconds NUMERIC(10,2),
  video_resolution TEXT,
  video_task_type TEXT,
  latency_ms INT,
  status INT NOT NULL,
  error TEXT,                                 -- error ≤2000 字符摘要
  retry_count INT NOT NULL DEFAULT 0,
  ttfb_ms INT,                                -- v1.16 补充（可观测）
  degraded BOOL NOT NULL DEFAULT FALSE,       -- v1.16 补充
  pricing_source TEXT NULL,
  cost_cny NUMERIC(20,10),
  cost_usd NUMERIC(20,10),
  price_used JSONB,
  fx_snapshot JSONB,
  usage_raw JSONB,
  debug_payload JSONB NULL,
  PRIMARY KEY (id, ts),
  UNIQUE (request_id, ts)
) PARTITION BY RANGE (ts);

CREATE INDEX IF NOT EXISTS idx_usage_logs_ts ON usage_logs (ts DESC);
CREATE INDEX IF NOT EXISTS idx_usage_logs_key_ts ON usage_logs (key_id, ts DESC);
CREATE INDEX IF NOT EXISTS idx_usage_logs_model_ts ON usage_logs (model, ts);
CREATE INDEX IF NOT EXISTS idx_usage_logs_upstream_ts ON usage_logs (upstream_id, ts DESC);

-- 迁移内按 30 天建「当前窗口 + 下一窗口」两个分区（幂等；epoch 对齐，边界即 to_timestamp(idx*days*86400)）。
DO $$
DECLARE
  days INT := 30;
  idx BIGINT;
  p_from TIMESTAMPTZ;
  p_to TIMESTAMPTZ;
BEGIN
  idx := floor(extract(epoch FROM now()) / 86400 / days)::bigint;
  FOR i IN 0..1 LOOP
    p_from := to_timestamp(idx * days * 86400);
    p_to := to_timestamp((idx + 1) * days * 86400);
    EXECUTE format(
      'CREATE TABLE IF NOT EXISTS usage_logs_p%s PARTITION OF usage_logs FOR VALUES FROM (%L) TO (%L)',
      idx, p_from, p_to
    );
    idx := idx + 1;
  END LOOP;
END $$;

CREATE TABLE IF NOT EXISTS usage_hourly (
  hour TIMESTAMPTZ NOT NULL,
  model TEXT NOT NULL,
  requests BIGINT NOT NULL DEFAULT 0,
  errors BIGINT NOT NULL DEFAULT 0,
  prompt_tokens BIGINT NOT NULL DEFAULT 0,
  completion_tokens BIGINT NOT NULL DEFAULT 0,
  cost_cny NUMERIC(20,10),
  cost_usd NUMERIC(20,10),
  PRIMARY KEY (hour, model)
);
