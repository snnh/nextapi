<template>
  <div class="page">
    <div class="page-head">
      <p class="page-desc">网关请求、成本与异常的实时概览；完整系统状态可从顶栏查看。</p>
      <div class="page-head-summary">
        <span v-if="updatedAt">更新于 {{ updatedAt }}</span>
      </div>
    </div>

    <div class="toolbar">
      <el-radio-group v-model="range">
        <el-radio-button value="today">今天</el-radio-button>
        <el-radio-button value="7d">近 7 天</el-radio-button>
      </el-radio-group>
      <div class="spacer" />
      <el-radio-group v-model="currency">
        <el-radio-button value="CNY">CNY</el-radio-button>
        <el-radio-button value="USD">USD</el-radio-button>
      </el-radio-group>
      <el-button :icon="Refresh" :loading="loading || statusLoading" @click="refreshAll">刷新</el-button>
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

    <!-- 待处理事项：仅异常时出现；完整系统状态统一由顶栏状态入口查看 -->
    <section
      v-if="pendingItems.length || (!systemStatus && statusError)"
      class="pending-panel"
      aria-label="待处理事项"
    >
      <div class="pending-head">
        <span class="pending-title">待处理事项</span>
        <span class="hint">{{ pendingItems.length }} 项</span>
        <span v-if="statusError && systemStatus" class="hint">刷新失败，展示上次结果</span>
        <div class="spacer" />
        <el-button link type="primary" size="small" :loading="statusLoading" @click="loadSystemStatus">
          刷新状态
        </el-button>
      </div>
      <ul v-if="pendingItems.length" class="pending-list">
        <li v-for="(it, i) in pendingItems" :key="i" class="pending-item">
          <span class="pending-dot" :class="`dot-${it.level}`" />
          <span class="pending-text">{{ it.text }}</span>
          <div class="spacer" />
          <el-button v-if="it.to" link type="primary" size="small" @click="router.push(it.to)">
            {{ it.action }}
          </el-button>
        </li>
      </ul>
      <div v-else class="pending-error">
        <span>系统状态获取失败：{{ statusError }}</span>
        <el-button link type="primary" size="small" :loading="statusLoading" @click="loadSystemStatus">
          重试
        </el-button>
      </div>
    </section>

    <div v-loading="loading">
      <StatCards :summary="summary" :currency="currency" />

      <el-card class="chart-card section-card" shadow="never">
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
          <el-card class="top-card section-card" shadow="never">
            <template #header>模型 Top 5</template>
            <el-table v-if="modelTop.length" :data="modelTop" size="small">
              <el-table-column prop="name" label="名称" min-width="120" show-overflow-tooltip />
              <el-table-column label="请求数" width="90" align="right">
                <template #default="{ row }">{{ fmtInt(row.requests) }}</template>
              </el-table-column>
              <el-table-column v-if="!isMobile" label="Token 数" width="110" align="right">
                <template #default="{ row }">{{ fmtInt(row.tokens) }}</template>
              </el-table-column>
              <el-table-column v-if="!isTablet" label="成本" width="110" align="right">
                <template #default="{ row }">{{ fmtMoney(row.cost) }}</template>
              </el-table-column>
            </el-table>
            <el-empty v-else description="暂无数据" :image-size="80" />
          </el-card>
        </el-col>
        <el-col :xs="24" :md="12">
          <el-card class="top-card section-card" shadow="never">
            <template #header>上游 Top 5</template>
            <el-table v-if="upstreamTop.length" :data="upstreamTop" size="small">
              <el-table-column prop="name" label="名称" min-width="120" show-overflow-tooltip />
              <el-table-column label="请求数" width="90" align="right">
                <template #default="{ row }">{{ fmtInt(row.requests) }}</template>
              </el-table-column>
              <el-table-column v-if="!isMobile" label="Token 数" width="110" align="right">
                <template #default="{ row }">{{ fmtInt(row.tokens) }}</template>
              </el-table-column>
              <el-table-column v-if="!isTablet" label="成本" width="110" align="right">
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
import { useBreakpoint } from '@/composables/useBreakpoint'
import { Refresh } from '@element-plus/icons-vue'
import { ElMessage } from 'element-plus'
import EChart from '@/components/EChart.vue'
import StatCards from '@/components/stats/StatCards.vue'
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
import { fmtInt, fmtMoney } from '@/utils/format'

type Range = 'today' | '7d'

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
const { isMobile, isTablet } = useBreakpoint()
const systemStatus = ref<SystemStatusResp | null>(null)
const statusLoading = ref(false)
// 状态拉取失败文案（成功后清空；已成功过则保留上次数据并提示快照）
const statusError = ref<string | null>(null)

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

async function loadSystemStatus() {
  if (statusLoading.value) return // 防连点
  statusLoading.value = true
  statusError.value = null
  try {
    systemStatus.value = await systemApi.status()
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
    row.tokens +=
      (p.prompt_tokens ?? 0) +
      (p.completion_tokens ?? 0) +
      (p.cache_read_tokens ?? 0) +
      (p.cache_write_tokens ?? 0)
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
.error-alert {
  margin-bottom: 16px;
}

/* —— 待处理事项：仅异常时出现，不占用正常首屏 —— */
.pending-panel {
  margin-bottom: 16px;
  padding: 12px 16px;
  border: 1px solid var(--nx-warning-border);
  border-radius: var(--nx-radius-md);
  background: var(--nx-warning-soft);
}
.pending-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}
.pending-title {
  font-size: var(--nx-font-small);
  font-weight: 600;
  color: var(--nx-text);
}
.pending-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
  margin: 0;
  padding: 0;
  list-style: none;
}
.pending-item {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  font-size: var(--nx-font-small);
  line-height: 1.5;
}
.pending-item .spacer {
  flex: 1;
}
.pending-text {
  color: var(--nx-text-2);
  word-break: break-all;
}
.pending-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
}
.dot-warn {
  background: var(--nx-warning);
}
.dot-danger {
  background: var(--nx-danger-bright);
}
.pending-error {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  color: var(--nx-text-2);
  font-size: var(--nx-font-small);
}

.error-body {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  font-size: var(--nx-font-small);
  line-height: 1.6;
}


.top-row {
  margin-top: 0;
}


@media (max-width: 800px) {
  .pending-panel {
    padding: 10px 12px;
  }
  .pending-item {
    align-items: flex-start;
    flex-wrap: wrap;
  }
}
</style>
