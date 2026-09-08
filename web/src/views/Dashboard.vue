<template>
  <div class="page">
    <div class="toolbar">
      <el-radio-group v-model="range">
        <el-radio-button value="today">今天</el-radio-button>
        <el-radio-button value="7d">近 7 天</el-radio-button>
      </el-radio-group>
      <div class="spacer" />
      <el-radio-group v-model="currency" size="small">
        <el-radio-button value="CNY">CNY</el-radio-button>
        <el-radio-button value="USD">USD</el-radio-button>
      </el-radio-group>
      <el-button :icon="Refresh" :loading="loading || statusLoading" @click="refreshAll">刷新</el-button>
      <span v-if="updatedAt" class="hint updated-at">更新于 {{ updatedAt }}</span>
    </div>

    <!-- 加载失败提示：保留上一次数据，不再伪装成「无数据」 -->
    <el-alert v-if="errorMsg" type="error" show-icon :closable="false" class="error-alert">
      <template #title>数据加载失败：{{ errorMsg }}</template>
      <div class="error-body">
        <span>当前展示内容为上一次成功加载的结果；首次加载失败时暂无可展示数据。</span>
        <el-button size="small" type="danger" plain :icon="Refresh" :loading="loading" @click="loadAll">
          重试
        </el-button>
      </div>
    </el-alert>

    <!-- 系统状态 / 待处理事项：仅复用现有 systemApi.status 数据（M13 §4.3 低风险改造，
         不新增后端接口；拉取失败时仅提示文案，不伪造任何状态） -->
    <el-card class="status-card" shadow="never">
      <template #header>
        <div class="status-head">
          <span class="status-title">系统状态 · 待处理事项</span>
          <div class="spacer" />
          <el-tag v-if="systemStatus" :type="healthTag" size="small" effect="light">{{ healthText }}</el-tag>
          <span v-if="statusUpdatedAt" class="hint status-updated">更新于 {{ statusUpdatedAt }}</span>
        </div>
      </template>

      <div v-loading="statusLoading" class="status-body">
        <!-- 拉取失败：保留上次成功结果（若有）并明确提示，不猜测当前状态 -->
        <el-alert
          v-if="statusError"
          :type="systemStatus ? 'warning' : 'info'"
          show-icon
          :closable="false"
          class="status-alert"
        >
          <template #title>
            {{ systemStatus ? '系统状态刷新失败，当前展示为上次成功结果' : `系统状态获取失败：${statusError}` }}
          </template>
          <div v-if="!systemStatus" class="status-err-sub">
            状态与待办摘要暂不可展示，请求日志等统计内容不受影响。
            <el-button link type="primary" size="small" :loading="statusLoading" @click="loadSystemStatus">重试</el-button>
          </div>
        </el-alert>

        <template v-else-if="systemStatus">
          <!-- 子系统速览 -->
          <div class="status-kv-grid">
            <div class="kv-item">
              <div class="kv-label">数据库</div>
              <div class="kv-value" :class="{ 'is-warn': !systemStatus.database.ok }">
                {{ systemStatus.database.ok ? '正常' : '异常' }}
                <span class="hint">{{ systemStatus.database.latency_ms }}ms</span>
              </div>
            </div>
            <div class="kv-item">
              <div class="kv-label">运行时长</div>
              <div class="kv-value">{{ fmtUptime(systemStatus.uptime_secs) }}</div>
            </div>
            <div class="kv-item">
              <div class="kv-label">熔断/禁用上游</div>
              <div class="kv-value" :class="{ 'is-warn': systemStatus.breakers.length > 0 }">
                {{ systemStatus.breakers.length ? `${systemStatus.breakers.length} 个` : '无' }}
              </div>
            </div>
            <div class="kv-item">
              <div class="kv-label">日志队列</div>
              <div class="kv-value">
                <template v-if="systemStatus.logging.queue_capacity != null">
                  {{ systemStatus.logging.queue_used }} / {{ systemStatus.logging.queue_capacity }}
                </template>
                <template v-else>同步直写</template>
                <span v-if="systemStatus.logging.overflow_total > 0" class="kv-note is-warn">
                  溢出 {{ systemStatus.logging.overflow_total }}
                </span>
              </div>
            </div>
            <div class="kv-item">
              <div class="kv-label">WAL 兜底</div>
              <div class="kv-value">
                {{ systemStatus.logging.wal_files }} 文件
                <span class="hint">{{ fmtBytes(systemStatus.logging.wal_bytes) }}</span>
              </div>
            </div>
            <div class="kv-item">
              <div class="kv-label">媒体轮询</div>
              <div class="kv-value">
                <template v-if="systemStatus.media_poller.interval_secs > 0">
                  每 {{ systemStatus.media_poller.interval_secs }}s
                </template>
                <template v-else>已停用</template>
                <span v-if="systemStatus.media_poller.pending_tasks > 0" class="kv-note is-warn">
                  待回联 {{ systemStatus.media_poller.pending_tasks }}
                </span>
              </div>
            </div>
          </div>

          <!-- 待处理事项：完全由 status 数据推导，命中异常才列出 -->
          <el-divider content-position="left">待处理事项</el-divider>
          <div v-if="pendingItems.length" class="todo-list">
            <div v-for="(it, i) in pendingItems" :key="i" class="todo-item">
              <span class="todo-dot" :class="`dot-${it.level}`" />
              <span class="todo-text">{{ it.text }}</span>
              <div class="spacer" />
              <el-button v-if="it.to" link type="primary" size="small" @click="router.push(it.to)">
                {{ it.action }}
              </el-button>
            </div>
          </div>
          <div v-else class="todo-empty">
            <span class="todo-dot dot-ok" />
            <span>系统各项状态正常，暂无待处理事项。</span>
          </div>
        </template>
      </div>
    </el-card>

    <div v-loading="loading">
      <!-- 统计指标卡：CSS grid 自适应列数（含延迟卡），大数字溢出省略 -->
      <div class="stat-grid">
        <el-card v-for="m in metricCards" :key="m.label" class="stat-card" shadow="never">
          <div class="stat-label">
            <el-tooltip :content="m.tip" placement="top">
              <span>{{ m.label }}</span>
            </el-tooltip>
          </div>
          <div class="stat-value">{{ m.value }}</div>
          <div v-if="m.sub" class="stat-sub hint">{{ m.sub }}</div>
        </el-card>

        <el-card class="stat-card" shadow="never">
          <div class="stat-label">
            <el-tooltip content="响应耗时的分位延迟（P50/P95/P99）" placement="top">
              <span>延迟</span>
            </el-tooltip>
          </div>
          <div class="stat-value delay">
            <el-tooltip content="P50 分位延迟：50% 的请求 ≤ 该耗时" placement="top">
              <span>P50 <b>{{ fmtDur(summary?.p50_ms) }}</b></span>
            </el-tooltip>
            <el-tooltip content="P95 分位延迟：95% 的请求 ≤ 该耗时" placement="top">
              <span>P95 <b>{{ fmtDur(summary?.p95_ms) }}</b></span>
            </el-tooltip>
            <el-tooltip content="P99 分位延迟：99% 的请求 ≤ 该耗时" placement="top">
              <span>P99 <b>{{ fmtDur(summary?.p99_ms) }}</b></span>
            </el-tooltip>
          </div>
        </el-card>
      </div>

      <el-card class="chart-card" shadow="never">
        <template #header>请求数 &amp; 成本趋势（{{ rangeLabel }}）</template>
        <EChart
          v-if="totalSeries.length"
          :option="lineOption"
          :loading="loading"
          height="320px"
          :aria-label="`请求数与成本趋势图（${rangeLabel}，${currency}）`"
        />
        <el-empty v-else description="暂无数据" :image-size="90" />
      </el-card>

      <el-row :gutter="16" class="top-row">
        <el-col :xs="24" :md="12">
          <el-card class="top-card" shadow="never">
            <template #header>模型 Top 5</template>
            <el-table v-if="modelTop.length" :data="modelTop" size="small">
              <el-table-column prop="name" label="名称" min-width="120" show-overflow-tooltip />
              <el-table-column label="请求数" width="90" align="right">
                <template #default="{ row }">{{ fmtInt(row.requests) }}</template>
              </el-table-column>
              <el-table-column label="Token 数" width="110" align="right">
                <template #default="{ row }">{{ fmtInt(row.tokens) }}</template>
              </el-table-column>
              <el-table-column label="成本" width="110" align="right">
                <template #default="{ row }">{{ fmtMoney(row.cost) }}</template>
              </el-table-column>
            </el-table>
            <el-empty v-else description="暂无数据" :image-size="80" />
          </el-card>
        </el-col>
        <el-col :xs="24" :md="12">
          <el-card class="top-card" shadow="never">
            <template #header>上游 Top 5</template>
            <el-table v-if="upstreamTop.length" :data="upstreamTop" size="small">
              <el-table-column prop="name" label="名称" min-width="120" show-overflow-tooltip />
              <el-table-column label="请求数" width="90" align="right">
                <template #default="{ row }">{{ fmtInt(row.requests) }}</template>
              </el-table-column>
              <el-table-column label="Token 数" width="110" align="right">
                <template #default="{ row }">{{ fmtInt(row.tokens) }}</template>
              </el-table-column>
              <el-table-column label="成本" width="110" align="right">
                <template #default="{ row }">{{ fmtMoney(row.cost) }}</template>
              </el-table-column>
            </el-table>
            <el-empty v-else description="暂无数据" :image-size="80" />
          </el-card>
        </el-col>
      </el-row>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import dayjs from 'dayjs'
