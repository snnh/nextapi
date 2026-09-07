<template>
  <div class="audit-panel">
    <div class="toolbar">
      <el-input v-model="action" placeholder="按 action 过滤" clearable class="filter-input" @keyup.enter="search" @clear="search" />
      <el-input
        v-model="objectType"
        placeholder="按 object_type 过滤"
        clearable
        class="filter-input"
        @keyup.enter="search"
        @clear="search"
      />
      <el-button type="primary" :icon="Search" @click="search">查询</el-button>
      <el-button :icon="Refresh" @click="reset">重置</el-button>
      <div class="spacer" />
      <span class="hint">管理端操作审计（登录 / 增删改 / 清理等）。</span>
    </div>

    <el-card shadow="never">
      <el-table :data="rows" v-loading="loading">
        <template #empty>
          <el-empty description="暂无审计日志" :image-size="60" />
        </template>
        <el-table-column label="时间" width="180">
          <template #default="{ row }">{{ fmtTime(row.created_at) }}</template>
        </el-table-column>
        <el-table-column label="管理员" min-width="120">
          <template #default="{ row }">
            <span v-if="row.admin_name">{{ row.admin_name }}</span>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>
        <el-table-column label="动作" min-width="130">
          <template #default="{ row }">
            <el-tag size="small" type="primary" effect="plain" class="mono">{{ row.action }}</el-tag>
          </template>
        </el-table-column>
        <el-table-column prop="object_type" label="对象类型" min-width="120">
          <template #default="{ row }">
            <span v-if="row.object_type">{{ row.object_type }}</span>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>
        <el-table-column prop="object_id" label="对象 ID" min-width="200">
          <template #default="{ row }">
            <CopyText v-if="row.object_id" :text="row.object_id" :truncate="16" />
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>
        <el-table-column prop="ip" label="IP" min-width="130">
          <template #default="{ row }">
            <CopyText v-if="row.ip" :text="row.ip" />
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>
        <el-table-column label="摘要" width="120" align="center">
          <template #default="{ row }">
            <el-button v-if="hasSummary(row)" size="small" link type="primary" @click="showSummary(row)">查看</el-button>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>
      </el-table>

      <div class="pager">
        <el-pagination
          v-model:current-page="page"
          v-model:page-size="pageSize"
          :total="total"
          :page-sizes="[20, 50, 100]"
          layout="total, sizes, prev, pager, next, jumper"
          background
          @current-change="load"
          @size-change="onSizeChange"
        />
      </div>
    </el-card>

    <el-dialog v-model="summaryVisible" title="摘要详情" class="dlg">
      <pre class="summary-pre">{{ summaryText }}</pre>
      <template #footer>
        <el-button @click="summaryVisible = false">关闭</el-button>
        <el-button type="primary" :icon="CopyDocument" @click="copySummary">复制</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { CopyDocument, Refresh, Search } from '@element-plus/icons-vue'
import { auditApi } from '@/api'
import { errMsg } from '@/api/http'
import { fmtTime } from '@/utils/format'
import { copyText } from '@/utils/clipboard'
import CopyText from '@/components/common/CopyText.vue'
import type { AuditRow } from '@/api/types'

const rows = ref<AuditRow[]>([])
const total = ref(0)
const page = ref(1)
const pageSize = ref(50)
const action = ref('')
const objectType = ref('')
const loading = ref(false)

const summaryVisible = ref(false)
const summaryJson = ref<Record<string, unknown> | null>(null)
const summaryText = ref('')

function hasSummary(row: AuditRow): boolean {
  return !!row.summary && Object.keys(row.summary).length > 0
}

function showSummary(row: AuditRow) {
  summaryJson.value = row.summary
  summaryText.value = JSON.stringify(row.summary, null, 2)
  summaryVisible.value = true
}

/** 复制当前摘要 JSON（同展示格式，缩进 2 空格） */
function copySummary() {
  if (!summaryJson.value) return
  copyText(JSON.stringify(summaryJson.value, null, 2))
}

// 请求序号：防翻页/筛选并发时旧响应覆盖（发布审阅前端 M2）
let loadSeq = 0

async function load() {
  const seq = ++loadSeq
  loading.value = true
  try {
    const r = await auditApi.list({
      page: page.value,
      page_size: pageSize.value,
      action: action.value.trim() || undefined,
      object_type: objectType.value.trim() || undefined,
    })
    if (seq !== loadSeq) return
    rows.value = r.items
    total.value = r.total
  } catch (e) {
    if (seq !== loadSeq) return
    ElMessage.error(errMsg(e))
  } finally {
    if (seq === loadSeq) loading.value = false
  }
}

function search() {
  page.value = 1
  load()
}

function reset() {
  action.value = ''
  objectType.value = ''
  page.value = 1
  load()
}

function onSizeChange() {
  page.value = 1
  load()
}

onMounted(load)
</script>

<style scoped>
.toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 14px;
}
.filter-input {
  width: 200px;
}
.toolbar .spacer {
  flex: 1;
}
.hint {
  font-size: 12px;
  color: #6b7280;
}
.muted {
  color: #c0c4cc;
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}
.pager {
  display: flex;
  justify-content: flex-end;
  margin-top: 14px;
}
.summary-pre {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-word;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 13px;
  line-height: 1.6;
  background: #f5f7fa;
  padding: 12px;
  border-radius: 6px;
  max-height: 60vh;
  overflow: auto;
}
</style>
