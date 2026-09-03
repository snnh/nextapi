<template>
  <div class="page">
    <!-- 顶部工具栏：右侧 CSV 导出 & 分区清理 -->
    <div class="toolbar">
      <div class="hint">时间留空表示服务端默认「当天」；筛选条件与 CSV 导出共用。</div>
      <div class="spacer" />
      <el-tooltip
        content="按当前筛选导出；上限 10 万行，超出将截断并追加注释行"
        placement="top"
      >
        <el-button type="primary" plain :icon="Download" :loading="exporting" @click="exportCsv">
          CSV 导出
        </el-button>
      </el-tooltip>
      <el-button type="warning" plain :icon="Delete" @click="openCleanup">分区清理</el-button>
    </div>

    <!-- 筛选工具栏 -->
    <el-card shadow="never" class="page-card">
      <div class="filter-grid">
        <div class="f-item">
          <span class="f-label">时间范围</span>
          <el-date-picker
            v-model="range"
            type="datetimerange"
            :shortcuts="timeShortcuts"
            range-separator="至"
            start-placeholder="开始时间"
            end-placeholder="结束时间"
            clearable
            style="width: 360px"
          />
        </div>
        <div class="f-item">
          <span class="f-label">Key</span>
          <el-select v-model="keyId" filterable clearable placeholder="全部 Key" style="width: 200px">
            <el-option v-for="k in keys" :key="k.id" :label="keyLabel(k)" :value="k.id" />
          </el-select>
        </div>
        <div class="f-item">
          <span class="f-label">上游</span>
          <el-select
            v-model="upstreamId"
            filterable
            clearable
            placeholder="全部上游"
            style="width: 180px"
          >
            <el-option v-for="u in upstreams" :key="u.id" :label="u.name" :value="u.id" />
          </el-select>
        </div>
        <div class="f-item">
          <span class="f-label">模型</span>
          <el-input v-model="model" placeholder="模型" clearable style="width: 160px" />
        </div>
        <div class="f-item">
          <span class="f-label">request_id</span>
          <el-input v-model="requestId" placeholder="request_id" clearable style="width: 200px" />
        </div>
        <div class="f-item">
          <span class="f-label">状态码</span>
          <el-input v-model="statusStr" type="number" placeholder="如 429" clearable style="width: 110px" />
        </div>
        <div class="f-item">
          <span class="f-label">流式</span>
          <el-select v-model="streamOpt" style="width: 90px">
            <el-option label="全部" value="" />
            <el-option label="是" value="true" />
            <el-option label="否" value="false" />
          </el-select>
        </div>
        <div class="f-item">
          <span class="f-label">降级</span>
          <el-select v-model="degradedOpt" style="width: 90px">
            <el-option label="全部" value="" />
            <el-option label="是" value="true" />
            <el-option label="否" value="false" />
          </el-select>
        </div>
      </div>
      <div class="filter-actions">
        <el-button type="primary" :icon="Search" @click="onQuery">查询</el-button>
        <el-button :icon="Refresh" @click="onReset">重置</el-button>
      </div>
    </el-card>

    <!-- 日志表格 -->
    <el-card shadow="never" class="page-card">
      <div class="table-head">
        <span>请求日志</span>
      </div>
      <el-table :data="rows" v-loading="loading" stripe>
        <template #empty>
          <el-empty description="无匹配日志" :image-size="90" />
        </template>
        <el-table-column prop="ts" label="时间" width="170">
          <template #default="{ row }">{{ fmtTime(row.ts) }}</template>
        </el-table-column>
        <el-table-column prop="model" label="模型" min-width="150" show-overflow-tooltip />
        <el-table-column label="Key" min-width="170" show-overflow-tooltip>
          <template #default="{ row }">{{ keyText(row) }}</template>
        </el-table-column>
        <el-table-column label="上游" min-width="130">
          <template #default="{ row }">{{ row.upstream_name || '-' }}</template>
        </el-table-column>
        <el-table-column label="协议链" min-width="190">
          <template #default="{ row }">
            <div class="proto-cell">
              <span>{{ protoLabel(row.protocol_in) }} → {{ protoLabel(row.protocol_out) }}</span>
              <el-tooltip
                v-if="row.convert_mode !== 'passthrough'"
                :content="`转换方式：${row.convert_mode}`"
                placement="top"
              >
                <el-tag size="small" type="warning" effect="plain" class="convert-tag">转换</el-tag>
              </el-tooltip>
            </div>
          </template>
        </el-table-column>
        <el-table-column label="流式" width="70" align="center">
          <template #default="{ row }">{{ row.stream ? '是' : '否' }}</template>
        </el-table-column>
        <el-table-column label="状态" width="84" align="center">
          <template #default="{ row }">
            <el-tag :type="statusType(row.status)" size="small">{{ row.status }}</el-tag>
          </template>
        </el-table-column>
        <el-table-column label="延迟" width="130">
          <template #default="{ row }">
            <div>{{ fmtDur(row.latency_ms) }}</div>
            <div v-if="row.ttfb_ms != null" class="hint">TTFB {{ row.ttfb_ms }}ms</div>
          </template>
        </el-table-column>
        <el-table-column label="Tokens" min-width="175">
          <template #default="{ row }">
            <div class="token-cell">
              <span>P {{ fmtInt(row.prompt_tokens) }}</span>
              <span>C {{ fmtInt(row.completion_tokens) }}</span>
              <span>Cache {{ fmtInt(row.cache_read_tokens) }}</span>
            </div>
          </template>
        </el-table-column>
        <el-table-column label="成本" width="150">
          <template #default="{ row }">
            <div v-if="row.cost_cny != null || row.cost_usd != null" class="cost-cell">
              <div>¥ {{ fmtMoney(row.cost_cny) }}</div>
              <div>$ {{ fmtMoney(row.cost_usd) }}</div>
            </div>
            <span v-else class="hint">未计价</span>
          </template>
        </el-table-column>
        <el-table-column label="操作" width="80" fixed="right">
          <template #default="{ row }">
            <el-button size="small" link type="primary" @click="openDetail(row)">详情</el-button>
          </template>
        </el-table-column>
      </el-table>
      <div class="pagination">
        <el-pagination
          v-model:current-page="currentPage"
          v-model:page-size="pageSize"
          :total="total"
          :page-sizes="[20, 50, 100]"
          layout="total, sizes, prev, pager, next"
          @current-change="onPageChange"
          @size-change="onSizeChange"
        />
      </div>
    </el-card>

    <!-- 详情抽屉 -->
    <el-drawer v-model="detailVisible" title="请求详情" size="60%">
      <div v-loading="detailLoading">
        <template v-if="detail">
          <el-descriptions :column="2" border>
            <el-descriptions-item label="request_id" :span="2">
              <span class="mono">{{ detail?.request_id }}</span>
              <el-button size="small" link type="primary" @click="copyRequestId(detail?.request_id ?? '')">
                复制
              </el-button>
            </el-descriptions-item>
            <el-descriptions-item label="时间">{{ fmtTime(detail?.ts) }}</el-descriptions-item>
            <el-descriptions-item label="模型">{{ detail?.model }}</el-descriptions-item>
            <el-descriptions-item label="Key">{{ keyText(detail) }}</el-descriptions-item>
            <el-descriptions-item label="上游">{{ detail?.upstream_name || '-' }}</el-descriptions-item>
            <el-descriptions-item label="协议链">
              {{ protoLabel(detail?.protocol_in) }} → {{ protoLabel(detail?.protocol_out) }}
            </el-descriptions-item>
            <el-descriptions-item label="转换方式">{{ detail?.convert_mode || '-' }}</el-descriptions-item>
            <el-descriptions-item label="流式">{{ detail?.stream ? '是' : '否' }}</el-descriptions-item>
            <el-descriptions-item label="状态">{{ detail?.status }}</el-descriptions-item>
            <el-descriptions-item label="降级">{{ detail?.degraded ? '是' : '否' }}</el-descriptions-item>
            <el-descriptions-item label="重试次数">{{ detail?.retry_count }}</el-descriptions-item>
            <el-descriptions-item label="延迟">{{ fmtDur(detail?.latency_ms) }}</el-descriptions-item>
            <el-descriptions-item label="TTFB">
              {{ detail?.ttfb_ms != null ? `${detail?.ttfb_ms}ms` : '-' }}
            </el-descriptions-item>
            <el-descriptions-item label="Prompt Tokens">{{ fmtInt(detail?.prompt_tokens) }}</el-descriptions-item>
            <el-descriptions-item label="Completion Tokens">{{ fmtInt(detail?.completion_tokens) }}</el-descriptions-item>
            <el-descriptions-item label="Cache Write">{{ fmtInt(detail?.cache_write_tokens) }}</el-descriptions-item>
            <el-descriptions-item label="Cache Read">{{ fmtInt(detail?.cache_read_tokens) }}</el-descriptions-item>
            <el-descriptions-item label="图片数">{{ fmtInt(detail?.images) }}</el-descriptions-item>
            <el-descriptions-item label="图片尺寸">{{ detail?.image_size || '-' }}</el-descriptions-item>
            <el-descriptions-item label="视频时长">{{ detail?.video_seconds || '-' }}</el-descriptions-item>
            <el-descriptions-item label="视频分辨率">{{ detail?.video_resolution || '-' }}</el-descriptions-item>
            <el-descriptions-item label="视频任务">{{ detail?.video_task_type || '-' }}</el-descriptions-item>
            <el-descriptions-item label="计价来源">{{ detail?.pricing_source || '-' }}</el-descriptions-item>
            <el-descriptions-item label="成本 CNY">
              {{ detail?.cost_cny != null ? fmtMoney(detail?.cost_cny) : '未计价' }}
            </el-descriptions-item>
            <el-descriptions-item label="成本 USD">
              {{ detail?.cost_usd != null ? fmtMoney(detail?.cost_usd) : '未计价' }}
            </el-descriptions-item>
          </el-descriptions>

          <div v-if="detail?.error" class="error-block">Error: {{ detail.error }}</div>

          <el-collapse v-model="detailOpen" class="json-cards">
            <el-collapse-item title="usage_raw" name="usage_raw">
              <pre class="mono">{{ jsonText(detail?.usage_raw) || '-' }}</pre>
            </el-collapse-item>
            <el-collapse-item title="price_used" name="price_used">
              <pre class="mono">{{ jsonText(detail?.price_used) || '-' }}</pre>
            </el-collapse-item>
            <el-collapse-item title="fx_snapshot" name="fx_snapshot">
              <pre class="mono">{{ jsonText(detail?.fx_snapshot) || '-' }}</pre>
            </el-collapse-item>
            <el-collapse-item title="debug_payload" name="debug_payload">
              <pre v-if="!isEmptyJson(detail?.debug_payload)" class="mono">{{ jsonText(detail?.debug_payload) }}</pre>
              <span v-else class="hint">无完整记录（该 Key 未开 debug 或已过期）</span>
            </el-collapse-item>
          </el-collapse>
        </template>
        <el-empty v-else-if="!detailLoading" description="无详情" :image-size="80" />
      </div>
    </el-drawer>

    <!-- 分区清理弹窗 -->
    <el-dialog v-model="cleanupVisible" title="日志分区清理" width="680px" :close-on-click-modal="false">
      <div class="cleanup-desc hint">
        日志按约 30 天为周期分区存储。清理仅删除「分区边界 to_ts ≤ 所选时间」的完整分区（整分区 DROP），
        未到期的分区不受影响；系统不会自动清理历史分区。建议先点击「预览（dry-run）」查看将删除的分区，
        确认无误后再执行删除。
      </div>
      <div class="f-item" style="margin-bottom: 6px">
        <span class="f-label">before 日期</span>
        <el-date-picker
          v-model="cleanupBefore"
          type="datetime"
          placeholder="选择分区截止时间"
          :clearable="false"
          style="width: 260px"
        />
      </div>
      <div class="hint" style="margin-bottom: 10px">仅删除分区边界 to_ts ≤ before 的整分区。</div>

      <div v-if="cleanupPreviewDone">
        <el-table v-if="cleanupPreview.length" :data="cleanupPreview" size="small" border max-height="260">
          <el-table-column prop="name" label="分区" min-width="180" show-overflow-tooltip />
          <el-table-column label="时间范围" min-width="230">
            <template #default="{ row }">{{ fmtTime(row.from_ts) }} ~ {{ fmtTime(row.to_ts) }}</template>
          </el-table-column>
          <el-table-column label="大小" width="100" align="right">
            <template #default="{ row }">{{ fmtBytes(row.size_bytes) }}</template>
          </el-table-column>
          <el-table-column label="行数" width="110" align="right">
            <template #default="{ row }">{{ fmtInt(row.row_estimate) }}</template>
          </el-table-column>
        </el-table>
        <el-empty v-else description="没有可清理分区" :image-size="80" />
      </div>

      <template #footer>
        <el-button @click="cleanupVisible = false">取消</el-button>
        <el-button type="primary" :loading="cleanupLoading" @click="runCleanupPreview">
          预览（dry-run）
        </el-button>
        <el-button
          v-if="cleanupPreview.length"
          type="danger"
          :loading="cleanupExecuting"
          @click="confirmCleanup"
        >
          确认删除 {{ cleanupPreview.length }} 个分区
        </el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Delete, Download, Refresh, Search } from '@element-plus/icons-vue'
