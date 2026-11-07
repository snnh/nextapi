-- 上游余额查询独立凭证（AES-256-GCM 密文；NULL = 回退推理 Key）。
-- 仅管理员可见 has_balance_key，不向前端回传密文或明文。
ALTER TABLE upstreams ADD COLUMN IF NOT EXISTS balance_key_enc TEXT;