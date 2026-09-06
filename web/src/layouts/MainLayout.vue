<template>
  <el-container class="layout">
    <el-aside :width="collapsed ? '64px' : '220px'" class="aside">
      <div class="brand" :class="{ 'brand-collapsed': collapsed }">
        <span class="brand-mark">N</span>
        <div v-show="!collapsed" class="brand-text"><strong>NextAPI</strong><small>网关控制台</small></div>
      </div>
      <el-menu :default-active="active" router :collapse="collapsed" :collapse-transition="false"
        background-color="transparent" text-color="#4b5563" active-text-color="#3a6ff7" class="menu">
        <el-menu-item v-for="m in menus" :key="m.path" :index="m.path">
          <el-icon><component :is="m.icon" /></el-icon>
          <template #title><span>{{ m.title }}</span></template>
        </el-menu-item>
      </el-menu>
    </el-aside>
    <el-container>
      <el-header class="header">
        <div class="header-left">
          <el-icon class="collapse-btn" :title="collapsed ? '展开侧栏' : '收起侧栏'" @click="collapsed = !collapsed">
            <Expand v-if="collapsed" /><Fold v-else />
          </el-icon>
          <div class="page-title">{{ route.meta.title ?? '' }}</div>
        </div>
        <el-dropdown @command="onCommand">
          <span class="user">
            <el-icon><UserFilled /></el-icon>
            {{ auth.username ?? 'admin' }}
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
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessageBox } from 'element-plus'
import { ArrowDown, Expand, Fold, UserFilled } from '@element-plus/icons-vue'
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
const pwdDlg = ref<InstanceType<typeof ChangePwdDialog>>()

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
@media (max-width: 720px) {
  .brand { justify-content: center; padding: 18px 8px; }
  .brand-text, .menu span { display: none; }
  .menu .el-menu-item { justify-content: center; padding: 0 !important; }
  .header { padding: 0 14px; }
  .page-title { font-size: 15px; }
}

.aside {
  background: #fff;
  border-right: 1px solid #e5e7eb;
  transition: width .2s ease;
  overflow: hidden;
}
.brand {
  color: #1f2329;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 20px 18px;
  white-space: nowrap;
}
.brand-collapsed {
  justify-content: center;
  padding: 20px 0;
}
.brand-mark {
  flex-shrink: 0;
  width: 30px;
  height: 30px;
  display: grid;
  place-items: center;
  border-radius: 7px;
  background: #3a6ff7;
  color: #fff;
  font-weight: 700;
}
.brand strong { display: block; font-size: 16px; letter-spacing: .2px; }
.brand small { display: block; margin-top: 2px; color: #9ca3af; font-size: 11px; }
.menu {
  border-right: none;
}
.header {
  background: #fff;
  border-bottom: 1px solid #e5e7eb;
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.header-left {
  display: flex;
  align-items: center;
  gap: 14px;
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
  background: #f5f6f8;
}
</style>
