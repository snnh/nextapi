//! WAL 兜底（PLAN §5.5：JSONL + CRC，单文件上限滚动，重放成功后归档 24h）。
//! 契约 contracts/m4-logging.md §4，M4-A 实现。
//!
//! 活跃文件判定（review P5 修正）：归档经 WalWriter::archive_if_inactive 在 append 同一把
//! 写锁内原子判定（cur_path 非活跃 + 长度未变才改名），不再用 mtime 最新启发式——
//! 启动重放时本进程尚无活跃文件，旧文件全部可归档，避免崩溃残留文件被每轮重放却
//! 永不归档；同时杜绝「读后并发追加的字节被归档滞留」的丢日志竞态。
//! WalWriter::new 不做任何 IO，目录在首次 append 时创建；磁盘写失败返回 Err 由调用方计数告警。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::{ApiError, ApiResult};
use crate::logging::{LogEvent, BATCH_MAX};

/// WAL 写入器（Clone 共享内部状态）。目录不可变，存放在结构体上以支持 `dir()` 返回引用。
#[derive(Clone)]
pub struct WalWriter {
    dir: PathBuf,
    inner: Arc<Mutex<WalInner>>,
}

/// 内部可变状态：当前活跃文件路径与其已写字节数，以及文件序号（保证同进程文件名唯一）。
struct WalInner {
    max_bytes: u64,
    cur_path: Option<PathBuf>,
    cur_size: u64,
    seq: u64,
}

impl WalWriter {
    /// 以 MB 为单位的上限创建写入器（不做 IO）。
    pub fn new(dir: PathBuf, max_mb: u64) -> Self {
        Self::with_max_bytes(dir, max_mb.saturating_mul(1024 * 1024))
    }

    /// 以字节为单位的上限创建写入器（供测试用小上限强制滚动）。
    fn with_max_bytes(dir: PathBuf, max_bytes: u64) -> Self {
        Self {
            dir,
            inner: Arc::new(Mutex::new(WalInner {
                max_bytes,
                cur_path: None,
                cur_size: 0,
                seq: 0,
            })),
        }
    }

    /// 追加一批（必要时滚动新文件；建目录/写失败 → Err，调用方计数告警）。
    pub async fn append(&self, events: &[LogEvent]) -> ApiResult<()> {
        if events.is_empty() {
            return Ok(());
        }
        // 先把整批编码为一段文本，便于一次写入并精确统计字节数。
        let mut buf = String::new();
        for ev in events {
            buf.push_str(&encode_line(ev));
            buf.push('\n');
        }
        let data_len = buf.len() as u64;

        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            return Err(ApiError::internal(e));
        }

        // 是否需要滚动新文件：当前没有活跃文件，或当前文件写满。
        let need_new = match inner.cur_path {
            None => true,
            Some(_) => should_roll(inner.cur_size, data_len, inner.max_bytes),
        };

        if need_new {
            inner.seq += 1;
            let path = wal_path(&self.dir, wal_timestamp(), inner.seq);
            write_all(&path, buf.as_bytes())?;
            inner.cur_path = Some(path);
            inner.cur_size = data_len;
        } else if let Some(path) = inner.cur_path.as_ref() {
            // 追加到当前活跃文件。
            write_all(path, buf.as_bytes())?;
            inner.cur_size += data_len;
        } else {
            // 防御分支：理论上不会走到（need_new 为 true 时已创建），但避免 unwrap。
            inner.seq += 1;
            let path = wal_path(&self.dir, wal_timestamp(), inner.seq);
            write_all(&path, buf.as_bytes())?;
            inner.cur_path = Some(path);
            inner.cur_size = data_len;
        }
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 若 path 非当前活跃文件且长度未变（无并发追加）则原子改名归档，返回是否归档。
    ///
    /// 判定与改名都在 append 同一把写锁内完成，杜绝「重放读到一半被并发追加、
    /// 归档后尾部字节滞留 .archived 永不落库」的竞态；len 校验兜底「读后追加再滚动」
    /// 场景（文件已非活跃但被追加过未重放的字节）。
    pub fn archive_if_inactive(&self, path: &Path, expected_len: u64) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.cur_path.as_deref() == Some(path) {
            return false;
        }
        let cur_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX);
        if cur_len != expected_len {
            return false;
        }
        match std::fs::rename(path, path.with_extension("archived")) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(path = %path.display(), "WAL 归档失败: {e}");
                false
            }
        }
    }
}

