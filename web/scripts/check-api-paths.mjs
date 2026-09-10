// 构建前自检（防回归）：apiBase() 只能返回「部署前缀」，不能拼接 "api"。
//
// 背景：api/index.ts 中所有请求路径都已包含 /api 前缀（如 '/api/auth/login'），
// 而 axios 会把 baseURL 与 url 拼接（combineURLs）。若 apiBase() 返回 "/api"，
// 实际请求会变成 /api/api/... → 全站 404（v0.3.0 线上回归，0.3.1 修复）。
import { readFileSync } from 'node:fs'

const baseTs = readFileSync(new URL('../src/utils/base.ts', import.meta.url), 'utf8')

// 粗粒度但有效的守卫：检测 `${appBase()}api` / appBase() + 'api' 之类的拼接
if (/\$\{appBase\(\)\}\s*api|appBase\(\)\s*\+\s*['"]api/.test(baseTs)) {
  console.error(
    '[check-api-paths] apiBase() 不得拼接 "api"：api/index.ts 的路径已含 /api 前缀，' +
      '会拼成 /api/api/... 导致全站 404。',
  )
  process.exit(1)
}

console.log('[check-api-paths] OK：apiBase() 仅返回部署前缀')
