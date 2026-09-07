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
        @select="onMenuSelect">
        <el-menu-item v-for="m in menus" :key="m.path" :index="m.path">
          <el-icon><component :is="m.icon" /></el-icon>
          <template #title><span>{{ m.title }}</span></template>
        </el-menu-item>
      </el-menu>
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
import ChangePwdDialog from '@/components/common/ChangePwdDialog.vue'

const route = useRoute()
const router = useRouter()
const auth = useAuthStore()

// 菜单从路由表派生（单源：router/index.ts 的 meta.title/meta.icon，review P3）
const menus = computed(() => {
  const root = router.options.routes.find((r) => r.path === '/')
  return (root?.children ?? [])
    .filter((c) => typeof c.path === 'string' && c.meta?.title)
    .map((c) => ({
      path: `/${c.path}`,
      title: c.meta!.title as string,
      icon: (c.meta!.icon as string) ?? 'Menu',
    }))
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
