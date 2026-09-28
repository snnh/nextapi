-- 统计报表优化（P1）：usage_hourly 增加缓存 token 列 + 历史回填。
-- 背景：明细表 usage_logs 一直有 cache_read_tokens / cache_write_tokens（计价在用），
-- 但汇总表缺失，导致长区间（>7 天）/日粒度查询走 hourly 源时缓存 token 恒 0。
-- 新口径：总 Token = prompt + completion + cache_read + cache_write（与配额侧一致）。
--
-- 回填范围与语义（务必知晓）：
--   1) 只覆盖现存 usage_logs 分区（30 天滚动，被 drop 的分区无明细可补，属可接受损失）；
--   2) 只回填「缓存列全为 0」的桶（守卫条件保证可重复执行、不会重复累加）；
--      已有非 0 缓存值的桶会被整桶跳过——正常启动顺序（migrate 先于日志写入）下不会出现
--      这种桶，仅手工补跑/双实例并发时可能，届时该桶少算迁移前的部分；
--   3) 小时截断必须与写入侧 logging::hour_bucket（UTC 整点，epoch 对齐）完全一致，
--      故显式走 UTC（`AT TIME ZONE 'UTC'` 往返），不能用裸 date_trunc('hour', ts)——
--      后者按 DB 会话时区截断，在半小时时区（如 Asia/Kolkata +05:30）会得到与
--      usage_hourly.hour 对不上的时刻，导致回填静默失效（UPDATE 0）。

ALTER TABLE usage_hourly
  ADD COLUMN IF NOT EXISTS cache_write_tokens BIGINT NOT NULL DEFAULT 0,
  ADD COLUMN IF NOT EXISTS cache_read_tokens BIGINT NOT NULL DEFAULT 0;

-- ts 下界取 usage_hourly 最早桶：join 本就只可能命中这些桶，加下界仅为分区裁剪
-- （usage_hourly 为空时下界为 NULL → 谓词为 NULL → 不更新任何行，符合预期）。
UPDATE usage_hourly h
   SET cache_write_tokens = a.cw,
       cache_read_tokens = a.cr
  FROM (
    SELECT date_trunc('hour', ts AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' AS hour,
           model,
           COALESCE(sum(cache_write_tokens), 0)::bigint AS cw,
           COALESCE(sum(cache_read_tokens), 0)::bigint AS cr
      FROM usage_logs
     WHERE (cache_write_tokens IS NOT NULL OR cache_read_tokens IS NOT NULL)
       AND ts >= (SELECT min(hour) FROM usage_hourly)
     GROUP BY 1, 2
  ) a
 WHERE h.hour = a.hour
   AND h.model = a.model
   AND h.cache_write_tokens = 0
   AND h.cache_read_tokens = 0
   AND (a.cw <> 0 OR a.cr <> 0);
