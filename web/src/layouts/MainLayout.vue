<template>
  <el-container class="layout">
    <el-aside
      :width="isMobile ? '248px' : collapsed ? '64px' : '220px'"
      class="aside"
      :class="{ 'aside-mobile': isMobile, 'aside-mobile-open': navOpen }"
    >
      <div class="brand" :class="{ 'brand-collapsed': collapsed && !isMobile }">
        <span class="brand-mark">next</span>
        <div v-show="!collapsed || isMobile" class="brand-text"><strong>NextAPI</strong><small>网关控制台</small></div>
      </div>
      <el-menu :default-active="active" router :collapse="collapsed && !isMobile" :collapse-transition="false"
        background-color="transparent" text-color="#9aa7ae" active-text-color="#ffffff" class="menu"
        popper-class="aside-menu-popup" @select="onMenuSelect">
        <!-- 菜单按「概览 / 接入 / 监控 / 高级」分组展示（index 仍为路由 path，兼容旧书签） -->
        <el-sub-menu v-for="g in menus" :key="g.label" :index="g.label">
          <template #title>
            <el-icon><component :is="g.icon" /></el-icon>
            <span>{{ g.label }}</span>
          </template>
          <el-menu-item v-for="m in g.items" :key="m.path" :index="m.path">
            <el-icon><component :is="m.icon" /></el-icon>
            <template #title><span>{{ m.title }}</span></template>
          </el-menu-item>
        </el-sub-menu>
      </el-menu>
      <div v-show="!collapsed || isMobile" class="aside-version">v{{ version }}</div>
    </el-aside>
    <transition name="mask-fade">
      <div v-if="isMobile && navOpen" class="nav-mask" @click="navOpen = false" />
    </transition>
    <el-container>
      <el-header class="header">
        <div class="header-left">
          <el-icon class="collapse-btn" :title="isMobile ? '打开菜单' : collapsed ? '展开侧栏' : '收起侧栏'"
            @click="onMenuBtn">
            <component :is="menuBtnIcon" />
          </el-icon>
          <div class="page-title">{{ route.meta.title ?? '' }}</div>
        </div>
        <el-dropdown @command="onCommand" trigger="click">
          <span class="user">
            <el-icon><UserFilled /></el-icon>
            <span class="username">{{ auth.username ?? 'admin' }}</span>
            <el-icon><ArrowDown /></el-icon>
          </span>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item command="password">修改密码</el-dropdown-item>
              <el-dropdown-item command="logout" divided>退出登录</el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
      </el-header>
      <el-main class="main">
        <router-view />
      </el-main>
    </el-container>
  </el-container>

  <ChangePwdDialog ref="pwdDlg" />
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessageBox } from 'element-plus'
import { ArrowDown, Expand, Fold, Menu as MenuIcon, UserFilled } from '@element-plus/icons-vue'
import { useAuthStore } from '@/stores/auth'
import { systemApi } from '@/api'
import ChangePwdDialog from '@/components/common/ChangePwdDialog.vue'

const route = useRoute()
const router = useRouter()
const auth = useAuthStore()

// 侧栏菜单分组（低风险 IA 优化：仅调整展示结构，不新增/删除/改名任何路由）
// 单源：router/index.ts 的 meta.title/meta.icon 仍负责标题与图标（review P3）。
type MenuGroupItem = { path: string; title: string; icon: string }
interface MenuGroupDef {
  label: string
  icon: string
  /** 组内路由 path（相对根路径），顺序即组内展示顺序 */
  paths: string[]
}
interface MenuGroup extends MenuGroupDef {
  items: MenuGroupItem[]
}

// 分组顺序即侧栏展示顺序；路由表顺序与组内顺序解耦
const MENU_GROUPS: MenuGroupDef[] = [
  { label: '概览', icon: 'Monitor', paths: ['/dashboard'] },
  { label: '接入', icon: 'Connection', paths: ['/upstreams', '/routes', '/aliases', '/keys'] },
  { label: '监控', icon: 'TrendCharts', paths: ['/logs', '/stats'] },
  { label: '高级', icon: 'Setting', paths: ['/pricing', '/settings'] },
]

const menus = computed<MenuGroup[]>(() => {
  const root = router.options.routes.find((r) => r.path === '/')
  const flat = (root?.children ?? [])
    .filter((c) => typeof c.path === 'string' && c.meta?.title)
    .map(
      (c): MenuGroupItem => ({
        path: `/${c.path}`,
        title: c.meta!.title as string,
        icon: (c.meta!.icon as string) ?? 'Menu',
      }),
    )
  const known = new Set(MENU_GROUPS.flatMap((g) => g.paths))
  // 兜底：未纳入分组的菜单项挂到「其他」，避免将来路由新增后被静默隐藏
  const rest = flat.filter((m) => !known.has(m.path))
  const groups = MENU_GROUPS.map((g) => ({
    ...g,
    items: flat.filter((m) => g.paths.includes(m.path)),
  })).filter((g) => g.items.length > 0)
  if (rest.length) groups.push({ label: '其他', icon: 'Menu', paths: [], items: rest })
  return groups
})

const active = computed(() => route.path)
const collapsed = ref(false)
const navOpen = ref(false)
const pwdDlg = ref<InstanceType<typeof ChangePwdDialog>>()

// 移动端断点：≤768px 时侧栏改为滑出式抽屉（不占据布局宽度）
const MOBILE_MQ = '(max-width: 768px)'
const isMobile = ref(false)
let mq: MediaQueryList | null = null
const onMq = (e: MediaQueryListEvent) => {
  isMobile.value = e.matches
  if (!e.matches) navOpen.value = false
}
// 侧栏底部版本号（来自后端真实二进制版本，失败静默）
const version = ref('')
onMounted(async () => {
  try {
    version.value = (await systemApi.version()).version
  } catch {
    /* 忽略：版本号仅为展示 */
  }
})

