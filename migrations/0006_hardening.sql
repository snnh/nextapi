-- M8.5 硬化（深度 review P2 批次）：
-- 1. usage_logs DEFAULT 分区：承接 ts 落在预建窗口之外的行
--    （上游未来时间戳/清理后滞留等），避免整批 INSERT 失败（review P2-4）；
--    DEFAULT 分区永不被手动整分区清理（其行属越界数据，由管理员另行处理）。
-- 2. admin_audit_logs 过滤/排序索引（review P2-14：audit 查询 action/时间过滤原为全表扫）。

-- usage_logs DEFAULT 分区（幂等）
CREATE TABLE IF NOT EXISTS usage_logs_default PARTITION OF usage_logs DEFAULT;

-- 审计索引（幂等）
CREATE INDEX IF NOT EXISTS idx_audit_created_at ON admin_audit_logs (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_action_created ON admin_audit_logs (action, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_object_type_created ON admin_audit_logs (object_type, created_at DESC);
