-- M14.3 日志调试头摘要：usage_logs.request_headers JSONB NULL。
--
-- 仅记录白名单请求头（user-agent / x-forwarded-for / x-real-ip / x-forwarded-proto /
-- content-type / accept / accept-encoding / host / origin / referer / x-request-id）的
-- 小写键值对；绝不记录 Authorization / Cookie / x-api-key 等敏感头（白名单在
-- src/logging/headers.rs 服务端过滤，入库前已完成剔除与长度截断）。
-- 旧行该列恒 NULL。声明式分区表上 ADD COLUMN 自动传播到各分区；幂等。
ALTER TABLE usage_logs ADD COLUMN IF NOT EXISTS request_headers JSONB;