import dayjs from 'dayjs'
import { keyApi, logApi, upstreamApi, type LogListQuery } from '@/api'
import { errMsg } from '@/api/http'
import type { ApiKeyRow, LogItem, PartitionInfo, UpstreamOut } from '@/api/types'
import { PROTOCOL_IN_LABELS, statusType } from '@/utils/consts'
import { downloadBlob } from '@/utils/download'
import { fmtBytes, fmtDur, fmtInt, fmtMoney, fmtTime } from '@/utils/format'

type TriState = '' | 'true' | 'false'

const timeShortcuts = [
  {
    text: '今天',
    value: () => [dayjs().startOf('day').toDate(), dayjs().toDate()],
  },
  {
    text: '昨天',
    value: () => [
      dayjs().startOf('day').subtract(1, 'day').toDate(),
      dayjs().endOf('day').subtract(1, 'day').toDate(),
    ],
  },
  {
    text: '近 7 天',
    value: () => [dayjs().subtract(7, 'day').toDate(), dayjs().toDate()],
  },
  {
    text: '近 30 天',
    value: () => [dayjs().subtract(30, 'day').toDate(), dayjs().toDate()],
  },
]

// —— 筛选状态 ——
const range = ref<[Date, Date] | null>(null)
const keyId = ref('')
const upstreamId = ref('')
const model = ref('')
const requestId = ref('')
const statusStr = ref('')
const streamOpt = ref<TriState>('')
const degradedOpt = ref<TriState>('')