onMounted(() => {
  mq = window.matchMedia(MOBILE_MQ)
  isMobile.value = mq.matches
  mq.addEventListener('change', onMq)
})
onBeforeUnmount(() => mq?.removeEventListener('change', onMq))

const menuBtnIcon = computed(() => {
  if (isMobile.value) return MenuIcon
  return collapsed.value ? Expand : Fold
})

function onMenuBtn() {
  if (isMobile.value) navOpen.value = !navOpen.value
  else collapsed.value = !collapsed.value
}

function onMenuSelect() {
  if (isMobile.value) navOpen.value = false
}

async function onCommand(cmd: string) {
  if (cmd === 'logout') {
    try {
      await ElMessageBox.confirm('确认退出登录？', '提示', { type: 'warning' })
    } catch {
      return
    }
    auth.logout()
    router.push('/login')
  } else if (cmd === 'password') {
    pwdDlg.value?.open()
  }
}
</script>

<style scoped>
.layout {
  height: 100%;
}

/* ---------- 侧栏（深色） ---------- */
.aside {
  position: relative;
  background: #17202a;
  transition: width .2s ease;
  overflow: hidden;
  flex-shrink: 0;
}
.brand {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 18px;
  white-space: nowrap;
}
.brand-collapsed {
  justify-content: center;
  padding: 18px 0;
}
.brand-mark {
  flex-shrink: 0;
  height: 30px;
  padding: 0 10px;
  display: grid;
  place-items: center;
  border-radius: 8px;
  background: #3d444c;
  color: #fff;
  font-weight: 800;
  font-size: 14px;
  letter-spacing: .5px;
  line-height: 1;
  box-shadow: 0 2px 8px rgba(23, 32, 42, .35);
}
.brand strong { display: block; font-size: 15px; letter-spacing: .2px; color: #f5f7f8; }
.brand small { display: block; margin-top: 2px; color: #7c8890; font-size: 11px; }
.menu {
  border-right: none;
  padding: 4px 8px;
  --el-menu-item-height: 42px;
}
.aside-version {
  position: absolute;
  bottom: 12px;
  left: 0;
  right: 0;
  text-align: center;
  color: #5b6874;
  font-size: 11px;
  letter-spacing: .3px;
  user-select: none;
}
.menu :deep(.el-menu-item) {
  border-radius: 8px;
  margin: 2px 0;
}
.menu :deep(.el-menu-item:hover) {
  background-color: rgba(255, 255, 255, .07);
  color: #e8edef;
}
.menu :deep(.el-menu-item.is-active) {
  background-color: rgba(255, 255, 255, .12);
  color: #fff;

}
/* 分组标题（el-sub-menu）与菜单项一致的圆角/悬停反馈 */
.menu :deep(.el-sub-menu__title) {
  border-radius: 8px;
  margin: 2px 0;
}
.menu :deep(.el-sub-menu__title:hover) {
  background-color: rgba(255, 255, 255, .07);
  color: #e8edef;
}

/* ---------- 移动端：侧栏变滑出抽屉 ---------- */
.aside-mobile {
  position: fixed;
  left: 0;
  top: 0;
  bottom: 0;
  z-index: 1001;
  transform: translateX(-100%);
  transition: transform .22s ease;
  box-shadow: 8px 0 24px rgba(0, 0, 0, .3);
}
.aside-mobile-open {
  transform: translateX(0);
}
.nav-mask {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(8, 12, 16, .5);
}
.mask-fade-enter-active,
.mask-fade-leave-active {
  transition: opacity .2s ease;
}
.mask-fade-enter-from,
.mask-fade-leave-to {
  opacity: 0;
}

/* ---------- 顶栏 ---------- */
.header {
  background: #fff;
  border-bottom: 1px solid #e8eaee;
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 56px;
}
.header-left {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
}
.collapse-btn {
  font-size: 18px;
  color: #4b5563;
  cursor: pointer;
  padding: 6px;
  border-radius: 6px;
}
.collapse-btn:hover {
  background: #f3f4f6;
  color: #1f2329;
}
.page-title {
  font-size: 16px;
  font-weight: 600;
  color: #1f2329;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.user {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  cursor: pointer;
  color: #4b5563;
}
.main {
  padding: 0;
  overflow-y: auto;
  background: #f4f5f7;
}

@media (max-width: 768px) {
  .header { padding: 0 12px; }
  .page-title { font-size: 15px; }
}
@media (max-width: 400px) {
  .username { display: none; }
}
</style>

<!-- 收起态（仅图标 64px）分组弹出面板由 el-sub-menu 渲染到 body，需非 scoped 才能命中 -->
<style>
.aside-menu-popup.el-popper {
  border: none;
  border-radius: 8px;
  padding: 4px;
  box-shadow: 0 6px 18px rgba(0, 0, 0, .28);
}
.aside-menu-popup .el-menu--popup {
  background-color: #1d2731;
  min-width: 160px;
  padding: 0;
}
.aside-menu-popup .el-menu-item {
  border-radius: 6px;
  margin: 2px 4px;
  color: #9aa7ae;
}
.aside-menu-popup .el-menu-item:hover {
  background-color: rgba(255, 255, 255, .07);
  color: #e8edef;
}
.aside-menu-popup .el-menu-item.is-active {
  background-color: rgba(255, 255, 255, .12);
  color: #fff;
}
.aside-menu-popup .el-popper__arrow::before {
  background-color: #1d2731 !important;
  border-color: #1d2731 !important;
}
</style>
