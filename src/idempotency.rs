//! 请求幂等键（M10.2 / PLAN.md §4.2）。
//!
//! 语义：
//! - 非流式请求与图片生成（含异步任务）支持 `Idempotency-Key` 请求头；
//! - 同 Key + 同参数在有效期（24h）内：重放缓存响应（带 `Idempotency-Replayed: true`）
//!   / 异步任务返回同一 task_id；参数不一致 → 409；同键处理中 → 409；
//! - 失败的请求（上游错误/转换失败）标记 failed，同键可重试；
//! - 流式请求携带幂等键 → 400（不缓存完整响应，明确拒绝而非静默忽略）；
//! - 重放不重复计费（不写 usage_logs、不计 metrics）；
//! - 限流/无路由等未触达上游的失败**不登记**（begin 在路由决策之后调用）；
//! - 过期记录由 main.rs 周期任务清理。

use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

/// 请求头名（小写；HeaderMap 大小写不敏感）
pub const IDEM_HEADER: &str = "idempotency-key";
/// 重放标记响应头
pub const REPLAYED_HEADER: &str = "idempotency-replayed";
/// 幂等记录有效期（小时）
pub const TTL_HOURS: i32 = 24;

/// begin 结果。
pub enum Begin {
    /// 新请求（或接管 failed 记录）：继续正常处理，结束时 complete/fail。
    Proceed(Uuid),
    /// 重放：直接返回缓存响应（原 HTTP 状态 + 原响应体）。
    Replay { status: i32, response: Value },
    /// 冲突：参数不一致或同键请求处理中。
    Conflict(&'static str),
}

/// 校验幂等键格式；Ok(规整后) / Err(中文原因)。
pub fn validate_key(k: &str) -> Result<&str, &'static str> {
    let k = k.trim();
    if k.len() < 8 || k.len() > 128 {
        return Err("Idempotency-Key 长度须为 8-128 字符");
    }
    if !k
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-' | '/'))
    {
        return Err("Idempotency-Key 只能包含字母、数字与 . _ : - /");
    }
    Ok(k)
}

/// 请求指纹：body 递归规范化（对象键排序、紧凑序列化）后 sha256 hex。纯函数，供测试。
pub fn fingerprint(body: &Value) -> String {
    let mut s = String::with_capacity(256);
    canon(body, &mut s);
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// 规范化序列化：对象键排序，数组保序（参数顺序语义不同视为不同请求）。
fn canon(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => out.push_str(&serde_json::to_string(s).unwrap_or_default()),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canon(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push(':');
                canon(&m[*k], out);
            }
            out.push('}');
        }
    }
}

/// 幂等登记：查有效记录 → 命中按状态分派；无记录则插入 processing
/// （并发下同键仅一个插入成功，失败者重读一次）。
pub async fn begin(
    pool: &PgPool,
    key_id: Uuid,
    idem_key: &str,
    fp: &str,
) -> Result<Begin, sqlx::Error> {
    for _ in 0..2 {
        let row = sqlx::query_as::<_, (Uuid, String, String, Option<Value>, Option<i32>)>(
            "SELECT id, fingerprint, status, response, response_status FROM idempotency_keys \
             WHERE key_id = $1 AND idem_key = $2 AND expires_at > now()",
        )
        .bind(key_id)
        .bind(idem_key)
        .fetch_optional(pool)
        .await?;

        if let Some((rid, rfp, status, response, rstatus)) = row {
            if rfp != fp {
                return Ok(Begin::Conflict("同一幂等键的请求参数不一致"));
            }
            return Ok(match status.as_str() {
                "completed" => Begin::Replay {
                    status: rstatus.unwrap_or(200),
                    response: response.unwrap_or(Value::Null),
                },
                "processing" => Begin::Conflict("相同幂等键的请求正在处理中"),
                // failed：接管重跑（状态回到 processing，刷新有效期）
                _ => {
                    sqlx::query(
                        "UPDATE idempotency_keys SET status = 'processing', response = NULL, \
                         response_status = NULL, task_id = NULL, \
                         expires_at = now() + make_interval(hours => $2) WHERE id = $1",
                    )
                    .bind(rid)
                    .bind(TTL_HOURS)
                    .execute(pool)
                    .await?;
                    Begin::Proceed(rid)
                }
            });
        }

        let ins = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO idempotency_keys (key_id, idem_key, fingerprint, status, expires_at) \
             VALUES ($1, $2, $3, 'processing', now() + make_interval(hours => $4)) \
             ON CONFLICT (key_id, idem_key) DO NOTHING RETURNING id",
        )
        .bind(key_id)
        .bind(idem_key)
        .bind(fp)
        .bind(TTL_HOURS)
        .fetch_optional(pool)
        .await?;

        if let Some(rid) = ins {
            return Ok(Begin::Proceed(rid));
        }
        // 并发冲突：对方记录此刻必然可见，回到循环重读一次
    }
    // 极端：连续两次插入冲突且读不到（对方刚插入未提交）→ 按处理中拒绝
    Ok(Begin::Conflict("相同幂等键的请求正在处理中"))
}

/// 登记成功结果（非流式响应体 / 异步任务 {"task_id":...}）。
pub async fn complete(
    pool: &PgPool,
    rid: Uuid,
    status: i32,
    response: &Value,
    task_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE idempotency_keys SET status = 'completed', response = $2, response_status = $3, \
         task_id = $4 WHERE id = $1",
    )
    .bind(rid)
    .bind(response)
    .bind(status)
    .bind(task_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 登记失败（上游/转换失败）：标记 failed 允许同键重试。
pub async fn fail(pool: &PgPool, rid: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE idempotency_keys SET status = 'failed' WHERE id = $1")
        .bind(rid)
        .execute(pool)
        .await?;
    Ok(())
}

/// 清理过期记录；返回删除行数。
pub async fn cleanup_expired(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let r = sqlx::query("DELETE FROM idempotency_keys WHERE expires_at <= now()")
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fingerprint_key_order_insensitive() {
        let a = json!({"model":"m","messages":[{"role":"user","content":"hi"}],"stream":false});
        let b = json!({"stream":false,"messages":[{"content":"hi","role":"user"}],"model":"m"});
        assert_eq!(fingerprint(&a), fingerprint(&b));
        // 参数不同 → 指纹不同
        let c = json!({"model":"m","messages":[{"role":"user","content":"hi"}],"stream":true});
        assert_ne!(fingerprint(&a), fingerprint(&c));
        // 数组顺序敏感
        let d = json!({"a":[1,2]});
        let e = json!({"a":[2,1]});
        assert_ne!(fingerprint(&d), fingerprint(&e));
    }

    #[test]
    fn validate_key_rules() {
        assert!(validate_key("abcdefgh").is_ok());
        assert!(validate_key("  order_2024-01:x/1  ").is_ok());
        assert!(validate_key("short").is_err());
        assert!(validate_key(&"a".repeat(129)).is_err());
        assert!(validate_key("has space inside").is_err());
        assert!(validate_key("中文键值不支持").is_err());
    }
}
