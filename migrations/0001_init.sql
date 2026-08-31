-- NextAPI M1 数据库表初始化（仅 M1 需要的表）。
--
-- schema 按 PLAN.md §6（数据库设计）：
--   app_meta / system_settings / admin_users / admin_audit_logs /
--   proxy_configs / fx_rates
--
-- 说明：
-- - 其余表（api_keys / upstreams / model_routes / price_rules / usage_logs /
--   usage_hourly / quota_usage / media_tasks）在后续里程碑（M3/M4/M5/M6）迁移中创建。
-- - gen_random_uuid() 为 PostgreSQL 15+ 内置，无需 pgcrypto 扩展。

-- 应用元信息：记录 seeded_at / schema_version 等（seed 标记，防止种子复活）
CREATE TABLE IF NOT EXISTS app_meta (
    key        TEXT PRIMARY KEY,
    value      JSONB,
    updated_at TIMESTAMPTZ DEFAULT now()
);

-- 系统设置（运行参数键值，UI 全配置；source 记录来源 file|ui|env）
CREATE TABLE IF NOT EXISTS system_settings (
    key              TEXT PRIMARY KEY,
    value            JSONB,
    source           TEXT NOT NULL DEFAULT 'file',
    secret           BOOL NOT NULL DEFAULT FALSE,
    restart_required BOOL NOT NULL DEFAULT FALSE,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT system_settings_source_check CHECK (source IN ('file', 'ui', 'env'))
);

-- 管理员账号（单管理员，无用户/角色表）
CREATE TABLE IF NOT EXISTS admin_users (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username      TEXT UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 管理操作审计日志（独立表，不混入 usage_logs）
CREATE TABLE IF NOT EXISTS admin_audit_logs (
    id          BIGSERIAL PRIMARY KEY,
    admin_id    UUID,
    action      TEXT NOT NULL,
    object_type TEXT,
    object_id   TEXT,
    summary     JSONB,
    ip          INET,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 代理配置（http|https|socks5；密码 AES-256-GCM 加密存 password_enc）
CREATE TABLE IF NOT EXISTS proxy_configs (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name         TEXT UNIQUE NOT NULL,
    kind         TEXT NOT NULL,
    host         TEXT NOT NULL,
    port         INT NOT NULL,
    username     TEXT,
    password_enc TEXT,
    no_proxy     TEXT[] NOT NULL DEFAULT '{}',
    enabled      BOOL NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT proxy_configs_kind_check CHECK (kind IN ('http', 'https', 'socks5')),
    CONSTRAINT proxy_configs_port_check CHECK (port BETWEEN 1 AND 65535)
);

-- 汇率（manual 与 auto 分行独立存储，主键含 source）
CREATE TABLE IF NOT EXISTS fx_rates (
    currency_from TEXT NOT NULL,
    currency_to   TEXT NOT NULL,
    rate          NUMERIC(20, 10) NOT NULL,
    source        TEXT NOT NULL,
    fetched_at    TIMESTAMPTZ,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (currency_from, currency_to, source),
    CONSTRAINT fx_rates_source_check CHECK (source IN ('manual', 'auto'))
);
