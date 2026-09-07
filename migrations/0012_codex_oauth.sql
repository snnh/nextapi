-- Codex（ChatGPT OAuth）渠道：upstreams 增加 oauth_enc 列，
-- AES-256-GCM 加密的 CodexOAuth JSON（access/refresh token、expires_at、account_id）。
-- 仅 kind='codex' 的上游使用；刷新成功即回写（refresh_token 轮换语义）。
ALTER TABLE upstreams ADD COLUMN oauth_enc TEXT NULL;
