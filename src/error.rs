//! 统一 API 错误类型。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("未授权")]
    Unauthorized,
    #[error("禁止访问")]
    Forbidden,
    #[error("资源不存在")]
    NotFound,
    #[error("参数错误: {0}")]
    BadRequest(String),
    #[error("冲突: {0}")]
    Conflict(String),
    #[error("请求过于频繁，请稍后再试")]
    RateLimited,
    /// 登录需 TOTP 二次验证码（未携带）；前端据 error.code=totp_required 弹出验证码输入。
    #[error("需要二次验证码")]
    TotpRequired,
    /// TOTP 验证码错误。
    #[error("二次验证码错误")]
    TotpInvalid,
    /// 网关/上游错误（如 GitHub 更新检查联不通），映射 HTTP 502。
    #[error("网关错误: {0}")]
    BadGateway(String),
    #[error("内部错误: {0}")]
    Internal(String),
}

impl ApiError {
    pub fn internal<E: std::fmt::Display>(e: E) -> Self {
        Self::Internal(e.to_string())
    }
    pub fn bad_request<S: Into<String>>(s: S) -> Self {
        Self::BadRequest(s.into())
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::TotpRequired | Self::TotpInvalid => StatusCode::UNAUTHORIZED,
            Self::BadGateway(_) => StatusCode::BAD_GATEWAY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let request_id = uuid::Uuid::new_v4().to_string();
        // 机器可读码：前端据 code 区分交互流程（如 TOTP 二步输入）。
        let code: Option<&str> = match &self {
            Self::TotpRequired => Some("totp_required"),
            Self::TotpInvalid => Some("totp_invalid"),
            _ => None,
        };
        let mut response = (
            status,
            Json(serde_json::json!({
                "error": { "message": self.to_string(), "type": "nextapi_error", "code": code }
            })),
        )
            .into_response();
        if let Ok(value) = axum::http::HeaderValue::from_str(&request_id) {
            response.headers_mut().insert("x-request-id", value);
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert("retry-after", axum::http::HeaderValue::from_static("1"));
        }
        response
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::NotFound,
            sqlx::Error::Database(ref dbe) if dbe.is_unique_violation() => {
                Self::Conflict("唯一键冲突".into())
            }
            // 外键违反（如引用不存在的 upstream）→ 400 参数错误而非 500（review P3）
            sqlx::Error::Database(ref dbe) if dbe.is_foreign_key_violation() => {
                Self::bad_request("引用的资源不存在")
            }
            other => Self::internal(other),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
