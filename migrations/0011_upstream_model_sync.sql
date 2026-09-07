-- 渠道模型管理（M12.6）：
--   upstreams 增加模型同步策略（manual=仅手动 / auto=跟随上游自动更新）、排除名单、
--   最近拉取的模型列表缓存；model_routes 增加 managed_by 标记（'auto'=同步托管，
--   自动同步只增删自己托管的路由，绝不碰手动路由）。
ALTER TABLE upstreams ADD COLUMN model_sync TEXT NOT NULL DEFAULT 'manual';
ALTER TABLE upstreams ADD COLUMN model_exclude TEXT[] NOT NULL DEFAULT '{}';
ALTER TABLE upstreams ADD COLUMN models_cache JSONB NOT NULL DEFAULT '[]';
ALTER TABLE upstreams ADD COLUMN models_fetched_at TIMESTAMPTZ;
ALTER TABLE upstreams ADD CONSTRAINT upstreams_model_sync_check
    CHECK (model_sync IN ('manual','auto'));

ALTER TABLE model_routes ADD COLUMN managed_by TEXT NULL;
ALTER TABLE model_routes ADD CONSTRAINT model_routes_managed_by_check
    CHECK (managed_by IS NULL OR managed_by IN ('auto'));

-- 同一上游下同一 model_pattern 的托管路由唯一（手动路由允许跨上游同模式，故仅部分索引）
CREATE UNIQUE INDEX model_routes_auto_uniq
    ON model_routes (model_pattern, upstream_id) WHERE managed_by = 'auto';
