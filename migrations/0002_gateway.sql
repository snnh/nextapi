-- M3 网关核心表：api_keys / upstreams / model_routes / quota_usage（PLAN.md §6）。

-- 网关 Key（LiteLLM virtual key 式；DB 只存 SHA-256 哈希 + 前 8 位前缀）
CREATE TABLE IF NOT EXISTS api_keys (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name              TEXT NOT NULL DEFAULT '',
    key_hash          TEXT UNIQUE NOT NULL,          -- sha256 hex
    prefix            TEXT NOT NULL,                 -- Key 前 8 位（展示用）
    enabled           BOOL NOT NULL DEFAULT TRUE,
    models            TEXT[] NULL,                   -- 模型白名单（通配符 *；NULL/空 = 全部）
    rpm               INT NULL,                      -- 每分钟请求数（NULL = 用系统默认）
    tpm               INT NULL,                      -- 每分钟 token 数
    quota_limit       NUMERIC(20,10) NULL,           -- 用量上限（仅控制，非余额）
    quota_unit        TEXT NULL,                     -- 'tokens'|'cost_cny'|'cost_usd'
    quota_window      TEXT NULL,                     -- 'daily'|'monthly'|'total'；total 不重置
    allow_upstream_passthrough BOOL NOT NULL DEFAULT FALSE,
    passthrough_upstreams TEXT[] NULL,               -- NULL/空 = 不限
    debug_enabled     BOOL NOT NULL DEFAULT FALSE,
    debug_expires_at  TIMESTAMPTZ,
    expires_at        TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at      TIMESTAMPTZ,
    CONSTRAINT api_keys_quota_unit_check CHECK (quota_unit IS NULL OR quota_unit IN ('tokens','cost_cny','cost_usd')),
    CONSTRAINT api_keys_quota_window_check CHECK (quota_window IS NULL OR quota_window IN ('daily','monthly','total'))
);

-- 上游渠道（api_key 加密存 api_key_enc；重试参数只在 model_routes 维护）
CREATE TABLE IF NOT EXISTS upstreams (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name                TEXT UNIQUE NOT NULL,
    kind                TEXT NOT NULL DEFAULT 'custom',  -- openai|anthropic|gemini|aliyun|baidu|kimi|tencent|zhipu|volcano|openrouter|zenmux|custom
    base_url            TEXT NOT NULL,
    api_key_enc         TEXT,                          -- AES-256-GCM（crypto.rs）
    protocols           TEXT[] NOT NULL DEFAULT '{openai_chat}',
    enabled             BOOL NOT NULL DEFAULT TRUE,
    timeout_ms          INT NOT NULL DEFAULT 300000,
    breaker_threshold   INT NOT NULL DEFAULT 5,
    probe_model         TEXT NULL,
    consecutive_failures INT NOT NULL DEFAULT 0,
    disabled_by         TEXT NULL,                     -- NULL|'manual'|'auto'；manual 不自动恢复
    cooldown_until      TIMESTAMPTZ,
    use_proxy           BOOL NOT NULL DEFAULT FALSE,
    proxy_id            UUID NULL,
    extra               JSONB NOT NULL DEFAULT '{}',   -- {protocol_priority:[...], overrides:{headers:{add,set},body:{add,set}}}
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT upstreams_disabled_by_check CHECK (disabled_by IS NULL OR disabled_by IN ('manual','auto'))
);

-- 模型路由（model_pattern 支持通配符 *）
CREATE TABLE IF NOT EXISTS model_routes (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    model_pattern     TEXT NOT NULL,
    upstream_id       UUID NOT NULL REFERENCES upstreams(id) ON DELETE CASCADE,
    override_model    TEXT NULL,                       -- 上游模型名覆盖（透传时重写）
    priority          INT NOT NULL DEFAULT 10,         -- 数字越小优先级越高
    weight            INT NOT NULL DEFAULT 1,
    enabled           BOOL NOT NULL DEFAULT TRUE,
    retries           INT NOT NULL DEFAULT 2,
    retry_status_codes INT[] NOT NULL DEFAULT '{429,500,502,503,504}',
    lock_upstream     BOOL NOT NULL DEFAULT FALSE,     -- 锁定上游：失败不自动故障转移
    sort_order        INT NOT NULL DEFAULT 0,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS model_routes_pattern_idx ON model_routes (model_pattern);

-- 用量上限独立计数器（与日志保留/清理解耦；计费 worker 与 usage_logs 同一批量事务累加）
CREATE TABLE IF NOT EXISTS quota_usage (
    key_id       UUID NOT NULL REFERENCES api_keys(id) ON DELETE CASCADE,
    unit         TEXT NOT NULL,                        -- 'tokens'|'cost_cny'|'cost_usd'
    window       TEXT NOT NULL,                        -- 'daily'|'monthly'|'total'
    period_start TIMESTAMPTZ NOT NULL,                 -- total 固定 '1970-01-01'
    value        NUMERIC(20,10) NOT NULL DEFAULT 0,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (key_id, unit, window, period_start)
);
