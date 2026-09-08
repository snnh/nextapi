<template>
  <div class="page">
    <!-- 顶部工具栏：右侧 CSV 导出 & 分区清理 -->
    <div class="toolbar">
      <div class="hint">时间默认「今天 00:00 ~ 现在」，清空即按服务端默认；筛选与 CSV 导出共用并同步地址栏。</div>
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

    <!-- 筛选工具栏：默认保留 时间/模型/request_id，Key/上游/状态码/流式/降级 归入「高级筛选」（默认折叠） -->
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
          <span class="f-label">模型</span>
          <el-input v-model="model" placeholder="模型" clearable style="width: 160px" @keyup.enter="onQuery" />
        </div>
        <div class="f-item">
          <span class="f-label">request_id</span>
          <el-input
            v-model="requestId"
            placeholder="request_id"
            clearable
            style="width: 200px"
            @keyup.enter="onQuery"
          />
        </div>
      </div>

      <!-- 高级筛选（Key/上游/状态码/流式/降级）：可折叠，避免默认平铺 -->
      <el-collapse-transition>
        <div v-show="advancedOpen" class="filter-grid advanced-grid">
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
            <span class="f-label">状态码</span>
            <el-input
              v-model="statusStr"
              type="number"
              placeholder="如 429"
              clearable
              style="width: 110px"
              @keyup.enter="onQuery"
            />
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
      </el-collapse-transition>

      <div class="filter-actions">
        <el-button type="primary" :icon="Search" @click="onQuery">查询</el-button>
        <el-button :icon="Refresh" @click="onReset">重置</el-button>
        <div class="spacer" />
        <el-tooltip
          content="Key、上游、精确状态码、流式、降级等诊断条件"
          placement="top"
        >
          <el-button link class="adv-toggle" :type="hasAdvancedFilter() ? 'primary' : 'info'" @click="advancedOpen = !advancedOpen">
            高级筛选
            <el-icon :class="{ 'is-open': advancedOpen }"><ArrowDown /></el-icon>
          </el-button>
        </el-tooltip>
      </div>
    </el-card>

    <!-- 日志表格 -->
    <el-card shadow="never" class="page-card">
      <div class="table-head">
        <span>请求日志</span>
      </div>
      <template v-if="listError">
        <div v-loading="loading" class="error-state">
          <p class="error-msg">请求日志加载失败：{{ listError }}</p>
          <el-button type="primary" :icon="Refresh" @click="loadLogs">重新加载</el-button>
        </div>
      </template>
      <template v-else>
      <el-table :data="rows" v-loading="loading" stripe>
        <template #empty>
          <el-empty description="无匹配日志" :image-size="90" />
        </template>
        <el-table-column prop="ts" label="时间" width="170">
          <template #default="{ row }">{{ fmtTime(row.ts) }}</template>
        </el-table-column>
        <el-table-column label="request_id" width="200">
          <template #default="{ row }">
            <CopyText :text="row.request_id" :truncate="16" />
          </template>
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
            <div v-if="row.ttfb_ms != null" class="hint">TTFB {{ fmtDur(row.ttfb_ms) }}</div>
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
      </template>
    </el-card>

    <!-- 详情抽屉 -->
    <el-drawer v-model="detailVisible" title="请求详情" :size="drawerSize">
      <div v-loading="detailLoading">
        <template v-if="detail">
          <el-descriptions :column="descColumn" border>
            <el-descriptions-item label="request_id" :span="descColumn">
              <span class="mono">{{ detail?.request_id }}</span>
              <el-button size="small" link type="primary" @click="copyRequestId(detail?.request_id ?? '')">
                复制
              </el-button>
            </el-descriptions-item>
            <el-descriptions-item label="时间">{{ fmtTime(detail?.ts) }}</el-descriptions-item>
            <el-descriptions-item label="模型">{{ detail?.model }}</el-descriptions-item>
            <el-descriptions-item v-if="detail?.requested_model" label="入口模型">
              {{ detail.requested_model }}<span class="hint">（别名）</span>
            </el-descriptions-item>
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
            <el-descriptions-item label="TTFB">{{ fmtDur(detail?.ttfb_ms) }}</el-descriptions-item>
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
            <el-collapse-item title="request_headers（白名单）" name="request_headers">
              <pre v-if="!isEmptyJson(detail?.request_headers)" class="mono">{{ jsonText(detail?.request_headers) }}</pre>
              <span v-else class="hint">无记录（仅记录 user-agent 等白名单调试头，不含敏感头）</span>
            </el-collapse-item>
            <el-collapse-item title="debug_payload" name="debug_payload">
              <pre v-if="!isEmptyJson(detail?.debug_payload)" class="mono">{{ jsonText(detail?.debug_payload) }}</pre>
              <span v-else class="hint">无完整记录（该 Key 未开 debug 或已过期）</span>
            </el-collapse-item>
          </el-collapse>
        </template>
        <div v-else-if="detailError" class="error-state">
          <p class="error-msg">详情加载失败：{{ detailError }}</p>
          <el-button type="primary" :icon="Refresh" @click="retryDetail">重试</el-button>
        </div>
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
import { computed, onMounted, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { ArrowDown, Delete, Download, Refresh, Search } from '@element-plus/icons-vue'
import dayjs from 'dayjs'
import { useRoute, useRouter } from 'vue-router'
import { keyApi, logApi, upstreamApi, type LogListQuery } from '@/api'
import { errMsg } from '@/api/http'
import type { ApiKeyRow, LogItem, PartitionInfo, UpstreamOut } from '@/api/types'
import CopyText from '@/components/common/CopyText.vue'
import { PROTOCOL_IN_LABELS, statusType } from '@/utils/consts'
import { downloadBlob } from '@/utils/download'
import { fmtBytes, fmtDur, fmtInt, fmtMoney, fmtTime, todayRange } from '@/utils/format'

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

// —— 路由（筛选同步到 URL query）——
const route = useRoute()
const router = useRouter()

// —— 筛选状态 ——
/** 默认时间范围：今天 00:00 ~ 现在（用户清空后仍可恢复服务端默认语义） */
function defaultRange(): [Date, Date] {
  const [s, e] = todayRange()
  return [new Date(s), new Date(e)]
}

const range = ref<[Date, Date] | null>(defaultRange())
const keyId = ref('')
const upstreamId = ref('')
const model = ref('')
const requestId = ref('')
const statusStr = ref('')
const streamOpt = ref<TriState>('')
const degradedOpt = ref<TriState>('')
/** 高级筛选（Key/上游/状态码/流式/降级）是否展开；默认收起，URL 带高级条件时自动展开 */
const advancedOpen = ref(false)

/** 是否存在任意高级筛选条件（用于 URL 恢复自动展开与按钮态提示） */
function hasAdvancedFilter(): boolean {
  return (
    keyId.value !== '' ||
    upstreamId.value !== '' ||
    statusStr.value !== '' ||
    streamOpt.value !== '' ||
    degradedOpt.value !== ''
  )
}

// —— 列表状态 ——
const rows = ref<LogItem[]>([])
const total = ref(0)
const currentPage = ref(1)
const pageSize = ref(20)
const loading = ref(false)
const listError = ref('')
/** 响应式判定：≤768px 视为窄屏（挂载时确定即可） */
const isNarrow = ref(false)

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

// 请求序号：旧响应到达时丢弃，防乱序覆盖新条件的结果（发布审阅前端 M2）
let loadSeq = 0

async function loadLogs() {
  const seq = ++loadSeq
  loading.value = true
  try {
    const resp = await logApi.list(buildQuery())
    if (seq !== loadSeq) return
    rows.value = resp.items
    total.value = resp.total
    listError.value = ''
  } catch (e) {
    if (seq !== loadSeq) return
    rows.value = []
    total.value = 0
    listError.value = errMsg(e)
  } finally {
    if (seq === loadSeq) loading.value = false
  }
}

/** 程序化重置页码（避免触发 pagination 的 current-change 二次加载） */
function resetPage() {
  suppressPageEvent = true
  currentPage.value = 1
  suppressPageEvent = false
}

/** 仅「查询」动作与分页变化时把当前筛选写入 URL query */
function syncQueryToUrl() {
  const f = buildFilterQuery()
  const q: Record<string, string | number> = {}
  if (f.from_ts) q.from_ts = f.from_ts
  if (f.to_ts) q.to_ts = f.to_ts
  if (f.key_id) q.key_id = f.key_id
  if (f.upstream_id) q.upstream_id = f.upstream_id
  if (f.model) q.model = f.model
  if (f.request_id) q.request_id = f.request_id
  if (f.status !== undefined) q.status = f.status
  if (f.stream !== undefined) q.stream = f.stream ? 'true' : 'false'
  if (f.degraded !== undefined) q.degraded = f.degraded ? 'true' : 'false'
  q.page = currentPage.value
  q.page_size = pageSize.value
  // 内容相同则跳过 replace，避免冗余导航
  if (canonicalQuery(q) === canonicalQuery(route.query)) return
  router.replace({ path: route.path, query: q })
}

function onQuery() {
  resetPage()
  syncQueryToUrl()
  loadLogs()
}

function onReset() {
  range.value = defaultRange()
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
  syncQueryToUrl()
  loadLogs()
}

function onSizeChange() {
  resetPage()
  syncQueryToUrl()
  loadLogs()
}

// —— URL query 同步辅助 ——
function canonicalQuery(q: Record<string, unknown>): string {
  return Object.keys(q)
    .filter((k) => q[k] !== undefined && q[k] !== null && q[k] !== '')
    .sort()
    .map((k) => {
      const v = q[k]
      const val = Array.isArray(v) ? v[0] : v
      return `${k}=${String(val)}`
    })
    .join('&')
}

function qv(key: string): string {
  const v = route.query[key]
  if (Array.isArray(v)) return v[0] ?? ''
  return v ?? ''
}

/** 挂载时从 URL query 恢复筛选（page/status 转数字，stream/degraded 转布尔） */
function restoreFromQuery() {
  const from = qv('from_ts')
  const to = qv('to_ts')
  if (from && to) {
    const d1 = new Date(from)
    const d2 = new Date(to)
    if (!Number.isNaN(d1.getTime()) && !Number.isNaN(d2.getTime())) {
      range.value = [d1, d2]
    }
  }
  keyId.value = qv('key_id')
  upstreamId.value = qv('upstream_id')
  model.value = qv('model')
  requestId.value = qv('request_id')

  const status = qv('status')
  statusStr.value = /^\d+$/.test(status) ? status : ''

  const stream = qv('stream')
  streamOpt.value = stream === 'true' || stream === 'false' ? stream : ''

  const degraded = qv('degraded')
  degradedOpt.value = degraded === 'true' || degraded === 'false' ? degraded : ''

  const page = Number.parseInt(qv('page'), 10)
  if (Number.isFinite(page) && page > 0) currentPage.value = page
  const size = Number.parseInt(qv('page_size'), 10)
  if (Number.isFinite(size) && size > 0) pageSize.value = size
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
  restoreFromQuery()
  isNarrow.value = window.innerWidth <= 768
  loadKeys()
  loadUpstreams()
  loadLogs()
})

