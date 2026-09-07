# NextAPI Web 前端

管理后台，Vue 3 + TypeScript + Vite + Element Plus + ECharts（Pinia / Vue Router / axios / dayjs / js-yaml）。

## 开发

```bash
npm install
npm run dev        # http://localhost:5173，/api 代理到 http://127.0.0.1:3220
```

后端需先运行（PostgreSQL + `cargo run`）。代理目标可用 `VITE_PROXY_TARGET` 覆盖。

## 构建

```bash
npm run build      # 产物 web/dist
npm run typecheck  # vue-tsc
```

`web/dist` 由后端 rust-embed 内嵌进单二进制（Docker 多阶段构建），管理后台与 API 同源部署。
注意：后端的 `cargo build` / `cargo test` 编译期会读 `web/dist`，目录缺失会编译失败。

## 代码结构

```
src/
├── api/          # http.ts（axios 实例 / 401 拦截）、types.ts（后端接口类型，唯一事实源）、
│                 # index.ts（分组 API 函数；页面不直接用 axios）
├── stores/       # auth.ts（token/username，localStorage 持久化）
├── router/       # 路由表 + 登录守卫
├── layouts/      # MainLayout.vue（侧栏导航 + 顶栏 + 移动端抽屉）
├── views/        # 9 个页面：Login/Dashboard/Keys/Upstreams/Routes/Pricing/Logs/Stats/Settings
├── components/   # 共享 EChart.vue；页面私有组件按 components/<分组>/ 目录（pricing/settings/upstreams/stats）
└── utils/        # format.ts（时间/金额）、consts.ts（协议/单位常量）、download.ts（文件下载）
```
