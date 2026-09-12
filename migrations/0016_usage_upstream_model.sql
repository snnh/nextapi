-- 计价跟随上游实际模型名（修复：路由 override_model 改写后仍按入口名查价格 → 未计价）。
--   usage_logs.upstream_model : 上游实际使用的模型名（override_model 生效且与入口 model 不同时记录，NULL=与 model 相同）
--   media_tasks.upstream_model: 异步媒体任务创建时的上游实际模型名（poller 计价按此名查价格）
-- 说明：仅新增可空列，历史数据不回填；计价在写入时按 COALESCE(upstream_model, model) 匹配价格规则。
ALTER TABLE usage_logs ADD COLUMN IF NOT EXISTS upstream_model text;
ALTER TABLE media_tasks ADD COLUMN IF NOT EXISTS upstream_model text;
