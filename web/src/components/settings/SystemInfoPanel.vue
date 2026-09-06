<template>
  <div class="system-panel" v-loading="pageLoading">
    <el-card shadow="never" class="card">
      <template #header>
        <div class="card-head"><span>网关信息</span></div>
      </template>
      <el-descriptions :column="2" border>
        <el-descriptions-item label="版本">
          <span class="mono">{{ version?.version ?? '-' }}</span>
        </el-descriptions-item>
        <el-descriptions-item label="启动时间">{{ fmtTime(version?.started_at) }}</el-descriptions-item>
      </el-descriptions>
    </el-card>

    <el-card shadow="never" class="card">
      <template #header>
        <div class="card-head">
          <span>运行状态</span>
          <el-button link type="primary" :icon="Refresh" @click="loadStatus">刷新</el-button>
        </div>
      </template>
      <el-descriptions v-if="status" :column="2" border>
        <el-descriptions-item label="运行时长">{{ fmtUptime(status.uptime_secs) }}</el-descriptions-item>
        <el-descriptions-item label="数据库">
          <el-tag :type="status.database.ok ? 'success' : 'danger'" size="small">
            {{ status.database.ok ? '正常' : '异常' }}
          </el-tag>
          <span class="hint" style="margin-left: 6px">{{ status.database.latency_ms }}ms</span>
        </el-descriptions-item>
        <el-descriptions-item label="配置来源">{{ settingsSourceText }}</el-descriptions-item>
        <el-descriptions-item label="日志队列">
          <template v-if="status.logging.queue_capacity != null">
            {{ status.logging.queue_used }} / {{ status.logging.queue_capacity }}
            <span class="hint">（溢出 {{ status.logging.overflow_total }}）</span>
          </template>
          <span v-else>同步直写（无队列）</span>
        </el-descriptions-item>
        <el-descriptions-item label="WAL 兜底">
          {{ status.logging.wal_files }} 个文件 / {{ fmtBytes(status.logging.wal_bytes) }}
        </el-descriptions-item>
        <el-descriptions-item label="媒体轮询">
          {{ status.media_poller.interval_secs > 0 ? `每 ${status.media_poller.interval_secs}s` : '已停用' }}
          <span class="hint">（待回联任务 {{ status.media_poller.pending_tasks }}）</span>
        </el-descriptions-item>
        <el-descriptions-item label="熔断上游" :span="2">
          <template v-if="status.breakers.length">
            <el-tag v-for="b in status.breakers" :key="b.name" size="small" type="danger" effect="plain" style="margin-right: 6px">
              {{ b.name }}（连续失败 {{ b.consecutive_failures }}<template v-if="b.disabled_by">，{{ b.disabled_by === 'manual' ? '手动禁用' : '自动熔断' }}</template>）
            </el-tag>
          </template>
          <span v-else class="hint">无</span>
        </el-descriptions-item>
      </el-descriptions>
      <template v-if="status?.recent_errors.length">
        <el-divider content-position="left">最近错误</el-divider>
        <el-table :data="status.recent_errors" size="small">
          <el-table-column label="时间" width="170">
            <template #default="{ row }">{{ fmtTime(row.ts) }}</template>
          </el-table-column>
          <el-table-column prop="model" label="模型" width="160" show-overflow-tooltip />
          <el-table-column prop="status" label="状态码" width="80" align="center" />
          <el-table-column prop="error" label="错误摘要" min-width="240" show-overflow-tooltip />
        </el-table>
      </template>
    </el-card>

    <el-card shadow="never" class="card">
      <template #header>
        <div class="card-head"><span>YAML 配置</span></div>
      </template>
      <div class="hint" style="margin-bottom: 12px">
        <span class="mono">***</span> = 已设置；导出文件中的 <span class="mono">***</span> 回传即保持原值。
      </div>
      <div class="config-actions">
        <el-button type="primary" :loading="exporting" :icon="Download" @click="exportYaml">导出 YAML</el-button>
        <el-button :icon="Upload" @click="openImport">导入 YAML/JSON</el-button>
        <el-button :loading="reloading" :icon="Refresh" @click="reload">热加载配置</el-button>
      </div>
      <el-collapse v-model="configOpen" class="config-collapse">
        <el-collapse-item name="config">
          <template #title><span class="notes-title">当前配置（掩码视图）</span></template>
          <pre v-if="configText" class="config-pre">{{ configText }}</pre>
          <div v-else class="hint">暂无配置。</div>
        </el-collapse-item>
      </el-collapse>
    </el-card>

    <el-card shadow="never" class="card">
      <template #header>
        <div class="card-head"><span>修改密码</span></div>
      </template>
      <el-alert type="info" :closable="false" show-icon title="修改密码请使用右上角头像菜单「修改密码」。" />
    </el-card>

    <el-dialog v-model="importVisible" title="导入 YAML / JSON" width="680px" :close-on-click-modal="false">
      <el-input
        v-model="importText"
        type="textarea"
        :rows="12"
        placeholder="粘贴 YAML 或 JSON 配置内容"
        class="import-textarea"
      />
      <el-upload
        :auto-upload="false"
        :show-file-list="false"
        accept=".yaml,.yml,.json"
        :on-change="onFileChange"
        class="import-upload"
      >
        <el-button :icon="Upload">选择文件（.yaml/.yml/.json）</el-button>
      </el-upload>
      <div class="hint" style="margin-top: 8px">粘贴内容或选择本地文件；掩码字段 <span class="mono">***</span> 回传即保持原值。</div>
      <template #footer>
        <el-button @click="importVisible = false">取消</el-button>
        <el-button type="primary" :loading="savingConfig" @click="doImport">写入并热加载</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ElMessage, type UploadFile } from 'element-plus'