// —— 展示辅助 ——
function keyLabel(k: ApiKeyRow): string {
  return `${k.name || '(未命名)'} · ${k.prefix}`
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
const detailError = ref('')
const detailRequestId = ref('')
const detailOpen = ref(['usage_raw'])

/** 响应式：≤768px 抽屉全宽、字段单列，桌面 60% / 双列 */
const drawerSize = computed(() => (isNarrow.value ? '100%' : '60%'))
const descColumn = computed(() => (isNarrow.value ? 1 : 2))

async function fetchDetail() {
  const rid = detailRequestId.value
  if (!rid) return
  detailLoading.value = true
  detail.value = null
  detailError.value = ''
  try {
    detail.value = await logApi.detail(rid)
  } catch (e) {
    detailError.value = errMsg(e)
  } finally {
    detailLoading.value = false
  }
}

function openDetail(row: LogItem) {
  detailVisible.value = true
  detailRequestId.value = row.request_id
  fetchDetail()
}

function retryDetail() {
  fetchDetail()
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
    // 删除后当前页可能越界：回到第 1 页再刷新
    onQuery()
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
  color: #4b5563;
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
  color: #1f2329;
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
  color: #1f2329;
}

.cost-cell {
  font-size: 12px;
  line-height: 1.5;
  color: #1f2329;
}

.pagination {
  margin-top: 14px;
  display: flex;
  justify-content: flex-end;
}

.json-cards {
  margin-top: 16px;
}

/* JSON 详情块：抽屉内自滚动，避免内外两层滚动条叠加 */
.json-cards pre {
  margin: 0;
  max-height: 320px;
  overflow: auto;
}

/* 列表 / 详情加载失败的错误块（区别于「无数据」空态） */
.error-state {
  padding: 30px 16px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 14px;
}

.error-msg {
  margin: 0;
  max-width: 100%;
  color: #f56c6c;
  font-size: 13px;
  text-align: center;
  word-break: break-all;
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