import { useRouter } from 'vue-router'
import { Refresh } from '@element-plus/icons-vue'
import { ElMessage } from 'element-plus'
import EChart from '@/components/EChart.vue'
import type { EChartsOption } from 'echarts'
import { statsApi, systemApi } from '@/api'
import { errMsg } from '@/api/http'
import type {
  SeriesDimension,
  SeriesGranularity,
  SeriesPoint,
  StatsCurrency,
  StatsQuery,
  StatsSummary,
  SystemStatusResp,
} from '@/api/types'
import { fmtBytes, fmtDur, fmtInt, fmtMoney, fmtPct } from '@/utils/format'

type Range = 'today' | '7d'

// 指标卡释义文案（与 stats/StatCards.vue 同口径）
const LABEL_TIPS = {
  requests: '统计区间内的网关请求总数',
  success: '成功率 = 状态码 < 400 的请求占比',
  tokens: '总 Token 数 = 输入（提示）+ 输出（生成）token 合计',
  cost: '按所选币种折算的合计成本；未计价（无定价规则）条数不计入',
} as const

const range = ref<Range>('today')
const currency = ref<StatsCurrency>('CNY')
const loading = ref(false)

const summary = ref<StatsSummary | null>(null)
const totalSeries = ref<SeriesPoint[]>([])
const modelSeries = ref<SeriesPoint[]>([])
const upstreamSeries = ref<SeriesPoint[]>([])