// —— 列表状态 ——
const rows = ref<LogItem[]>([])
const total = ref(0)
const currentPage = ref(1)
const pageSize = ref(20)
const loading = ref(false)

const keys = ref<ApiKeyRow[]>([])
const upstreams = ref<UpstreamOut[]>([])

function tsToIso(d: Date): string {
  return dayjs(d).toISOString()
}

function triToBool(v: TriState): boolean | undefined {
  if (v === 'true') return true
  if (v === 'false') return false
  return undefined
}

function buildFilterQuery(): LogListQuery {
  const q: LogListQuery = {}
  if (range.value) {
    q.from_ts = tsToIso(range.value[0])
    q.to_ts = tsToIso(range.value[1])
  }
  if (keyId.value) q.key_id = keyId.value
  if (upstreamId.value) q.upstream_id = upstreamId.value
  const m = model.value.trim()
  if (m) q.model = m
  const rid = requestId.value.trim()
  if (rid) q.request_id = rid
  if (statusStr.value !== '') {
    const n = Number(statusStr.value)
    if (Number.isFinite(n)) q.status = n
  }
  const stream = triToBool(streamOpt.value)
  if (stream !== undefined) q.stream = stream
  const degraded = triToBool(degradedOpt.value)
  if (degraded !== undefined) q.degraded = degraded
  return q
}

