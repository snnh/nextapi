<template>
  <a class="skip-link" href="#main">跳到主内容</a>
  <el-container class="layout">
    <el-aside
      :width="isMobile ? '248px' : collapsed ? '64px' : '220px'"
      class="aside"
      :class="{ 'aside-mobile': isMobile, 'aside-mobile-open': navOpen }"
      :role="isMobile ? 'dialog' : undefined"
      :aria-modal="isMobile && navOpen ? 'true' : undefined"
      :aria-hidden="isMobile && !navOpen ? 'true' : undefined"
      :aria-label="isMobile ? '导航菜单' : undefined"
      :inert="isMobile && !navOpen ? '' : undefined"
      id="app-nav"
    >
      <div ref="asideRef" class="aside-inner">
        <el-button
          v-if="isMobile"
          class="drawer-close"
          link
          :icon="Close"
          aria-label="关闭菜单"
          @click="closeNav"
        />
        <div class="brand" :class="{ 'brand-collapsed': collapsed && !isMobile }">
          <span class="brand-mark">next</span>
          <div v-show="!collapsed || isMobile" class="brand-text"><strong>NextAPI</strong><small>网关控制台</small></div>
        </div>
        <el-menu :default-active="active" router :collapse="collapsed && !isMobile" :collapse-transition="false"
          class="menu" popper-class="aside-menu-popup" @select="onMenuSelect">
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
      </div>
    </el-aside>
    <transition name="mask-fade">
      <div v-if="isMobile && navOpen" class="nav-mask" @click="closeNav" />
    </transition>
    <el-container>
      <el-header class="header">
        <div class="header-left">
          <el-button
            class="collapse-btn"
            link
            :icon="menuBtnIcon"
            :title="isMobile ? '打开菜单' : collapsed ? '展开侧栏' : '收起侧栏'"
            :aria-label="isMobile ? '打开菜单' : collapsed ? '展开侧栏' : '收起侧栏'"
            :aria-expanded="isMobile ? navOpen : !collapsed"
            aria-controls="app-nav"
            @click="onMenuBtn"
          />
          <div class="page-title">{{ route.meta.title ?? '' }}</div>
        </div>
        <div class="header-right">
          <SystemStatusBadge />
          <ThemeToggle />
          <el-dropdown @command="onCommand" trigger="click">
            <el-button class="user" link :aria-label="`用户菜单：${auth.username ?? 'admin'}`">
              <el-icon><UserFilled /></el-icon>
              <span class="username">{{ auth.username ?? 'admin' }}</span>
              <el-icon><ArrowDown /></el-icon>
            </el-button>
            <template #dropdown>
              <el-dropdown-menu>
                <el-dropdown-item command="password">修改密码</el-dropdown-item>
                <el-dropdown-item command="logout" divided>退出登录</el-dropdown-item>
              </el-dropdown-menu>
            </template>
          </el-dropdown>
        </div>
      </el-header>
      <el-main id="main" class="main" tabindex="-1">
        <router-view />
      </el-main>
    </el-container>
  </el-container>

  <ChangePwdDialog ref="pwdDlg" />
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessageBox } from 'element-plus'
import { ArrowDown, Close, Expand, Fold, Menu as MenuIcon, UserFilled } from '@element-plus/icons-vue'
import { useAuthStore } from '@/stores/auth'
import { systemApi } from '@/api'
import ChangePwdDialog from '@/components/common/ChangePwdDialog.vue'
import SystemStatusBadge from '@/components/common/SystemStatusBadge.vue'
import ThemeToggle from '@/components/common/ThemeToggle.vue'
import { useBreakpoint } from '@/composables/useBreakpoint'

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
const NAV_COLLAPSED_KEY = 'nextapi.nav.collapsed'
const collapsed = ref(readCollapsed())
const navOpen = ref(false)
const pwdDlg = ref<InstanceType<typeof ChangePwdDialog>>()
const asideRef = ref<HTMLElement | null>(null)

// 桌面侧栏收起偏好持久化；移动端抽屉状态不落盘（两种形态互不影响）
function readCollapsed(): boolean {
  try {
    return localStorage.getItem(NAV_COLLAPSED_KEY) === '1'
  } catch {
    return false
  }
}
function saveCollapsed(v: boolean) {
  try {
    localStorage.setItem(NAV_COLLAPSED_KEY, v ? '1' : '0')
  } catch {
    /* 忽略：隐私模式等场景 localStorage 不可用 */
  }
}