// 加载失败错误文案（非 null 时顶部展示错误提示条）；成功加载后清空
const errorMsg = ref<string | null>(null)
// 最近一次成功加载完成时间，格式 HH:mm:ss
const updatedAt = ref<string | null>(null)

// —— 系统状态 / 待处理事项（复用 systemApi.status，独立于统计加载，失败不影响主内容）——
const router = useRouter()
const systemStatus = ref<SystemStatusResp | null>(null)
const statusLoading = ref(false)
// 状态拉取失败文案（成功后清空；已成功过则保留上次数据并提示快照）
const statusError = ref<string | null>(null)
const statusUpdatedAt = ref<string | null>(null)

type Health = 'ok' | 'warn' | 'danger'
const health = computed<Health>(() => {
  const s = systemStatus.value
  if (!s) return 'ok'
  if (!s.database.ok || s.breakers.length > 0) return 'danger'
  if (
    s.logging.overflow_total > 0 ||
    s.media_poller.pending_tasks > 0 ||
    s.recent_errors.length > 0
  ) {
    return 'warn'
  }
  return 'ok'
})
const healthText = computed(() => (({ ok: '正常', warn: '需关注', danger: '异常' }) as const)[health.value])
const healthTag = computed(() => (({ ok: 'success', warn: 'warning', danger: 'danger' }) as const)[health.value])

interface PendingItem {
  level: 'warn' | 'danger'
  text: string
  to?: string
  action: string
}

/** 待处理事项由 status 现有字段推导：数据库、熔断/禁用上游、日志溢出、媒体待回联、最近错误 */
const pendingItems = computed<PendingItem[]>(() => {
  const s = systemStatus.value
  if (!s) return []
  const items: PendingItem[] = []
  if (!s.database.ok) {
    items.push({
      level: 'danger',
      text: `数据库连接异常（延迟 ${s.database.latency_ms}ms），请检查数据库可用性`,
      to: '/logs',
      action: '查看日志',
    })
  }
  for (const b of s.breakers) {
    items.push({
      level: 'danger',
      text: `上游「${b.name}」${b.disabled_by === 'manual' ? '被手动禁用' : '触发熔断'}（连续失败 ${b.consecutive_failures} 次）`,
      to: '/upstreams',
      action: '处理上游',
    })
  }
  if (s.logging.overflow_total > 0) {
    items.push({
      level: 'warn',
      text: `日志队列累计溢出 ${s.logging.overflow_total} 条，已由 WAL 兜底落盘`,
      to: '/settings?tab=system',
      action: '查看详情',
    })
  }
  if (s.media_poller.pending_tasks > 0) {
    items.push({
      level: 'warn',
      text: `媒体轮询存在 ${s.media_poller.pending_tasks} 个待回联任务`,
      to: '/settings?tab=system',
      action: '查看配置',
    })
  }
  if (s.recent_errors.length > 0) {
    items.push({
      level: 'warn',
      text: `最近有 ${s.recent_errors.length} 条请求错误（HTTP ≥ 400）`,
      to: '/logs',
      action: '查看日志',
    })
  }
  return items
})

