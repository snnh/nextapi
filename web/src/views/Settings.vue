<template>
  <div class="page" v-loading="settingsLoading">
    <div class="page-card tabs-card">
      <!-- 初始加载失败：错误条 + 重试 -->
      <el-alert v-if="loadError" class="load-error" type="error" show-icon :closable="false">
        <div class="load-error-body">
          <span class="err-text">运行参数加载失败：{{ loadError }}</span>
          <el-button size="small" type="primary" plain :loading="settingsLoading" @click="loadSettings">
            重试
          </el-button>
        </div>
      </el-alert>

      <el-tabs v-model="activeTab" :before-leave="onBeforeLeave">
      <el-tab-pane label="运行参数" name="runtime">
        <runtime-params-panel ref="runtimePanelRef" :settings="settings" @saved="loadSettings" />
      </el-tab-pane>
      <el-tab-pane label="代理管理" name="proxy" lazy>
        <proxy-panel :settings="settings" />
      </el-tab-pane>
      <el-tab-pane label="更新检查" name="update" lazy>
        <update-check-panel :settings="settings" />
      </el-tab-pane>
      <el-tab-pane label="审计日志" name="audit" lazy>
        <audit-panel />
      </el-tab-pane>
      <el-tab-pane label="安全" name="security" lazy>
        <totp-panel />
      </el-tab-pane>
      <el-tab-pane label="系统" name="system" lazy>
        <system-info-panel />
      </el-tab-pane>
      </el-tabs>
    </div>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ElMessageBox } from 'element-plus'
import { settingsApi } from '@/api'
import { errMsg } from '@/api/http'
import type { SettingItem } from '@/api/types'
import RuntimeParamsPanel from '@/components/settings/RuntimeParamsPanel.vue'
import ProxyPanel from '@/components/settings/ProxyPanel.vue'
import UpdateCheckPanel from '@/components/settings/UpdateCheckPanel.vue'
import AuditPanel from '@/components/settings/AuditPanel.vue'
import SystemInfoPanel from '@/components/settings/SystemInfoPanel.vue'
import TotpPanel from '@/components/settings/TotpPanel.vue'

/** 页签名（对应 el-tab-pane name），用于 ?tab= 深链恢复 */
const TAB_NAMES = ['runtime', 'proxy', 'update', 'audit', 'security', 'system'] as const
type TabName = (typeof TAB_NAMES)[number]

const route = useRoute()
const router = useRouter()

const runtimePanelRef = ref<InstanceType<typeof RuntimeParamsPanel> | null>(null)
const activeTab = ref('runtime')
const settings = ref<SettingItem[]>([])
const settingsLoading = ref(false)
const loadError = ref('')

/** 从 URL query 解析初始页签，非法/缺失回退 runtime */
function tabFromQuery(): TabName {
  const raw = route.query.tab
  const v = typeof raw === 'string' ? raw : ''
  return (TAB_NAMES as readonly string[]).includes(v) ? (v as TabName) : 'runtime'
}

async function loadSettings() {
  settingsLoading.value = true
  loadError.value = ''
  try {
    settings.value = await settingsApi.view()
  } catch (e) {
    loadError.value = errMsg(e)
  } finally {
    settingsLoading.value = false
  }
}

/** 页签同步到 URL query（刷新 / 深链保持） */
watch(activeTab, (v) => {
  if (route.query.tab === v) return
  router.replace({ query: { ...route.query, tab: v } })
})

/** 切换页签前：运行参数有未保存修改时二次确认 */
async function onBeforeLeave(newName: string | number): Promise<boolean> {
  if (newName === activeTab.value) return true
  const dirty = runtimePanelRef.value?.isDirty ?? false
  if (!dirty) return true
  try {
    await ElMessageBox.confirm(
      '「运行参数」存在未保存的修改，建议先保存。仍要切换到其他页签吗？',
      '未保存的修改',
      {
        type: 'warning',
        confirmButtonText: '仍要切换',
        cancelButtonText: '留在本页',
        closeOnClickModal: false,
      },
    )
    return true
  } catch {
    return false
  }
}

onMounted(() => {
  // 深链 / 刷新恢复页签
  const initial = tabFromQuery()
  const raw = route.query.tab
  // query 中存在但非法 → 移除 tab 参数避免污染 URL
  if (typeof raw === 'string' && raw !== initial) {
    const q = { ...route.query }
    delete q.tab
    router.replace({ query: q })
  }
  activeTab.value = initial
  loadSettings()
})
</script>

<style scoped>
.load-error {
  margin-bottom: 14px;
}
.load-error-body {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  width: 100%;
}
.err-text {
  font-size: 13px;
  word-break: break-all;
}
</style>
