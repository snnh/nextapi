-- 0010：TOTP 防重放（发布审阅 M3）——记录最近一次校验通过的时间步，
-- 同一窗口内的同一验证码不可二次使用（RFC 6238 §5.2）。
ALTER TABLE admin_users
    ADD COLUMN IF NOT EXISTS last_totp_step BIGINT;
