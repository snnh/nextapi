# 集成验收说明

本目录保留 PostgreSQL 与 Mock 上游集成测试入口。执行前请启动仓库提供的 PostgreSQL：

```bash
docker compose up -d postgres
export DATABASE_URL=postgres://nextapi:nextapi@127.0.0.1:5432/nextapi
cargo test --test integration
```

当前沙盒无 Docker daemon，无法执行需要真实 PostgreSQL 的测试；不会在单元测试中伪造数据库行为。

发布验收建议依次验证：迁移、Key/上游/路由 CRUD、网关请求记账、重复 request_id 幂等、分区创建、价格与 FX 回填、WAL 恢复、配额累计及配置优先级。

浏览器验收：

```bash
cd web
npm run typecheck
npm run build
```

完成构建后，在具备浏览器的环境访问服务根路径并执行登录、仪表盘、Key、上游、路由、价格、日志、统计和设置页面检查。

Docker 验收：

```bash
docker build -t nextapi:test .
docker compose up -d
curl -f http://127.0.0.1:8080/healthz
curl -f http://127.0.0.1:8080/readyz
```

`/healthz` 仅表示进程存活；`/readyz` 成功执行数据库探测后才返回 ready。

## 本地验证基线

- `cargo fmt -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --release`
- `cargo build --release`
- `npm run typecheck`
- `npm run build`
- `docker compose config`
- `git diff --check`