// 移动端（≤800px）侧栏改为滑出式抽屉；断点与全站 useBreakpoint 同源
const { isMobile } = useBreakpoint()
watch(isMobile, (mobile) => {
  if (!mobile) navOpen.value = false
})
// 侧栏底部版本号（来自后端真实二进制版本，失败静默）
const version = ref('')
onMounted(async () => {
  try {
    version.value = (await systemApi.version()).version
  } catch {
    /* 忽略：版本号仅为展示 */
  }
})

onBeforeUnmount(() => {
  document.removeEventListener('keydown', onDrawerKeydown)
})

const menuBtnIcon = computed(() => {
  if (isMobile.value) return MenuIcon
  return collapsed.value ? Expand : Fold
})

function onMenuBtn() {
  if (isMobile.value) {
    if (navOpen.value) closeNav()
    else openNav()
  } else {
    collapsed.value = !collapsed.value
    saveCollapsed(collapsed.value)
  }
}

function onMenuSelect() {
  if (isMobile.value) closeNav()
}

// 移动端抽屉：Esc 关闭、焦点陷阱、关闭后焦点回归触发按钮
const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'
let lastFocused: HTMLElement | null = null

function focusableNodes(): HTMLElement[] {
  const root = asideRef.value
  if (!root) return []
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => el.getAttribute('aria-hidden') !== 'true' && el.getClientRects().length > 0,
  )
}

function openNav() {
  lastFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null
  navOpen.value = true
}

function closeNav() {
  navOpen.value = false
}

function onDrawerKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    closeNav()
    return
  }
  if (event.key !== 'Tab') return
  const nodes = focusableNodes()
  if (!nodes.length) return
  const first = nodes[0]
  const last = nodes[nodes.length - 1]
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault()
    last.focus()
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault()
    first.focus()
  }
}

watch([navOpen, isMobile], async ([open, mobile]) => {
  document.removeEventListener('keydown', onDrawerKeydown)
  if (!mobile) return
  if (open) {
    await nextTick()
    focusableNodes()[0]?.focus()
    document.addEventListener('keydown', onDrawerKeydown)
  } else {
    await nextTick()
    lastFocused?.focus()
  }
})

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
  background: var(--nx-canvas);
}

/* ---------- 侧栏：跟随主题，无阴影 ---------- */
.aside {
  position: relative;
  background: var(--nx-sidebar-bg);
  border-right: 1px solid var(--nx-sidebar-border);
  transition: width var(--nx-dur-slow) var(--nx-ease);
  overflow: hidden;
  flex-shrink: 0;
}
.aside-inner {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}
.drawer-close {
  position: absolute;
  top: 12px;
  right: 8px;
  z-index: 1;
  width: 36px;
  height: 36px;
  padding: 0;
  color: var(--nx-sidebar-text);
}
.drawer-close:hover {
  background: var(--nx-sidebar-hover);
  color: var(--nx-sidebar-strong);
}
.brand {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 18px;
  white-space: nowrap;
  flex-shrink: 0;
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
  border-radius: var(--nx-radius);
  background: var(--nx-accent-solid);
  color: var(--nx-on-accent);
  font-weight: 800;
  font-size: var(--nx-font-body);
  letter-spacing: 0.5px;
  line-height: 1;
}
.brand strong {
  display: block;
  font-size: 15px;
  letter-spacing: 0.2px;
  color: var(--nx-sidebar-strong);
}
.brand small {
  display: block;
  margin-top: 2px;
  color: var(--nx-sidebar-muted);
  font-size: 11px;
}
.menu {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
  border-right: none;
  padding: 4px 8px;
  --el-menu-item-height: 42px;
  --el-menu-bg-color: transparent;
  --el-menu-text-color: var(--nx-sidebar-text);
  --el-menu-active-color: var(--nx-sidebar-strong);
  --el-menu-hover-bg-color: var(--nx-sidebar-hover);
  --el-menu-hover-text-color: var(--nx-sidebar-strong);
}
.aside-version {
  flex-shrink: 0;
  padding: 12px 0 14px;
  text-align: center;
  color: var(--nx-sidebar-muted);
  font-size: 11px;
  letter-spacing: 0.3px;
  user-select: none;
}
.menu :deep(.el-menu-item) {
  border-radius: var(--nx-radius);
  margin: 2px 0;
}
.menu :deep(.el-menu-item:hover) {
  background-color: var(--nx-sidebar-hover);
  color: var(--nx-sidebar-strong);
}
.menu :deep(.el-menu-item.is-active) {
  background-color: var(--nx-sidebar-active-bg);
  color: var(--nx-sidebar-active-text);
  box-shadow: inset 3px 0 0 var(--nx-sidebar-accent);
}
.menu :deep(.el-menu-item:focus-visible),
.menu :deep(.el-sub-menu__title:focus-visible) {
  outline: 2px solid var(--nx-focus-ring);
  outline-offset: -2px;
}
/* 分组标题（el-sub-menu）与菜单项一致的圆角/悬停反馈 */
.menu :deep(.el-sub-menu__title) {
  border-radius: var(--nx-radius);
  margin: 2px 0;
}
.menu :deep(.el-sub-menu__title:hover) {
  background-color: var(--nx-sidebar-hover);
  color: var(--nx-sidebar-strong);
}

