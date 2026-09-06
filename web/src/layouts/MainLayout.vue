<template>
  <el-container class="layout">
    <el-aside width="232px" class="aside">
      <div class="brand"><span class="brand-mark">N</span><div><strong>NextAPI</strong><small>Gateway console</small></div></div>
      <el-menu :default-active="active" router background-color="#1f2421"
        text-color="rgba(255,255,255,.68)" active-text-color="#fff" class="menu">
        <el-menu-item v-for="m in menus" :key="m.path" :index="m.path">
          <el-icon><component :is="m.icon" /></el-icon>
          <span>{{ m.title }}</span>
        </el-menu-item>
      </el-menu>
    </el-aside>
    <el-container>
      <el-header class="header">
        <div class="page-title">{{ route.meta.title ?? '' }}</div>
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
import { ArrowDown, UserFilled } from '@element-plus/icons-vue'
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
  .aside { width: 64px !important; }
  .brand { justify-content: center; padding: 18px 8px; }
  .brand > div:last-child, .menu span { display: none; }
  .menu .el-menu-item { justify-content: center; padding: 0 !important; }
  .header { padding: 0 14px; }
  .page-title { font-size: 15px; }
}

.aside {
  background: #1f2421;
}
.brand {
  color: #f5f3ed;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 20px 18px;
}
.brand-mark {
  width: 30px;
  height: 30px;
  display: grid;
  place-items: center;
  border-radius: 8px;
  background: #d5f36f;
  color: #1f2421;
  font-weight: 800;
}
.brand strong { display: block; font-size: 17px; letter-spacing: .2px; }
.brand small { display: block; margin-top: 2px; color: #a8ada6; font-size: 10px; }
.menu {
  border-right: none;
}
.header {
  background: rgba(255, 255, 255, .92);
  border-bottom: 1px solid #e7e5e0;
  display: flex;
  align-items: center;
  justify-content: space-between;
  box-shadow: 0 1px 4px rgba(0, 21, 41, 0.08);
}
.page-title {
  font-size: 16px;
  font-weight: 600;
}
.user {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  cursor: pointer;
  color: #303133;
}
.main {
  padding: 0;
  overflow-y: auto;
  background: #f0f2f5;
}
</style>
