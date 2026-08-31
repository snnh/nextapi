# =============================================================================
# NextAPI Dockerfile —— 多阶段构建
# -----------------------------------------------------------------------------
# builder：官方 rust bookworm 镜像，cargo build --release；
# runtime：debian bookworm-slim（镜像小、非 root 运行）。
#
# 说明：
# - rustls：runtime 需要系统 CA 证书（ca-certificates）以访问 HTTPS 外联；
# - sqlx：运行时校验（无 query! 宏），构建期无需数据库；
# - M8 前端将用 rust-embed 内嵌进二进制，无需复制静态资源（预留注释见下）。
# =============================================================================

# ---------- builder ----------
FROM rust:1-bookworm AS builder
WORKDIR /app

# 先拷贝依赖清单做哑构建，缓存依赖层
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs \
    && cargo build --release \
    && rm -rf src

# 拷贝真实源码与迁移文件（sqlx::migrate! 编译期引用 migrations/）
COPY src ./src
COPY migrations ./migrations
RUN cargo build --release

# ---------- runtime ----------
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -r nextapi \
    && useradd -r -g nextapi nextapi \
    && mkdir -p /data /etc/nextapi \
    && chown -R nextapi:nextapi /data /etc/nextapi

COPY --from=builder /app/target/release/nextapi /usr/local/bin/nextapi

# M8 前端经 rust-embed 编译进二进制；若改为外部 dist 挂载可在此添加 COPY。

ENV NEXTAPI_CONFIG=/etc/nextapi/config.yaml
EXPOSE 8080
VOLUME /data

USER nextapi
CMD ["nextapi"]
