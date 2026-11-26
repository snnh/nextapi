// 路由表（M8 契约冻结：路径与页面文件一一对应）+ 登录守卫
// M15 §6.4：history base 取部署前缀（<base href>），支持根路径与子路径反代
import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { appBase } from '@/utils/base'

const routes: RouteRecordRaw[] = [
  {
    path: '/login',
    name: 'login',
    component: () => import('@/views/Login.vue'),
    meta: { title: '登录', public: true },
  },
  {
    path: '/',
    component: () => import('@/layouts/MainLayout.vue'),
    redirect: '/usage',
    children: [
      {
        path: 'usage',
        name: 'usage',
        component: () => import('@/views/UsageView.vue'),
        meta: { title: '用量与日志', icon: 'DataAnalysis' },
      },
      {
        path: 'models',
        name: 'models',
        component: () => import('@/views/ModelsView.vue'),
        meta: { title: '模型接入', icon: 'Connection' },
      },
      {
        path: 'keys',
        name: 'keys',
        component: () => import('@/views/Keys.vue'),
        meta: { title: '密钥', icon: 'Key' },
      },
      {
        path: 'settings',
        name: 'settings',
        component: () => import('@/views/Settings.vue'),
        meta: { title: '系统设置', icon: 'Setting' },
      },
    ],
  },
  // M20：旧路径不再单独成页，统一兜底到 /usage（不做 redirect 映射表）
  { path: '/:pathMatch(.*)*', redirect: '/usage' },
]

const router = createRouter({
  history: createWebHistory(appBase()),
  routes,
})

router.beforeEach((to) => {
  const auth = useAuthStore()
  if (!to.meta.public && !auth.loggedIn) {
    return { path: '/login', query: { redirect: to.fullPath } }
  }
  if (to.path === '/login' && auth.loggedIn) {
    return { path: '/usage' }
  }
  return true
})

export default router
