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
    pub fn conflict<S: Into<String>>(s: S) -> Self {
        Self::Conflict(s.into())
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::BadGateway(_) => StatusCode::BAD_GATEWAY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(serde_json::json!({
            "error": { "message": self.to_string(), "type": "nextapi_error" }
        }));
        (status, body).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::NotFound,
            sqlx::Error::Database(ref dbe) if dbe.is_unique_violation() => {
                Self::Conflict("唯一键冲突".into())
            }
            other => Self::internal(other),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
