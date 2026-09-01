-- M5 计价引擎：price_rules（PLAN §6 / 契约 contracts/m5-billing.md §2）。
-- M5-A 归属。fx_rates 已在 0001_init.sql 建好（勿重建），此处仅新增价格规则表。

CREATE TABLE IF NOT EXISTS price_rules (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  upstream_id UUID NOT NULL REFERENCES upstreams(id) ON DELETE CASCADE,
  model_id TEXT NOT NULL,                    -- 网关模型 ID，精确匹配
  unit TEXT NOT NULL,                        -- token_in|token_out|token_cache_write|token_cache_read|image|video_second
  currency TEXT NOT NULL,                    -- CNY|USD
  base_price NUMERIC(20,10) NOT NULL,
  source TEXT NOT NULL DEFAULT 'manual',     -- manual|import
  dimensions JSONB,                          -- 图片 {image_size} / 视频 {resolution, task_type}
  dimension_key TEXT NOT NULL DEFAULT '',    -- 归一化（按键排序 JSON；空=''）
  segments JSONB,                            -- 有序分段 [{name?,price,weekdays?,windows?,min_prompt_tokens?,max_prompt_tokens?}]
  context_basis TEXT NOT NULL DEFAULT 'prompt_tokens',  -- prompt_tokens|total_tokens
  effective_from TIMESTAMPTZ, effective_to TIMESTAMPTZ,
  priority INT NOT NULL DEFAULT 10, sort_order INT NOT NULL DEFAULT 0,
  enabled BOOL NOT NULL DEFAULT TRUE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (upstream_id, model_id, unit, currency, dimension_key),
  CHECK (unit IN ('token_in','token_out','token_cache_write','token_cache_read','image','video_second')),
  CHECK (currency IN ('CNY','USD')),
  CHECK (source IN ('manual','import')),
  CHECK (context_basis IN ('prompt_tokens','total_tokens')),
  CHECK (base_price >= 0)
);

CREATE INDEX IF NOT EXISTS idx_price_rules_key ON price_rules (upstream_id, model_id) WHERE enabled;
