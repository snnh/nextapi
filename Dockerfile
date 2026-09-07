# =============================================================================
# NextAPI Dockerfile —— 多阶段构建（M8：前端内嵌）
# -----------------------------------------------------------------------------
# fe：node:22-alpine 构建 Vue3 管理后台（web/dist）；
# builder：官方 rust bookworm 镜像，cargo build --release（rust-embed 内嵌前端）；
# runtime：debian bookworm-slim（镜像小、非 root 运行）。
#
# 说明：
# - rustls：runtime 需要系统 CA 证书（ca-certificates）以访问 HTTPS 外联；
# - sqlx：运行时校验（无 query! 宏），构建期无需数据库；
# - rust-embed 编译期读取 web/dist，必须先构建前端（本文件已保证顺序）。
# =============================================================================

# ---------- 前端构建 ----------
FROM node:22-alpine AS fe
WORKDIR /fe
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web ./
RUN npm run build

# ---------- builder ----------
FROM rust:1-bookworm AS builder
WORKDIR /app

# 先拷贝依赖清单做哑构建，缓存依赖层
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs \
    && cargo build --release \
    && rm -rf src

# 拷贝真实源码、迁移文件与前端产物（sqlx::migrate! 需 migrations/，rust-embed 需 web/dist）
COPY src ./src
COPY migrations ./migrations
COPY --from=fe /fe/dist ./web/dist
# BuildKit 的 COPY 保留源文件 mtime：真实源码若旧于哑构建产物时间戳，cargo 指纹比对
# 会误判 fresh 而跳过主 crate 重编，把 374KB 的哑构建空壳打进镜像。此处刷新 mtime 强制
# 全量重编，并用体积下限做硬校验（真实 release 二进制 >> 1MB，哑构建约 0.4MB）。
RUN find src migrations -type f -exec touch {} + \
    && cargo build --release \
    && test "$(stat -c%s target/release/nextapi)" -gt 1048576

# ---------- runtime ----------
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -r nextapi \
    && useradd -r -g nextapi nextapi \
    && mkdir -p /data /etc/nextapi \
    && chown -R nextapi:nextapi /data /etc/nextapi

COPY --from=builder /app/target/release/nextapi /usr/local/bin/nextapi

# 前端已经 rust-embed 编译进二进制；如改为外部 dist 挂载可在此添加 COPY。

ENV NEXTAPI_CONFIG=/etc/nextapi/config.yaml
EXPOSE 8080
VOLUME /data

USER nextapi
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1
CMD ["nextapi"]