/** 秒级运行时长人性化（与 SystemInfoPanel 同口径，@/utils/format 无等价函数） */
function fmtUptime(secs: number): string {
  if (secs < 60) return `${Math.floor(secs)} 秒`
  if (secs < 3600) return `${Math.floor(secs / 60)} 分钟`
  if (secs < 86400) return `${Math.floor(secs / 3600)} 小时 ${Math.floor((secs % 3600) / 60)} 分`
  return `${Math.floor(secs / 86400)} 天 ${Math.floor((secs % 86400) / 3600)} 小时`
}

async function loadSystemStatus() {
  if (statusLoading.value) return // 防连点
  statusLoading.value = true
  statusError.value = null
  try {
    systemStatus.value = await systemApi.status()
    statusUpdatedAt.value = dayjs().format('HH:mm:ss')
  } catch (e) {
    // 保留上次成功结果（若有）；失败仅展示提示文案，不伪造当前状态
    statusError.value = errMsg(e)
  } finally {
    statusLoading.value = false
  }
}

/** 手动刷新：统计 + 系统状态 并行（系统状态独立失败，不影响统计数据） */
function refreshAll() {
  loadAll()
  loadSystemStatus()
}

const rangeLabel = computed(() => (range.value === '7d' ? '近 7 天' : '今天'))

const granularity = computed<SeriesGranularity>(() => (range.value === '7d' ? 'day' : 'hour'))

interface MetricCard {
  label: string
  value: string
  sub: string
  tip: string
}

const metricCards = computed<MetricCard[]>(() => {
  const s = summary.value
  return [
    { label: '请求数', value: fmtInt(s?.requests), sub: '', tip: LABEL_TIPS.requests },
    { label: '成功率', value: fmtPct(s?.success_rate), sub: '', tip: LABEL_TIPS.success },
    { label: '总 Token 数', value: fmtInt(s?.total_tokens), sub: '', tip: LABEL_TIPS.tokens },
    {
      label: `成本（${currency.value}）`,
      value: fmtMoney(s?.cost_display),
      sub: s && s.cost_na_count > 0 ? `${s.cost_na_count} 条未计价` : '',
      tip: LABEL_TIPS.cost,
    },
  ]
})

interface TopRow {
  name: string
  requests: number
  tokens: number
  cost: number
}

function aggregateDim(points: SeriesPoint[]): TopRow[] {
  const map = new Map<string, TopRow>()
  for (const p of points) {
    const name = p.dimension || '—'
    const row = map.get(name) ?? { name, requests: 0, tokens: 0, cost: 0 }
    row.requests += p.requests
    row.tokens += (p.prompt_tokens ?? 0) + (p.completion_tokens ?? 0)
    row.cost += Number(p.cost_display ?? 0)
    map.set(name, row)
  }
  return [...map.values()].sort((a, b) => b.requests - a.requests).slice(0, 5)
}

const modelTop = computed(() => aggregateDim(modelSeries.value))
const upstreamTop = computed(() => aggregateDim(upstreamSeries.value))

function fmtBucket(bucket: string, g: SeriesGranularity): string {
  const d = dayjs(bucket)
  if (!d.isValid()) return bucket
  return g === 'hour' ? d.format('HH:mm') : d.format('MM-DD')
}

const lineOption = computed<EChartsOption>(() => {
  const pts = totalSeries.value
  const labels = pts.map((p) => fmtBucket(p.bucket, granularity.value))
  const requests = pts.map((p) => p.requests)
  const costs = pts.map((p) => (p.cost_display != null ? Number(p.cost_display) : null))
  return {
    tooltip: { trigger: 'axis' },
    legend: { data: ['请求数', `成本（${currency.value}）`] },
    grid: { left: 10, right: 24, top: 36, bottom: 10, containLabel: true },
    xAxis: { type: 'category', boundaryGap: false, data: labels },
    yAxis: [
      { type: 'value' },
      { type: 'value', splitLine: { show: false } },
    ],
    series: [
      { name: '请求数', type: 'line', smooth: true, connectNulls: true, yAxisIndex: 0, data: requests },
      { name: `成本（${currency.value}）`, type: 'line', smooth: true, connectNulls: true, yAxisIndex: 1, data: costs },
    ],
  }
})

