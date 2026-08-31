//! Prometheus 指标（/metrics 正式暴露，PLAN.md §5.5）。

use axum::{extract::State, response::IntoResponse};
use prometheus_client::{
    encoding::{text::encode, EncodeLabelSet},
    metrics::{counter::Counter, family::Family, gauge::Gauge, histogram::Histogram},
    registry::Registry,
};
use std::sync::{Arc, Mutex};

use crate::state::AppState;

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct RequestLabels {
    /// 网关 Key 前 8 位前缀（不暴露完整 Key）
    pub key_prefix: String,
    pub model: String,
    pub upstream: String,
    pub protocol: String,
}

pub struct Metrics {
    registry: Mutex<Registry>,
    /// 网关请求数
    pub gateway_requests: Family<RequestLabels, Counter>,
    /// 网关错误数
    pub gateway_errors: Family<RequestLabels, Counter>,
    /// 请求延迟直方图（秒）
    pub gateway_latency: Family<RequestLabels, Histogram>,
    /// token 计数
    pub gateway_tokens: Family<RequestLabels, Counter>,
    /// 异步日志队列 / WAL 溢出计数
    pub log_overflows: Counter,
    /// 当前异步日志队列深度
    pub log_queue_depth: Gauge,
}

impl Metrics {
    pub fn new() -> Self {
        let mut registry = Registry::default();
        let gateway_requests = Family::<RequestLabels, Counter>::default();
        let gateway_errors = Family::<RequestLabels, Counter>::default();
        let gateway_latency = Family::<RequestLabels, Histogram>::new_with_constructor(|| {
            Histogram::new(prometheus_client::metrics::histogram::exponential_buckets(0.005, 2.0, 12))
        });
        let gateway_tokens = Family::<RequestLabels, Counter>::default();
        let log_overflows = Counter::default();
        let log_queue_depth = Gauge::default();

        registry.register("nextapi_gateway_requests", "网关请求数", gateway_requests.clone());
        registry.register("nextapi_gateway_errors", "网关错误数", gateway_errors.clone());
        registry.register("nextapi_gateway_latency_seconds", "请求延迟", gateway_latency.clone());
        registry.register("nextapi_gateway_tokens", "token 计数", gateway_tokens.clone());
        registry.register("nextapi_log_overflows", "日志队列/WAL 溢出计数", log_overflows.clone());
        registry.register("nextapi_log_queue_depth", "异步日志队列深度", log_queue_depth.clone());

        Metrics {
            registry: Mutex::new(registry),
            gateway_requests,
            gateway_errors,
            gateway_latency,
            gateway_tokens,
            log_overflows,
            log_queue_depth,
        }
    }

    pub fn render(&self) -> String {
        let registry = self.registry.lock().unwrap();
        let mut buf = String::new();
        let _ = encode(&mut buf, &registry);
        buf
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// GET /metrics
pub async fn handle(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    (
        [("content-type", "application/openmetrics-text; version=1.0.0; charset=utf-8")],
        state.metrics.render(),
    )
}
