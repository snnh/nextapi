// 路由表（M8 契约冻结：路径与页面文件一一对应）+ 登录守卫
import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router'
import { useAuthStore } from '@/stores/auth'

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
    redirect: '/dashboard',
    children: [
      {
        path: 'dashboard',
        name: 'dashboard',
        component: () => import('@/views/Dashboard.vue'),
        meta: { title: '仪表盘', icon: 'Monitor' },
      },
      {
        path: 'keys',
        name: 'keys',
        component: () => import('@/views/Keys.vue'),
        meta: { title: 'API 密钥', icon: 'Key' },
      },
      {
        path: 'upstreams',
        name: 'upstreams',
        component: () => import('@/views/Upstreams.vue'),
        meta: { title: '上游供应商', icon: 'Connection' },
      },
      {
        path: 'routes',
        name: 'routes',
        component: () => import('@/views/Routes.vue'),
        meta: { title: '模型路由', icon: 'Operation' },
      },
      {
        path: 'pricing',
        name: 'pricing',
        component: () => import('@/views/Pricing.vue'),
        meta: { title: '价格管理', icon: 'Coin' },
      },
      {
        path: 'logs',
        name: 'logs',
        component: () => import('@/views/Logs.vue'),
        meta: { title: '请求日志', icon: 'Document' },
      },
      {
        path: 'stats',
        name: 'stats',
        component: () => import('@/views/Stats.vue'),
        meta: { title: '统计报表', icon: 'DataAnalysis' },
      },
      {
        path: 'settings',
        name: 'settings',
        component: () => import('@/views/Settings.vue'),
        meta: { title: '系统设置', icon: 'Setting' },
      },
    ],
  },
  { path: '/:pathMatch(.*)*', redirect: '/dashboard' },
]

const router = createRouter({
  history: createWebHistory(),
  routes,
})

router.beforeEach((to) => {
  const auth = useAuthStore()
  if (!to.meta.public && !auth.loggedIn) {
    return { path: '/login', query: { redirect: to.fullPath } }
  }
  if (to.path === '/login' && auth.loggedIn) {
    return { path: '/dashboard' }
  }
  return true
})

export default router
