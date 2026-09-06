-- M10.1 模型别名（PLAN.md §4.1）：
-- alias → model 映射，入口接受别名、路由匹配/白名单/计价均按解析后的实际模型；
-- usage_logs 增加 requested_model 保留客户端原始入口模型（未走别名为 NULL）。

CREATE TABLE IF NOT EXISTS model_aliases (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    alias       TEXT NOT NULL UNIQUE,
    model       TEXT NOT NULL,
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 分区父表加列自动传播到全部分区与 DEFAULT 分区（PG 15+）
ALTER TABLE usage_logs ADD COLUMN IF NOT EXISTS requested_model TEXT;