function buildQuery(): LogListQuery {
  return { ...buildFilterQuery(), page: currentPage.value, page_size: pageSize.value }
}

let suppressPageEvent = false

async function loadLogs() {
  loading.value = true
  try {
    const resp = await logApi.list(buildQuery())
    rows.value = resp.items
    total.value = resp.total
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    loading.value = false
  }
}

function onQuery() {
  suppressPageEvent = true
  currentPage.value = 1
  suppressPageEvent = false
  loadLogs()
}

function onReset() {
  range.value = null
  keyId.value = ''
  upstreamId.value = ''
  model.value = ''
  requestId.value = ''
  statusStr.value = ''
  streamOpt.value = ''
  degradedOpt.value = ''
  onQuery()
}

function onPageChange() {
  if (suppressPageEvent) return
  loadLogs()
}

function onSizeChange() {
  suppressPageEvent = true
  currentPage.value = 1
  suppressPageEvent = false
  loadLogs()
}

async function loadKeys() {
  try {
    keys.value = await keyApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

async function loadUpstreams() {
  try {
    upstreams.value = await upstreamApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

onMounted(() => {
  loadKeys()
  loadUpstreams()
  loadLogs()
})

// —— 展示辅助 ——
function keyLabel(k: ApiKeyRow): string {
  return `${k.name} · ${k.prefix}`
}

function keyText(row?: LogItem | null): string {
  if (!row) return '-'
  const parts = [row.key_prefix, row.key_name].filter(Boolean)
  return parts.length ? parts.join(' · ') : '-'
}

function protoLabel(v?: string | null): string {
  if (!v) return '-'
  return PROTOCOL_IN_LABELS[v] ?? v
}

// —— 详情抽屉 ——
const detailVisible = ref(false)
const detailLoading = ref(false)
const detail = ref<LogItem | null>(null)
const detailOpen = ref(['usage_raw'])

async function openDetail(row: LogItem) {
  detailVisible.value = true
  detailLoading.value = true
  detail.value = null
  try {
    detail.value = await logApi.detail(row.request_id)
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    detailLoading.value = false
  }
}

async function copyRequestId(id: string) {
  if (!id) return
  try {
    await navigator.clipboard.writeText(id)
    ElMessage.success('已复制 request_id')
  } catch {
    ElMessage.error('复制失败')
  }
}

function isEmptyJson(v: unknown): boolean {
  if (v === null || v === undefined || v === '') return true
  if (Array.isArray(v)) return v.length === 0
  if (typeof v === 'object') return Object.keys(v).length === 0
  return false
}

function jsonText(v: unknown): string {
  if (isEmptyJson(v)) return ''
  try {
    return JSON.stringify(v, null, 2)
  } catch {
    return String(v)
  }
}

// —— CSV 导出 ——
const exporting = ref(false)

async function exportCsv() {
  exporting.value = true
  try {
    const blob = await logApi.exportCsv(buildFilterQuery())
    downloadBlob(blob, 'usage_logs.csv')
    ElMessage.success('已导出 usage_logs.csv')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    exporting.value = false
  }
}

// —— 分区清理 ——
const cleanupVisible = ref(false)
const cleanupBefore = ref<Date | null>(null)
const cleanupLoading = ref(false)
const cleanupExecuting = ref(false)
const cleanupPreview = ref<PartitionInfo[]>([])
const cleanupPreviewDone = ref(false)

function openCleanup() {
  cleanupBefore.value = null
  cleanupPreview.value = []
  cleanupPreviewDone.value = false
  cleanupVisible.value = true
}

async function runCleanupPreview() {
  if (!cleanupBefore.value) {
    ElMessage.warning('请选择 before 日期')
    return
  }
  cleanupLoading.value = true
  try {
    const resp = await logApi.cleanup({ before: tsToIso(cleanupBefore.value), dry_run: true })
    cleanupPreview.value = resp.dropped
    cleanupPreviewDone.value = true
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    cleanupLoading.value = false
  }
}

async function confirmCleanup() {
  if (!cleanupBefore.value) return
  try {
    await ElMessageBox.confirm(
      `确定删除 ${cleanupPreview.value.length} 个分区？此操作将对整分区执行 DROP，删除的数据不可恢复！`,
      '确认删除分区',
      { type: 'warning', confirmButtonText: '确认删除', confirmButtonClass: 'el-button--danger' },
    )
  } catch {
    return
  }
  cleanupExecuting.value = true
  try {
    const resp = await logApi.cleanup({ before: tsToIso(cleanupBefore.value), dry_run: false })
    const n = resp.dropped.length
    ElMessage.success(`已删除 ${n} 个分区`)
    cleanupVisible.value = false
    loadLogs()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    cleanupExecuting.value = false
  }
}
</script>

<style scoped>
.filter-grid {
  display: flex;
  flex-wrap: wrap;
  gap: 14px 18px;
  align-items: center;
}

.f-item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.f-label {
  color: #606266;
  font-size: 13px;
  white-space: nowrap;
}

.filter-actions {
  margin-top: 14px;
  display: flex;
  gap: 10px;
}

.table-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 12px;
  font-weight: 600;
  color: #303133;
}

.proto-cell {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.convert-tag {
  cursor: default;
}

.token-cell {
  display: flex;
  gap: 8px;
  font-size: 12px;
  color: #303133;
}

.cost-cell {
  font-size: 12px;
  line-height: 1.5;
  color: #303133;
}

.pagination {
  margin-top: 14px;
  display: flex;
  justify-content: flex-end;
}

.json-cards {
  margin-top: 16px;
}

.error-block {
  margin-top: 14px;
  padding: 10px 12px;
  background: #fef0f0;
  color: #f56c6c;
  border-radius: 6px;
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  word-break: break-all;
}

.cleanup-desc {
  margin-bottom: 14px;
  padding: 10px 12px;
  background: #f4f4f5;
  border-radius: 6px;
}
</style>