function baseQuery(): StatsQuery {
  const q: StatsQuery = { currency: currency.value }
  if (range.value === '7d') {
    q.from_ts = dayjs().subtract(7, 'day').toISOString()
  }
  return q
}

function seriesQuery(dimension?: SeriesDimension): StatsQuery {
  const q = baseQuery()
  q.granularity = granularity.value
  if (dimension) q.dimension = dimension
  return q
}

let reqSeq = 0

async function loadAll() {
  const my = ++reqSeq
  loading.value = true
  // 新一轮请求开始：清除旧错误提示（失败时重新置回）
  errorMsg.value = null
  try {
    const [sum, total, model, upstream] = await Promise.all([
      statsApi.summary(baseQuery()),
      statsApi.series(seriesQuery()),
      statsApi.series(seriesQuery('model')),
      statsApi.series(seriesQuery('upstream')),
    ])
    if (my !== reqSeq) return
    summary.value = sum
    totalSeries.value = total.points
    modelSeries.value = model.points
    upstreamSeries.value = upstream.points
    updatedAt.value = dayjs().format('HH:mm:ss')
  } catch (e) {
    if (my !== reqSeq) return
    // 加载失败：保留上一次数据（若有），仅提示错误；首次加载且无数据时才保持全空
    const msg = errMsg(e)
    errorMsg.value = msg
    ElMessage.error(msg)
  } finally {
    if (my === reqSeq) loading.value = false
  }
}

onMounted(refreshAll)
watch([range, currency], () => loadAll())
</script>

<style scoped>
/* 统计指标卡：自适应列数网格（含延迟卡），避免 el-col 固定 4 格在宽屏留白 */
.stat-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
  gap: 16px;
  margin-bottom: 16px;
}

.stat-card {
  min-width: 0; /* 允许内部省略号收缩 */
  box-sizing: border-box;
}

.error-alert {
  margin-bottom: 16px;
}

/* —— 系统状态 / 待处理事项（M13 §4.3 低风险区块） —— */
.status-card {
  margin-bottom: 16px;
}
.status-head {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
}
.status-head .spacer {
  flex: 1;
}
.status-title {
  font-size: 14px;
  font-weight: 600;
  color: #1f2329;
}
.status-updated {
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}
.status-body {
  min-height: 60px;
}
.status-alert {
  margin-bottom: 0;
}
.status-err-sub {
  margin-top: 4px;
  font-size: 13px;
  line-height: 1.7;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
}
.status-kv-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
  gap: 14px;
}
.kv-item {
  min-width: 0;
}
.kv-label {
  font-size: 12px;
  color: #909399;
  margin-bottom: 4px;
}
.kv-value {
  font-size: 15px;
  font-weight: 600;
  color: #1f2329;
  line-height: 1.4;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.kv-value .hint {
  font-weight: 400;
  margin-left: 6px;
}
.kv-note {
  display: inline-block;
  margin-left: 8px;
  font-weight: 600;
}
.is-warn {
  color: #d4380d;
}
.status-body :deep(.el-divider) {
  margin: 16px 0 12px;
}
.todo-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.todo-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  background: #fafafa;
  border: 1px solid #f0f2f5;
  border-radius: 6px;
  font-size: 13px;
  line-height: 1.5;
}
.todo-item .spacer {
  flex: 1;
}
.todo-text {
  color: #303133;
  word-break: break-all;
}
.todo-empty {
  display: flex;
  align-items: center;
  gap: 8px;
  color: #6b7280;
  font-size: 13px;
}
.todo-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}
.dot-warn {
  background: #e6a23c;
}
.dot-danger {
  background: #f56c6c;
}
.dot-ok {
  background: #67c23a;
}

.error-body {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  line-height: 1.6;
}

.updated-at {
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

.stat-label {
  font-size: 13px;
  color: #6b7280;
  margin-bottom: 8px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.stat-value {
  font-size: 24px;
  font-weight: 600;
  color: #1f2329;
  line-height: 1.2;
  white-space: nowrap; /* 大数字单行溢出省略，防破卡 */
  overflow: hidden;
  text-overflow: ellipsis;
}

.stat-value.delay {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 16px;
  font-size: 14px;
  font-weight: 400;
  white-space: normal;
  overflow: visible;
}

.stat-value.delay b {
  font-weight: 600;
  color: #1f2329;
}

.stat-sub {
  margin-top: 6px;
}

.chart-card {
  margin-bottom: 16px;
}

.top-row {
  margin-top: 0;
}

.top-card {
  margin-bottom: 16px;
}
</style>
