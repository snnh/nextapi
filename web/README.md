# NextAPI Web 前端（Vue 3 + Element Plus）

M8 管理后台。技术栈：Vue 3 / TypeScript / Vite / Element Plus / ECharts / Pinia / Vue Router / axios / dayjs / js-yaml。

## 开发

```bash
npm install
npm run dev        # http://localhost:5173，/api 代理到 http://127.0.0.1:8080（后端）
```

后端需先运行（PostgreSQL + cargo run）。代理目标可用 `VITE_PROXY_TARGET` 覆盖。

## 构建

```bash
npm run build      # 产物 web/dist
```

构建产物由后端 rust-embed 内嵌进单二进制（Docker 多阶段构建），管理后台与 API 同源部署。

## 代码结构

```
src/
├── api/          # HTTP 封装：http.ts（axios 实例/401 处理）、types.ts（后端 API 类型，唯一事实源）、
│                 # index.ts（分组 API 函数，签名冻结；页面禁止裸 axios）
├── stores/       # pinia：auth.ts（token/username，localStorage 持久化）
├── router/       # 路由表 + 登录守卫
├── layouts/      # MainLayout.vue（侧边栏导航 + 顶栏）
├── views/        # 9 个页面：Login/Dashboard/Keys/Upstreams/Routes/Pricing/Logs/Stats/Settings
├── components/   # 共享 EChart.vue；页面私有组件按 components/<分组>/ 前缀目录
└── utils/        # format.ts（时间/金额）、consts.ts（协议/单位常量）、download.ts（文件下载）
```

## E2E 冒烟（待真实环境）

后端 + PostgreSQL 就绪后人工冒烟路径：登录 → 仪表盘 → 各页面增删改查 → 日志分区 dry-run 清理。
Playwright 自动化用例留待 M9 测试阶段补充（沙盒无 PostgreSQL/浏览器下载环境）。
