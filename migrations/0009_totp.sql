-- 0007：管理员 TOTP 二次验证（M10 收尾扩展）
-- totp_secret_enc：AES-256-GCM 加密的 base32 机密（NEXTAPI_SECRET_KEY），
--   存在但未启用 = 设置流程中待确认（pending）；
-- totp_enabled：true 时登录必须携带有效 TOTP 码。
ALTER TABLE admin_users
    ADD COLUMN IF NOT EXISTS totp_secret_enc TEXT,
    ADD COLUMN IF NOT EXISTS totp_enabled BOOLEAN NOT NULL DEFAULT FALSE;