/// 生成 WAL 文件名路径：`<dir>/wal-<nanos>-<seq>.jsonl`，保证同进程内唯一。
fn wal_path(dir: &Path, nanos: u128, seq: u64) -> PathBuf {
    dir.join(format!("wal-{nanos}-{seq}.jsonl"))
}

/// 当前时间戳（纳秒），用于文件名排序/去重。
fn wal_timestamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// 纯函数：判断给定当前大小与追加大小是否触发滚动（max_bytes<=0 视为不限）。供测试。
fn should_roll(cur_size: u64, incoming: u64, max_bytes: u64) -> bool {
    max_bytes > 0 && cur_size.saturating_add(incoming) > max_bytes
}

/// 将数据写入（追加到）指定文件；失败返回 Err。
fn write_all(path: &Path, data: &[u8]) -> ApiResult<()> {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(ApiError::internal)?;
    f.write_all(data).map_err(ApiError::internal)?;
    f.flush().map_err(ApiError::internal)?;
    // fsync：flush 只到 page cache，主机掉电会丢尾字节——WAL 的定位就是
    // 崩溃兜底，必须落盘（发布审阅数据批 #4c；该路径低频，开销可接受）。
    f.sync_all().map_err(ApiError::internal)?;
    Ok(())
}

/// 行编码：{"crc":u32,"event":{...}}（crc32fast 对 event JSON bytes）。
///
/// 先经 serde_json::Value 序列化，保证解码端 `serde_json::to_string(&Value)` 与之字节一致
/// （serde_json 的 Object 默认按键排序，直接序列化结构体为字段声明序，两者不一致会导致 crc 失配）。
pub fn encode_line(ev: &LogEvent) -> String {
    let event_json =
        serde_json::to_string(&serde_json::to_value(ev).unwrap_or_else(|_| serde_json::json!({})))
            .unwrap_or_else(|_| "{}".into());
    let crc = crc32fast::hash(event_json.as_bytes());
    format!("{{\"crc\":{crc},\"event\":{event_json}}}")
}

/// 行解码：crc 校验失败 → None。
pub fn decode_line(line: &str) -> Option<LogEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let crc = v.get("crc")?.as_u64()? as u32;
    let event = v.get("event")?;
    let event_str = serde_json::to_string(event).ok()?;
    if crc32fast::hash(event_str.as_bytes()) != crc {
        return None;
    }
    serde_json::from_value(event.clone()).ok()
}

/// 读单个 WAL 文件并解码（坏行跳过 + warn）。供重放与测试使用。
fn decode_file(path: &Path) -> ApiResult<Vec<LogEvent>> {
    let text = std::fs::read_to_string(path).map_err(ApiError::internal)?;
    let mut events = Vec::new();
    for line in text.lines() {
        match decode_line(line) {
            Some(ev) => events.push(ev),
            None => tracing::warn!(path = %path.display(), "WAL 坏行跳过"),
        }
    }
    Ok(events)
}

