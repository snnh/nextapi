<template>
  <el-container class="layout">
    <el-aside width="220px" class="aside">
      <div class="logo">NextAPI</div>
      <el-menu :default-active="active" router background-color="#001529"
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
import {
  ArrowDown,
  Coin,
  Connection,
  DataAnalysis,
  Document,
  Key,
  Monitor,
  Operation,
  Setting,
  UserFilled,
} from '@element-plus/icons-vue'
import { useAuthStore } from '@/stores/auth'
import ChangePwdDialog from '@/components/common/ChangePwdDialog.vue'

const route = useRoute()
const router = useRouter()
const auth = useAuthStore()

const menus = [
  { path: '/dashboard', title: '仪表盘', icon: Monitor },
  { path: '/keys', title: 'API 密钥', icon: Key },
  { path: '/upstreams', title: '上游供应商', icon: Connection },
  { path: '/routes', title: '模型路由', icon: Operation },
  { path: '/pricing', title: '价格管理', icon: Coin },
  { path: '/logs', title: '请求日志', icon: Document },
  { path: '/stats', title: '统计报表', icon: DataAnalysis },
  { path: '/settings', title: '系统设置', icon: Setting },
]

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
.aside {
  background: #001529;
}
.logo {
  color: #fff;
  font-size: 20px;
  font-weight: 700;
  padding: 18px 20px;
  letter-spacing: 1px;
}
.menu {
  border-right: none;
}
.header {
  background: #fff;
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
