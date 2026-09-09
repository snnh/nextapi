-- M14 §5.2 日志游标分页：usage_logs (ts DESC, id DESC) 复合索引。
--
-- 深页 keyset 分页查询形态为 `WHERE (ts, id) < ($1, $2) ORDER BY ts DESC, id DESC`；
-- 既有 idx_usage_logs_ts (ts DESC) 无法同时满足行值比较与 id 次序，复合索引避免额外排序。
-- 声明式分区表上建索引会自动传播到既有分区与后续 ATTACH 的分区（含分区索引继承）。
-- 不改旧迁移，IF NOT EXISTS 保证幂等。
CREATE INDEX IF NOT EXISTS idx_usage_logs_ts_id ON usage_logs (ts DESC, id DESC);
