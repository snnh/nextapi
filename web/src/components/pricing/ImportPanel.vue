<template>
  <div class="import-panel">
    <el-row :gutter="16">
      <el-col :xs="24" :md="12">
        <el-card shadow="never">
          <template #header>
            <div class="card-head"><span>本地文件导入</span></div>
          </template>
          <div class="hint" style="margin-bottom: 12px">
            支持 nextapi 导出的 XML / JSON 价格文件。「预览」只校验并展示写入条数，不落库；确认无误后再点「导入」正式写入。
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
              <div class="el-upload__tip">单个文件，支持 XML / JSON，≤ 1MB</div>
            </template>
          </el-upload>

          <div class="import-action">
            <el-button :disabled="!selectedFile" :loading="filePreviewLoading" @click="previewFile">
              预览
            </el-button>
            <el-button
              type="primary"
              :disabled="!canImportFile"
              :loading="fileImporting"
              @click="importFileNow"
            >
              导入
            </el-button>
            <span v-if="filePreview" class="hint preview-summary">
              已预览：成功 {{ filePreview.succeeded }} · 跳过 {{ filePreview.skipped }} · 失败
              {{ filePreview.failed.length }}
            </span>
          </div>
        </el-card>
      </el-col>

      <el-col :xs="24" :md="12">
        <el-card shadow="never">
          <template #header>
            <div class="card-head"><span>URL 导入</span></div>
          </template>
          <div class="hint" style="margin-bottom: 12px">
            从网络地址拉取价格文件（仅 https 协议；已做 SSRF 防护；单文件 ≤ 1MB）。先「预览」确认写入条数，再「导入」正式写入。
          </div>
          <el-input v-model="url" placeholder="https://example.com/prices.xml" clearable style="margin-bottom: 12px" />
          <div class="import-action">
            <el-button :disabled="!url.trim()" :loading="urlPreviewLoading" @click="previewUrl">
              预览
            </el-button>
            <el-button
              type="primary"
              :disabled="!canImportUrl"
              :loading="urlImporting"
              @click="importUrlNow"
            >
              导入
            </el-button>
            <span v-if="urlPreview" class="hint preview-summary">
              已预览：成功 {{ urlPreview.succeeded }} · 跳过 {{ urlPreview.skipped }} · 失败
              {{ urlPreview.failed.length }}
            </span>
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
import { computed, reactive, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { UploadFilled } from '@element-plus/icons-vue'
import { pricingApi } from '@/api'
import { errMsg } from '@/api/http'
import { confirmDanger } from '@/utils/confirm'
import type { ImportReport } from '@/api/types'
import ImportReportDialog from './ImportReportDialog.vue'

const url = ref('')
const selectedFile = ref<File | null>(null)
const filePreviewLoading = ref(false)
const fileImporting = ref(false)
const urlPreviewLoading = ref(false)
const urlImporting = ref(false)
/** 最近一次预览报告（仅当来源未变化时可据此导入，避免预览与写入内容不一致） */
const filePreview = ref<ImportReport | null>(null)
const urlPreview = ref<ImportReport | null>(null)
let filePreviewKey = ''
let urlPreviewKey = ''
const reportVisible = ref(false)
const report = ref<ImportReport | null>(null)
const confirmLoading = ref(false)
const uploadRef = ref()

const fileKey = (f: File) => `${f.name}:${f.size}:${f.lastModified}`
const canImportFile = computed(
  () => !!selectedFile.value && !!filePreview.value && filePreviewKey === fileKey(selectedFile.value),
)
const canImportUrl = computed(
  () => !!urlPreview.value && urlPreviewKey === url.value.trim() && !!url.value.trim(),
)

/** 本地/网络价格文件体积上限（后端同样限制单文件 ≤ 1MB） */
const MAX_IMPORT_BYTES = 1024 * 1024
const IMPORT_EXTS = ['xml', 'json']

watch(url, () => {
  urlPreview.value = null
  urlPreviewKey = ''
})

const lastSource = reactive<{ kind: 'file' | 'url'; file: File | null; url: string }>({
  kind: 'file',
  file: null,
  url: '',
})

function extOf(name: string): string {
  const i = name.lastIndexOf('.')
  return i >= 0 ? name.slice(i + 1).toLowerCase() : ''
}

/** 本地文件预检（大小 + 扩展名），通过返回 true；不通过即时提示并清空选择 */
function checkLocalFile(f: File): boolean {
  const ext = extOf(f.name)
  if (!IMPORT_EXTS.includes(ext)) {
    ElMessage.error(`文件「${f.name}」不是价格文件：仅支持 ${IMPORT_EXTS.join(' / ').toUpperCase()} 扩展名（拖拽也会校验）`)
    return false
  }
  if (f.size > MAX_IMPORT_BYTES) {
    ElMessage.error(`文件「${f.name}」大小 ${(f.size / 1024 / 1024).toFixed(2)}MB，超过 1MB 上限`)
    return false
  }
  return true
}

function onFileChange(file: { raw?: File }) {
  const f = file.raw || null
  if (!f) {
    selectedFile.value = null
    return
  }
  if (!checkLocalFile(f)) {
    uploadRef.value?.clearFiles()
    selectedFile.value = null
    filePreview.value = null
    filePreviewKey = ''
    return
  }
  selectedFile.value = f
  filePreview.value = null
  filePreviewKey = ''
}

function onFileRemove() {
  selectedFile.value = null
  filePreview.value = null
  filePreviewKey = ''
}

/** 已选文件后再选/拖入新文件：替换原文件（先清空列表，避免触发 limit 追加失败） */
function onFileExceed(files: File[]) {
  const f = files[0] ?? null
  if (!f) return
  uploadRef.value?.clearFiles()
  if (!checkLocalFile(f)) {
    selectedFile.value = null
    return
  }
  selectedFile.value = f
  filePreview.value = null
  filePreviewKey = ''
  ElMessage.info('已替换原选择文件')
}

/** 正式写库前确认：写入条数取自已完成的预览；明确 upsert 覆盖语义 */
function confirmRealImport(report: ImportReport, source: 'file' | 'url'): Promise<boolean> {
  const remoteNote =
    source === 'url'
      ? ' URL 内容会在导入时重新拉取；若远端文件已变化，将以新内容为准。'
      : ''
  return confirmDanger({
    title: '正式导入价格',
    message:
      `将写入 ${report.succeeded} 条价格；按（上游 + 模型 + 计价单位 + 币种 + 维度）幂等键 upsert，` +
      `同键已有规则会被覆盖，其他条目不受影响。${remoteNote}`,
    confirmText: '确认导入',
  })
}

async function runPreview(
  source: 'file' | 'url',
  dryRun: boolean,
  reportBox: typeof filePreview,
): Promise<ImportReport | null> {
  const f = selectedFile.value
  const u = url.value.trim()
  if (source === 'file' && !f) {
    ElMessage.warning('请先选择文件')
    return null
  }
  if (source === 'url' && !u) {
    ElMessage.warning('请输入 URL')
    return null
  }
  if (source === 'url' && !u.startsWith('https://')) {
    ElMessage.warning('仅支持 https 协议')
    return null
  }

  const loading = source === 'file' ? filePreviewLoading : urlPreviewLoading
  loading.value = true
  try {
    const r =
      source === 'file' && f
        ? await pricingApi.importFile(f, dryRun)
        : await pricingApi.importUrl(u, dryRun)
    lastSource.kind = source
    lastSource.file = source === 'file' ? f : null
    lastSource.url = source === 'url' ? u : ''
    report.value = r
    reportVisible.value = true
    if (dryRun) {
      reportBox.value = r
      if (source === 'file' && f) filePreviewKey = fileKey(f)
      else urlPreviewKey = u
    } else {
      reportBox.value = null
      if (source === 'file' && f) filePreviewKey = ''
      else urlPreviewKey = ''
    }
    return r
  } catch (e) {
    ElMessage.error(errMsg(e))
    return null
  } finally {
    loading.value = false
  }
}

function previewFile() {
  return runPreview('file', true, filePreview)
}

function previewUrl() {
  return runPreview('url', true, urlPreview)
}

async function importFileNow() {
  if (!canImportFile.value || !filePreview.value || fileImporting.value) {
    ElMessage.warning('请先预览当前文件，确认写入条数后再导入')
    return
  }
  const ok = await confirmRealImport(filePreview.value, 'file')
  if (!ok) return
  fileImporting.value = true
  try {
    await runPreview('file', false, filePreview)
  } finally {
    fileImporting.value = false
  }
}

async function importUrlNow() {
  if (!canImportUrl.value || !urlPreview.value || urlImporting.value) {
    ElMessage.warning('请先预览当前 URL，确认写入条数后再导入')
    return
  }
  const ok = await confirmRealImport(urlPreview.value, 'url')
  if (!ok) return
  urlImporting.value = true
  try {
    await runPreview('url', false, urlPreview)
  } finally {
    urlImporting.value = false
  }
}

async function confirmImport() {
  if (!report.value?.dry_run) return
  if (lastSource.kind === 'file' && !canImportFile.value) {
    ElMessage.warning('文件已变化，请重新预览')
    return
  }
  if (lastSource.kind === 'url' && !canImportUrl.value) {
    ElMessage.warning('URL 已变化，请重新预览')
    return
  }
  const ok = await confirmRealImport(report.value, lastSource.kind)
  if (!ok) return
  confirmLoading.value = true
  try {
    const r = await runPreview(lastSource.kind, false, lastSource.kind === 'file' ? filePreview : urlPreview)
    if (r && !r.dry_run) ElMessage.success(`已导入 ${r.succeeded} 条价格`)
  } finally {
    confirmLoading.value = false
  }
}
</script>

<style scoped>
.card-head {
  font-size: var(--nx-font-body);
  font-weight: 600;
  color: var(--el-text-color-primary);
}
.import-action {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-top: 14px;
  flex-wrap: wrap;
}
.preview-summary {
  flex: 1 1 100%;
  margin-top: 2px;
}
</style>
