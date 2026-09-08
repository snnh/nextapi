-- M14.2 日志查询性能：usage_logs(request_id, ts DESC) 显式索引（PLAN §5.2）。
--
-- 0003_logging.sql 的 UNIQUE (request_id, ts) 虽隐式建了唯一索引（ts ASC），
-- 但详情/按 request_id 过滤查询均为 `WHERE request_id = $1 ORDER BY ts DESC`，
-- 显式 DESC 索引避免反向扫描；声明式分区表上建索引会自动传播到各分区
-- （含 ATTACH 的新分区，分区索引随父索引继承）。
-- 不改旧迁移，IF NOT EXISTS 保证幂等。
CREATE INDEX IF NOT EXISTS idx_usage_logs_request_ts ON usage_logs (request_id, ts DESC);
