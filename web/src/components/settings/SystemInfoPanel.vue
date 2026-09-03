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
import type { ConfigFileView, VersionResp } from '@/api/types'

const version = ref<VersionResp | null>(null)
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
  Promise.all([loadVersion(), loadConfig()]).finally(() => {
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
  color: #303133;
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}
.hint {
  font-size: 12px;
  color: #909399;
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
  color: #303133;
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
