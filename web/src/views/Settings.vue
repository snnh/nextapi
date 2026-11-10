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

    <div class="settings-layout">
      <nav class="settings-nav" aria-label="设置分类">
        <button
          v-for="tab in SETTINGS_TABS"
          :key="tab.name"
          type="button"
          class="nav-item"
          :class="{ 'is-active': activeTab === tab.name }"
          :aria-current="activeTab === tab.name ? 'page' : undefined"
          @click="switchTab(tab.name)"
        >
          <el-icon><component :is="tab.icon" /></el-icon>
          <span>{{ tab.label }}</span>
          <span
            v-if="tab.name === 'runtime' && runtimeDirty"
            class="nav-dirty"
            :title="`${runtimeDirty} 项未保存`"
          >{{ runtimeDirty }}</span>
        </button>
      </nav>

      <el-select
        class="settings-tab-select"
        :model-value="activeTab"
        aria-label="设置分类"
        @change="onMobileTabChange"
      >
        <el-option v-for="tab in SETTINGS_TABS" :key="tab.name" :label="tab.label" :value="tab.name" />
      </el-select>

      <div class="settings-content page-card">
        <!-- 初始加载失败：错误条 + 重试 -->
        <el-alert v-if="loadError" class="load-error" type="error" show-icon :closable="false">
          <div class="load-error-body">
            <span class="err-text">运行参数加载失败：{{ loadError }}</span>
            <el-button size="small" type="primary" plain :loading="settingsLoading" @click="loadSettings">
              重试
            </el-button>
          </div>
        </el-alert>

        <div v-if="visited.has('runtime')" v-show="activeTab === 'runtime'">
          <RuntimeParamsPanel
            ref="runtimePanelRef"
            :settings="settings"
            @saved="loadSettings"
            @dirty-change="runtimeDirty = $event"
          />
        </div>
        <div v-if="visited.has('proxy')" v-show="activeTab === 'proxy'">
          <ProxyPanel :settings="settings" />
        </div>
        <div v-if="visited.has('update')" v-show="activeTab === 'update'">
          <UpdateCheckPanel :settings="settings" />
        </div>
        <div v-if="visited.has('audit')" v-show="activeTab === 'audit'">
          <AuditPanel />
        </div>
        <div v-if="visited.has('security')" v-show="activeTab === 'security'">
          <TotpPanel />
        </div>
        <div v-if="visited.has('system')" v-show="activeTab === 'system'">
          <SystemInfoPanel />
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
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

/** 设置分类名，用于左侧导航与 ?tab= 深链恢复 */
const TAB_NAMES = ['runtime', 'proxy', 'update', 'audit', 'security', 'system'] as const
type TabName = (typeof TAB_NAMES)[number]

const route = useRoute()
const router = useRouter()

const SETTINGS_TABS = [
  { name: 'runtime', label: '运行参数', icon: Operation },
  { name: 'proxy', label: '代理管理', icon: Connection },
  { name: 'update', label: '更新检查', icon: Refresh },
  { name: 'audit', label: '审计日志', icon: Document },
  { name: 'security', label: '安全', icon: Lock },
  { name: 'system', label: '系统', icon: Monitor },
] as const

const runtimePanelRef = ref<InstanceType<typeof RuntimeParamsPanel> | null>(null)
const runtimeDirty = ref(0)
const activeTab = ref<TabName>('runtime')
/** 已访问过的分类才挂载，避免一次性渲染全部设置面板 */
const visited = reactive(new Set<TabName>(['runtime']))
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

/** 切换设置分类；运行参数有未保存改动时先确认 */
async function switchTab(name: TabName) {
  if (name === activeTab.value) return
  if (!(await onBeforeLeave(name))) return
  activeTab.value = name
  visited.add(name)
}

/** 移动端分类下拉（取消切换时由 model-value 自动回到当前分类） */
function onMobileTabChange(value: string | number | boolean | undefined) {
  if (typeof value !== 'string') return
  if (!(TAB_NAMES as readonly string[]).includes(value)) return
  void switchTab(value as TabName)
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
  visited.add(initial)
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
.settings-layout {
  display: grid;
  grid-template-columns: 190px minmax(0, 1fr);
  gap: 16px;
  align-items: start;
}
.settings-nav {
  position: sticky;
  top: 16px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 8px;
  border: 1px solid var(--nx-border);
  border-radius: var(--nx-radius-lg);
  background: var(--nx-surface);
}
.nav-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 9px 10px;
  border: 0;
  border-radius: var(--nx-radius);
  background: transparent;
  color: var(--nx-text-2);
  font-size: var(--nx-font-small);
  text-align: left;
  cursor: pointer;
  transition: background-color var(--nx-dur-fast) var(--nx-ease),
    color var(--nx-dur-fast) var(--nx-ease);
}
.nav-item:hover {
  background: var(--nx-hover);
  color: var(--nx-text);
}
.nav-item.is-active {
  background: var(--nx-accent-soft);
  color: var(--nx-accent);
  font-weight: 600;
}
.nav-item:focus-visible {
  outline: 2px solid var(--nx-focus-ring);
  outline-offset: -2px;
}
.nav-dirty {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 16px;
  height: 16px;
  margin-left: auto;
  padding: 0 5px;
  border-radius: var(--nx-radius-round);
  background: var(--nx-warning);
  color: var(--nx-on-accent);
  font-size: 10px;
  font-weight: 600;
  line-height: 1;
}
.settings-content {
  min-width: 0;
  margin-bottom: 0;
}
.settings-tab-select {
  display: none;
  width: 100%;
}
@media (max-width: 1000px) {
  .settings-layout {
    grid-template-columns: 1fr;
  }
  .settings-nav {
    display: none;
  }
  .settings-tab-select {
    display: block;
    margin-bottom: 12px;
  }
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