import { Download, Refresh, Upload } from '@element-plus/icons-vue'
import yaml from 'js-yaml'
import { configApi, systemApi } from '@/api'
import { errMsg } from '@/api/http'
import { fmtTime } from '@/utils/format'
import { downloadText } from '@/utils/download'
import type { ConfigFileView, SystemStatusResp, VersionResp } from '@/api/types'

const version = ref<VersionResp | null>(null)
const status = ref<SystemStatusResp | null>(null)
const config = ref<ConfigFileView | null>(null)
const pageLoading = ref(false)
const exporting = ref(false)
const reloading = ref(false)
const savingConfig = ref(false)
const configOpen = ref<string[]>(['config'])
const importVisible = ref(false)
const importText = ref('')

const configText = computed(() =>
  config.value ? JSON.stringify(config.value, null, 2) : '',
)

const settingsSourceText = computed(() => {
  const s = status.value?.settings_sources
  if (!s || !Object.keys(s).length) return '-'
  const labels: Record<string, string> = { ui: '界面', file: 'YAML', env: '环境变量' }
  return Object.entries(s)
    .map(([k, n]) => `${labels[k] ?? k} ${n}`)
    .join(' · ')
})

function fmtUptime(secs: number): string {
  if (secs < 3600) return `${Math.floor(secs / 60)} 分钟`
  if (secs < 86400) return `${Math.floor(secs / 3600)} 小时 ${Math.floor((secs % 3600) / 60)} 分`
  return `${Math.floor(secs / 86400)} 天 ${Math.floor((secs % 86400) / 3600)} 小时`
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1024 / 1024).toFixed(1)} MB`
}

async function loadStatus() {
  try {
    status.value = await systemApi.status()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

async function loadVersion() {
  try {
    version.value = await systemApi.version()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

async function loadConfig() {
  try {
    config.value = await configApi.get()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

function exportYaml() {
  if (!config.value) {
    ElMessage.warning('暂无配置')
    return
  }
  exporting.value = true
  try {
    const text = yaml.dump(config.value, { lineWidth: 120 })
    downloadText(text, 'config.yaml', 'application/x-yaml;charset=utf-8')
    ElMessage.success('已导出 config.yaml')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    exporting.value = false
  }
}

async function reload() {
  reloading.value = true
  try {
    await configApi.reload()
    ElMessage.success('已仅重载 hot 运行参数（不覆盖 DB 业务实体）')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    reloading.value = false
  }
}

function openImport() {
  importText.value = ''
  importVisible.value = true
}

function onFileChange(file: UploadFile) {
  const raw = file.raw
  if (!raw) return
  const reader = new FileReader()
  reader.onload = () => {
    importText.value = String(reader.result || '')
  }
  reader.readAsText(raw)
}

async function doImport() {
  if (!importText.value.trim()) {
    ElMessage.warning('请粘贴或选择 YAML/JSON 内容')
    return
  }
  let parsed: unknown
  try {
    parsed = yaml.load(importText.value)
  } catch (e) {
    ElMessage.error(`格式错误：${e instanceof Error ? e.message : String(e)}`)
    return
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    ElMessage.error('配置必须为对象（键值）形式，不能是数组或标量')
    return
  }
  savingConfig.value = true
  try {
    await configApi.put(parsed as ConfigFileView)
    ElMessage.success('已写入并热加载（敏感掩码字段保持原值）')
    importVisible.value = false
    importText.value = ''
    loadConfig()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    savingConfig.value = false
  }
}

onMounted(() => {
  pageLoading.value = true
  Promise.all([loadVersion(), loadConfig(), loadStatus()]).finally(() => {
    pageLoading.value = false
  })
})
</script>

<style scoped>
.card {
  margin-bottom: 16px;
}
.card-head {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  font-weight: 600;
  color: #1f2329;
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}
.hint {
  font-size: 12px;
  color: #6b7280;
}
.config-actions {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.config-collapse {
  margin-top: 12px;
}
.notes-title {
  font-size: 14px;
  font-weight: 600;
  color: #1f2329;
}
.config-pre {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-word;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 13px;
  line-height: 1.6;
  background: #f5f7fa;
  padding: 12px;
  border-radius: 6px;
  max-height: 420px;
  overflow: auto;
}
.import-textarea {
  width: 100%;
}
.import-upload {
  margin-top: 10px;
}
</style>