/* ---------- 移动端：侧栏变滑出抽屉 ---------- */
.aside-mobile {
  position: fixed;
  left: 0;
  top: 0;
  bottom: 0;
  z-index: 1001;
  transform: translateX(-100%);
  transition: transform var(--nx-dur-slow) var(--nx-ease);
}
.aside-mobile-open {
  transform: translateX(0);
}
.nav-mask {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: var(--nx-mask);
}
.mask-fade-enter-active,
.mask-fade-leave-active {
  transition: opacity var(--nx-dur-slow) var(--nx-ease);
}
.mask-fade-enter-from,
.mask-fade-leave-to {
  opacity: 0;
}

/* ---------- 顶栏 ---------- */
.header {
  background: var(--nx-surface);
  border-bottom: 1px solid var(--nx-border);
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 56px;
  padding: 0 20px;
  transition: background-color var(--nx-dur-base) var(--nx-ease),
    border-color var(--nx-dur-base) var(--nx-ease);
}
.header-left {
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 0;
}
.header-right {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}
.collapse-btn {
  width: var(--el-component-size);
  height: var(--el-component-size);
  padding: 0;
  font-size: var(--el-font-size-large);
  color: var(--nx-text-2);
  border-radius: var(--nx-radius);
}
.collapse-btn:hover {
  background: var(--nx-hover);
  color: var(--nx-text);
}
.page-title {
  font-size: var(--nx-font-title);
  font-weight: 600;
  color: var(--nx-text);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.user {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: auto;
  padding: 0;
  color: var(--nx-text-2);
}
.user:hover {
  color: var(--nx-text);
  background: transparent;
}
.main {
  padding: 0;
  overflow-y: auto;
  background: var(--nx-canvas);
  outline: none;
}

@media (max-width: 800px) {
  .header {
    padding: 0 12px;
  }
  .header-left {
    gap: 8px;
  }
  .page-title {
    font-size: var(--nx-font-body);
  }
  .collapse-btn {
    width: 40px;
    height: 40px;
  }
}
@media (max-width: 400px) {
  .username {
    display: none;
  }
}
</style>

<!-- 收起态（仅图标 64px）分组弹出面板由 el-sub-menu 渲染到 body，需非 scoped 才能命中 -->
<style>
.aside-menu-popup.el-popper {
  border: 1px solid var(--nx-border);
  border-radius: var(--nx-radius);
  padding: 4px;
  background: var(--nx-floating);
  box-shadow: var(--nx-shadow-pop);
}
.aside-menu-popup .el-menu--popup {
  background-color: var(--nx-floating);
  min-width: 160px;
  padding: 0;
}
.aside-menu-popup .el-menu-item {
  border-radius: var(--nx-radius-sm);
  margin: 2px 4px;
  color: var(--nx-sidebar-text);
}
.aside-menu-popup .el-menu-item:hover {
  background-color: var(--nx-sidebar-hover);
  color: var(--nx-sidebar-strong);
}
.aside-menu-popup .el-menu-item.is-active {
  background-color: var(--nx-sidebar-active-bg);
  color: var(--nx-sidebar-active-text);
}
.aside-menu-popup .el-popper__arrow::before {
  background-color: var(--nx-floating) !important;
  border-color: var(--nx-border) !important;
}
</style>