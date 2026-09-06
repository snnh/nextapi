<template>
  <div class="import-panel">
    <el-row :gutter="16">
      <el-col :xs="24" :md="12">
        <el-card shadow="never">
          <template #header>
            <div class="card-head"><span>本地文件导入</span></div>
          </template>
          <div class="hint" style="margin-bottom: 12px">
            支持 nextapi 导出的 XML / JSON 价格文件。选择文件后可用「预览导入（dry-run）」先行校验，确认无误再正式导入。
          </div>
          <el-upload
            ref="uploadRef"
            drag
            :auto-upload="false"
            :limit="1"
            accept=".xml,.json"
            :on-change="onFileChange"
            :on-remove="onFileRemove"
            :on-exceed="onFileExceed"
          >
            <el-icon class="el-icon--upload"><UploadFilled /></el-icon>
            <div class="el-upload__text">拖拽文件到此处，或<em>点击选择</em></div>
            <template #tip>
              <div class="el-upload__tip">单个文件，支持 XML / JSON</div>
            </template>
          </el-upload>

          <div class="import-action">
            <span class="import-label">预览模式（dry-run）</span>
            <el-switch v-model="fileDryRun" />
            <el-button
              type="primary"
              :disabled="!selectedFile"
              :loading="filePreviewLoading"
              @click="previewFile"
            >
              预览导入（dry_run）
            </el-button>
          </div>
        </el-card>
      </el-col>

      <el-col :xs="24" :md="12">
        <el-card shadow="never">
          <template #header>
            <div class="card-head"><span>URL 导入</span></div>
          </template>
          <div class="hint" style="margin-bottom: 12px">
            从网络地址拉取价格文件（仅 https 协议；已做 SSRF 防护；单文件 ≤ 1MB）。可先 dry-run 预览。
          </div>
          <el-input v-model="url" placeholder="https://example.com/prices.xml" clearable style="margin-bottom: 12px" />
          <div class="import-action">
            <span class="import-label">预览模式（dry-run）</span>
            <el-switch v-model="urlDryRun" />
            <el-button type="primary" :disabled="!url.trim()" :loading="urlImporting" @click="importFromUrl">
              导入
            </el-button>
          </div>
        </el-card>
      </el-col>
    </el-row>

    <ImportReportDialog
      v-model="reportVisible"
      :report="report"
      :confirm-loading="confirmLoading"
      @confirm="confirmImport"
    />
  </div>
</template>

<script setup lang="ts">
import { reactive, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { UploadFilled } from '@element-plus/icons-vue'
import { pricingApi } from '@/api'
import { errMsg } from '@/api/http'
import type { ImportReport } from '@/api/types'
import ImportReportDialog from './ImportReportDialog.vue'

const fileDryRun = ref(true)
const urlDryRun = ref(true)
const url = ref('')
const selectedFile = ref<File | null>(null)
const filePreviewLoading = ref(false)
const urlImporting = ref(false)
const reportVisible = ref(false)
const report = ref<ImportReport | null>(null)
const confirmLoading = ref(false)
const uploadRef = ref()

const lastSource = reactive<{ kind: 'file' | 'url'; file: File | null; url: string }>({
  kind: 'file',
  file: null,
  url: '',
})

function onFileChange(file: { raw?: File }) {
  selectedFile.value = file.raw || null
}

function onFileRemove() {
  selectedFile.value = null
}

function onFileExceed() {
  // 已选文件时重新选择：清空后选择新文件
  uploadRef.value?.clearFiles()
  selectedFile.value = null
}

async function previewFile() {
  const f = selectedFile.value
  if (!f) {
    ElMessage.warning('请先选择文件')
    return
  }
  filePreviewLoading.value = true
  try {
    const r = await pricingApi.importFile(f, true)
    lastSource.kind = 'file'
    lastSource.file = f
    lastSource.url = ''
    report.value = r
    reportVisible.value = true
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    filePreviewLoading.value = false
  }
}

async function importFromUrl() {
  const u = url.value.trim()
  if (!u) {
    ElMessage.warning('请输入 URL')
    return
  }
  if (!u.startsWith('https://')) {
    ElMessage.warning('仅支持 https 协议')
    return
  }
  urlImporting.value = true
  try {
    const r = await pricingApi.importUrl(u, urlDryRun.value)
    lastSource.kind = 'url'
    lastSource.file = null
    lastSource.url = u
    report.value = r
    reportVisible.value = true
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    urlImporting.value = false
  }
}

async function confirmImport() {
  if (!report.value) return
  confirmLoading.value = true
  try {
    let r: ImportReport
    if (lastSource.kind === 'file' && lastSource.file) {
      r = await pricingApi.importFile(lastSource.file, false)
    } else if (lastSource.kind === 'url' && lastSource.url) {
      r = await pricingApi.importUrl(lastSource.url, false)
    } else {
      ElMessage.warning('导入源已失效')
      return
    }
    report.value = r
    if (!r.dry_run) {
      ElMessage.success(`已导入 ${r.succeeded} 条价格`)
    }
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    confirmLoading.value = false
  }
}
</script>

<style scoped>
.card-head {
  font-size: 14px;
  font-weight: 600;
  color: #1f2329;
}
.import-action {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 14px;
  flex-wrap: wrap;
}
.import-label {
  font-size: 13px;
  color: #4b5563;
}
</style>