/// 列出目录下全部 wal-*.jsonl 文件，按 mtime 升序（末尾为最新）。
fn list_wal_files(dir: &Path) -> ApiResult<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(dir).map_err(ApiError::internal)?;
    let mut files = Vec::new();
    for e in entries {
        let e = e.map_err(ApiError::internal)?;
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("wal-") && name.ends_with(".jsonl") {
            let p = e.path();
            if p.is_file() {
                files.push(p);
            }
        }
    }
    files.sort_by_key(|p| {
        p.metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    Ok(files)
}

/// 删除 mtime 超过 24h 的 *.archived 文件。
fn cleanup_archived(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !name.ends_with(".archived") {
                continue;
            }
            let stale = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| m.elapsed().ok())
                .map(|el| el.as_secs() > 24 * 3600)
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

/// 重放所有 wal-*.jsonl：坏行跳过 + warn；非活跃文件 insert_batch 成功后归档（改名 .archived，
/// 由 WalWriter::archive_if_inactive 在写锁内原子判定，活跃/并发追加的文件留待下轮）；
/// 删除 mtime 超过 24h 的 *.archived。返回 (回放事件数, 归档文件数)。
///
/// `wal` 为本进程的写入器：启动重放时其尚无活跃文件（内部 None），旧文件全部可归档，
/// 修正此前 mtime 最新启发式导致崩溃残留文件永不归档、每轮空转重放的问题（review P5）。
pub async fn replay_and_archive(
    dir: &Path,
    pool: &sqlx::PgPool,
    billing_tz: &str,
    fx_stale_max_minutes: u64,
    wal: &WalWriter,
) -> ApiResult<(u64, u64)> {
    let files = list_wal_files(dir)?;
    if files.is_empty() {
        return Ok((0, 0));
    }
    let mut replayed = 0u64;
    let mut archived = 0u64;

    for f in &files {
        // 重放前记录长度：归档时复核，防止「读后并发追加」的字节被归档滞留。
        let len_before = std::fs::metadata(f).map(|m| m.len()).unwrap_or(u64::MAX);
        let (n, ok) = replay_wal_file(pool, f, billing_tz, fx_stale_max_minutes).await?;
        replayed += n;
        if ok {
            if wal.archive_if_inactive(f, len_before) {
                archived += 1;
            }
        } else {
            tracing::warn!(path = %f.display(), "WAL 重放失败，保留文件留待下次");
        }
    }

    cleanup_archived(dir);
    Ok((replayed, archived))
}

/// 重放单个 WAL 文件：逐批 insert_batch。返回 (成功重放事件数, 是否全部成功)。
/// 写库失败 → ok=false（文件保留留待下次）；坏行跳过 + warn 视为成功。
async fn replay_wal_file(
    pool: &sqlx::PgPool,
    f: &std::path::Path,
    billing_tz: &str,
    fx_stale_max_minutes: u64,
) -> ApiResult<(u64, bool)> {
    let events = match decode_file(f) {
        Ok(ev) => ev,
        Err(e) => {
            tracing::warn!(path = %f.display(), "WAL 读取失败，跳过: {e}");
            return Ok((0, false));
        }
    };
    if events.is_empty() {
        return Ok((0, true));
    }
    let mut replayed = 0u64;
    for chunk in events.chunks(BATCH_MAX) {
        match crate::logging::insert_batch(pool, chunk, billing_tz, fx_stale_max_minutes).await {
            Ok(_) => replayed += chunk.len() as u64,
            Err(e) => {
                tracing::warn!(path = %f.display(), "WAL 重放写库失败，保留文件: {e}");
                return Ok((replayed, false));
            }
        }
    }
    Ok((replayed, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    fn mk_event(i: i64) -> LogEvent {
        LogEvent {
            request_id: format!("req-{i}"),
            ts: Utc.timestamp_opt(i, 0).unwrap(),
            key_id: Some(Uuid::new_v4()),
            model: "gpt-4".into(),
            requested_model: None,
            upstream_model: None,
            upstream_id: None,
            protocol_in: "openai_chat".into(),
            protocol_out: "openai_chat".into(),
            convert_mode: "passthrough".into(),
            stream: false,
            status: 200,
            error: None,
            prompt_tokens: Some(10),
            completion_tokens: Some(5),
            cache_write_tokens: None,
            cache_read_tokens: None,
            latency_ms: Some(42),
            retry_count: 0,
            ttfb_ms: None,
            degraded: false,
            usage_raw: None,
            debug_payload: None,
            // M5 计价字段（测试里恒 None）。
            cost_cny: None,
            cost_usd: None,
            pricing_source: None,
            price_used: None,
            fx_snapshot: None,
            // M6 媒体字段（测试里恒 None）。
            images: None,
            image_size: None,
            video_seconds: None,
            video_resolution: None,
            video_task_type: None,
            // M14.3 白名单请求头摘要（测试里恒 None）。
            request_headers: None,
        }
    }

    #[test]
    fn line_roundtrip() {
        let ev = mk_event(1_700_000_000);
        let line = encode_line(&ev);
        let out = decode_line(&line).expect("应解码成功");
        assert_eq!(
            serde_json::to_string(&out).unwrap(),
            serde_json::to_string(&ev).unwrap()
        );
    }

    #[test]
    fn crc_corruption_returns_none() {
        let ev = mk_event(1_700_000_001);
        let line = encode_line(&ev);
        // 篡改 event 字段导致 crc 不匹配
        let corrupted = line.replacen(r#""status":200"#, r#""status":500"#, 1);
        assert!(decode_line(&corrupted).is_none());
    }

    #[test]
    fn decode_file_ignores_truncated_tail() {
        let dir = std::env::temp_dir().join(format!("wal-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wal-test.jsonl");
        let good = encode_line(&mk_event(1_700_000_002));
        std::fs::write(&path, format!("{good}\n{{\"event\":")).unwrap();
        let events = decode_file(&path).unwrap();
        assert_eq!(events.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn should_roll_boundary() {
        assert!(!should_roll(0, 100, 0)); // max_bytes=0 视为不限
        assert!(!should_roll(90, 10, 100));
        assert!(should_roll(99, 2, 100));
        assert!(should_roll(0, 101, 100));
    }

    #[tokio::test]
    async fn append_empty_batch_does_not_create_directory() {
        let dir = std::env::temp_dir().join(format!("wal-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let w = WalWriter::with_max_bytes(dir.clone(), 512);
        w.append(&[]).await.unwrap();
        assert!(!dir.exists());
    }

    #[tokio::test]
    async fn append_rolls_when_bytes_exceed() {
        let dir = std::env::temp_dir().join(format!("wal-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let w = WalWriter::with_max_bytes(dir.clone(), 512); // 512 字节上限，强制滚动

        // 用大量事件驱动滚动
        let mut n = 0;
        for _ in 0..20 {
            let batch: Vec<LogEvent> = (0..10).map(|i| mk_event(n + i)).collect();
            n += 10;
            w.append(&batch).await.expect("append 应成功");
        }

        // 至少产生多个文件
        let files = list_wal_files(&dir).expect("列目录失败");
        assert!(files.len() >= 2, "应发生滚动，实际文件数 {}", files.len());

        // 编解码往返
        for f in &files {
            let events = decode_file(f).expect("读文件失败");
            assert!(!events.is_empty());
            for e in &events {
                assert!(e.request_id.starts_with("req-"));
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn archive_if_inactive_guards_active_and_grown_files() {
        let dir = std::env::temp_dir().join(format!("wal-arch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let w = WalWriter::with_max_bytes(dir.clone(), 512);
        w.append(&[mk_event(1_700_000_010)]).await.unwrap();
        let files = list_wal_files(&dir).unwrap();
        assert_eq!(files.len(), 1);
        let active = files[0].clone();
        let len = std::fs::metadata(&active).unwrap().len();

        // 活跃文件不归档
        assert!(!w.archive_if_inactive(&active, len));
        assert!(active.exists());

        // 非活跃但长度不符（并发追加过）不归档
        let other = dir.join("wal-other.jsonl");
        std::fs::write(&other, b"x").unwrap();
        assert!(!w.archive_if_inactive(&other, 999));
        assert!(other.exists());

        // 非活跃且长度一致 → 归档成功
        assert!(w.archive_if_inactive(&other, 1));
        assert!(!other.exists());
        assert!(other.with_extension("archived").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
