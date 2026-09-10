// ============================================================================
// 部署前缀工具（M15 §6.4 子路径部署）
//
// 后端在返回 index.html 时按 X-Forwarded-Prefix / 请求路径注入 <base href="...">，
// 前端据此推导路由 base 与 API base，从而同时支持「根路径」与「/nextapi/」等
// 子路径挂载，无需为不同前缀重新构建前端。
//
// 约定：appBase() 恒以 "/" 结尾（根路径为 "/"）。
// ============================================================================

/** 应用部署前缀（来自 <base href>，缺省 "/"） */
export function appBase(): string {
  try {
    const p = new URL(document.baseURI).pathname
    return p.endsWith('/') ? p : `${p}/`
  } catch {
    return '/'
  }
}

/**
 * axios baseURL：**只提供部署前缀**（根路径 `/`，子路径 `/nextapi/`）。
 *
 * ⚠️ 关键约定：`api/index.ts` 中所有请求路径都已包含 `/api` 前缀（如 `/api/auth/login`），
 * 而 axios 会把 baseURL 与 url 拼接（`combineURLs`）。因此这里**不能再加 `api`**，
 * 否则会拼成 `/api/api/...` 导致全站 404（v0.3.0 线上回归的根因，0.3.1 修复）。
 *
 * 构建期可用 `VITE_API_BASE` 覆盖（如跨域部署：填完整前缀 `https://api.example.com`）。
 */
export function apiBase(): string {
  const env = import.meta.env.VITE_API_BASE
  if (env) return env
  return appBase()
}

/** 拼接到部署前缀下的绝对路径（用于整页导航等浏览器级跳转） */
export function appPath(path: string): string {
  return `${appBase()}${path.replace(/^\//, '')}`
}

/** 网关调用地址前缀（如 `https://host/nextapi/v1`），用于调用示例展示 */
export function gatewayBase(): string {
  return `${window.location.origin}${appBase()}v1`
}
