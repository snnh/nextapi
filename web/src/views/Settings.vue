<template>
  <div class="page" v-loading="settingsLoading">
    <div class="page-head">
      <div class="page-head-copy">
        <p class="page-desc">
          集中管理运行参数、代理、更新检查、审计与安全设置。热生效项保存后立即应用，标注「下次启动」的项需重启服务。
        </p>
        <div class="page-head-summary" aria-label="设置项概览">
          <span><b>{{ settings.length }}</b> 项可配置</span>
          <span>热生效 <b>{{ hotCount }}</b></span>
          <span>需重启 <b>{{ restartCount }}</b></span>
          <span>环境锁定 <b>{{ envCount }}</b></span>
        </div>
      </div>
      <div class="page-head-actions">
        <el-button :icon="Refresh" :loading="settingsLoading" @click="refreshSettings">刷新</el-button>
      </div>
    </div>

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
        <el-tab-pane name="runtime">
          <template #label>
            <span class="tab-label">
              <el-icon><Operation /></el-icon>运行参数
              <span v-if="runtimeDirty" class="tab-dirty" :title="`${runtimeDirty} 项未保存`">{{ runtimeDirty }}</span>
            </span>
          </template>
          <RuntimeParamsPanel
            ref="runtimePanelRef"
            :settings="settings"
            @saved="loadSettings"
            @dirty-change="runtimeDirty = $event"
          />
        </el-tab-pane>
        <el-tab-pane name="proxy" lazy>
          <template #label>
            <span class="tab-label"><el-icon><Connection /></el-icon>代理管理</span>
          </template>
          <ProxyPanel :settings="settings" />
        </el-tab-pane>
        <el-tab-pane name="update" lazy>
          <template #label>
            <span class="tab-label"><el-icon><Refresh /></el-icon>更新检查</span>
          </template>
          <UpdateCheckPanel :settings="settings" />
        </el-tab-pane>
        <el-tab-pane name="audit" lazy>
          <template #label>
            <span class="tab-label"><el-icon><Document /></el-icon>审计日志</span>
          </template>
          <AuditPanel />
        </el-tab-pane>
        <el-tab-pane name="security" lazy>
          <template #label>
            <span class="tab-label"><el-icon><Lock /></el-icon>安全</span>
          </template>
          <TotpPanel />
        </el-tab-pane>
        <el-tab-pane name="system" lazy>
          <template #label>
            <span class="tab-label"><el-icon><Monitor /></el-icon>系统</span>
          </template>
          <SystemInfoPanel />
        </el-tab-pane>
      </el-tabs>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { onBeforeRouteLeave, useRoute, useRouter } from 'vue-router'
import { ElMessageBox } from 'element-plus'
import { Connection, Document, Lock, Monitor, Operation, Refresh } from '@element-plus/icons-vue'
import { settingsApi } from '@/api'
import { errMsg } from '@/api/http'
import type { SettingItem } from '@/api/types'
import { STARTUP_ONLY_KEYS } from '@/components/settings/fields'
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
const runtimeDirty = ref(0)
const activeTab = ref('runtime')
const settings = ref<SettingItem[]>([])
const settingsLoading = ref(false)
const loadError = ref('')

const needsRestart = (it: SettingItem) => it.restart_required || STARTUP_ONLY_KEYS.has(it.key)
const hotCount = computed(() => settings.value.filter((it) => !needsRestart(it)).length)
const restartCount = computed(() => settings.value.filter((it) => needsRestart(it)).length)
const envCount = computed(() => settings.value.filter((it) => it.source === 'env').length)

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

/** 手动刷新：有未保存修改时先确认，避免一键丢弃 */
async function refreshSettings() {
  if (runtimePanelRef.value?.isDirty) {
    try {
      await ElMessageBox.confirm(
        '刷新会重新读取服务端配置并丢弃「运行参数」中尚未保存的修改。继续刷新吗？',
        '未保存的修改',
        {
          type: 'warning',
          confirmButtonText: '丢弃并刷新',
          cancelButtonText: '取消',
          closeOnClickModal: false,
        },
      )
    } catch {
      return
    }
  }
  await loadSettings()
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

/** 离开设置页时同样保护未保存修改（SPA 路由跳转） */
onBeforeRouteLeave(async () => {
  if (!runtimePanelRef.value?.isDirty) return true
  try {
    await ElMessageBox.confirm(
      '「运行参数」存在未保存的修改。离开后这些修改会丢失，仍要离开吗？',
      '未保存的修改',
      {
        type: 'warning',
        confirmButtonText: '仍要离开',
        cancelButtonText: '留在本页',
        closeOnClickModal: false,
      },
    )
    return true
  } catch {
    return false
  }
})

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
.page-head-copy {
  min-width: 0;
}
.page-head-summary {
  margin-top: 6px;
}
.tab-label {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.tab-label .el-icon {
  font-size: 14px;
}
.tab-dirty {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 16px;
  height: 16px;
  padding: 0 5px;
  border-radius: var(--nx-radius-round);
  background: var(--nx-warning);
  color: var(--nx-on-accent);
  font-size: 10px;
  font-weight: 600;
  line-height: 1;
}
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
  font-size: var(--nx-font-small);
  word-break: break-all;
}
</style>
