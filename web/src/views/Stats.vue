<template>
  <div class="page">
    <!-- 顶部工具栏：说明 + CSV 导出 -->
    <div class="toolbar">
      <div class="hint">按时间范围 / 维度 / 币种 / Key / 上游查询统计；粒度由后端自动选择。</div>
      <div class="spacer" />
      <el-button
        type="primary"
        plain
        :icon="Download"
        :loading="exporting"
        :disabled="!hasData"
        @click="exportCsv"
      >
        导出 CSV
      </el-button>
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
          <el-select v-model="keyId" filterable clearable placeholder="全部 Key" style="width: 190px">
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
            style="width: 170px"
          >
            <el-option v-for="u in upstreams" :key="u.id" :label="u.name" :value="u.id" />
          </el-select>
        </div>
        <div class="f-item">
          <span class="f-label">维度</span>
          <el-select v-model="dimension" style="width: 148px">
            <el-option v-for="o in DIMENSION_OPTIONS" :key="o.value" :label="o.label" :value="o.value" />
          </el-select>
        </div>
        <div class="f-item">
          <span class="f-label">币种</span>
          <el-select v-model="currency" style="width: 92px">
            <el-option v-for="c in CURRENCIES" :key="c" :label="c" :value="c" />
          </el-select>
        </div>
        <div class="filter-actions">
          <el-button type="primary" :icon="Search" :loading="loading" @click="onQuery">查询</el-button>
        </div>
      </div>
      <div class="hint granularity-hint">{{ granularityHint }}</div>
    </el-card>

    <div v-loading="loading">
      <!-- 汇总卡 -->
      <StatCards :summary="summary" :currency="currency" />

      <!-- 趋势图 -->
      <el-card shadow="never" class="page-card">
        <template #header>{{ chartTitle }}</template>
        <EChart v-if="hasData" :option="chartOption" height="360px" />
        <el-empty v-else :description="emptyText" :image-size="90" />
        <div class="metric-switch">
          <span class="hint">切换指标：</span>
          <el-radio-group v-model="metric" size="small">
            <el-radio-button value="requests">请求数</el-radio-button>
            <el-radio-button value="tokens">Token 数</el-radio-button>
            <el-radio-button value="cost">成本</el-radio-button>
          </el-radio-group>
        </div>
      </el-card>

      <!-- 维度明细表 -->
      <el-card v-if="dimension" shadow="never" class="page-card">
        <template #header>维度明细</template>
        <el-table :data="detailRows" stripe max-height="420">
          <el-table-column label="时间" width="140">
            <template #default="{ row }">{{ row.bucketText }}</template>
          </el-table-column>
          <el-table-column prop="dimLabel" label="维度" min-width="140" show-overflow-tooltip />
          <el-table-column label="请求数" width="104" align="right">
            <template #default="{ row }">{{ fmtInt(row.requests) }}</template>
          </el-table-column>
          <el-table-column label="错误" width="88" align="right">
            <template #default="{ row }">{{ fmtInt(row.errors) }}</template>
          </el-table-column>
          <el-table-column label="Token 数" width="128" align="right">
            <template #default="{ row }">{{ fmtInt((row.prompt_tokens ?? 0) + (row.completion_tokens ?? 0)) }}</template>
          </el-table-column>
          <el-table-column label="成本" width="128" align="right">
            <template #default="{ row }">{{ fmtMoney(row.cost_display) }}</template>
          </el-table-column>
        </el-table>
      </el-card>
    </div>
  </div>
</template>

<script setup lang="ts">
// 统计报表页（M8 §4.8）：汇总卡 + 趋势图 + 维度明细 + CSV 导出
import { computed, onMounted, ref, watch } from 'vue'
import dayjs from 'dayjs'
import { ElMessage } from 'element-plus'
import { Download, Search } from '@element-plus/icons-vue'
import EChart from '@/components/EChart.vue'
import StatCards from '@/components/stats/StatCards.vue'
import type { EChartsOption } from 'echarts'
import { keyApi, statsApi, upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import type {
  ApiKeyRow,
  SeriesDimension,
  SeriesPoint,
  StatsCurrency,
  StatsQuery,
  StatsSummary,
  UpstreamOut,
} from '@/api/types'
import { CURRENCIES, DIMENSION_OPTIONS, PROTOCOL_IN_LABELS } from '@/utils/consts'
import { downloadBlob } from '@/utils/download'
import { fmtInt, fmtMoney } from '@/utils/format'

type Metric = 'requests' | 'tokens' | 'cost'

// —— 筛选状态 ——
const range = ref<[Date, Date] | null>([
  dayjs().subtract(7, 'day').toDate(),
  dayjs().toDate(),
])
const dimension = ref<SeriesDimension>('')
const currency = ref<StatsCurrency>('CNY')
const keyId = ref('')
const upstreamId = ref('')
const keys = ref<ApiKeyRow[]>([])
const upstreams = ref<UpstreamOut[]>([])
const metric = ref<Metric>('requests')

// —— 数据状态 ——
const loading = ref(false)
const exporting = ref(false)
/** 是否成功加载过一次（区分「从未有数据」与「当前区间无数据」） */
const loadedOnce = ref(false)
const summary = ref<StatsSummary | null>(null)
const points = ref<SeriesPoint[]>([])

const timeShortcuts = [
  { text: '今天', value: () => [dayjs().startOf('day').toDate(), dayjs().toDate()] },
  { text: '近 7 天', value: () => [dayjs().subtract(7, 'day').toDate(), dayjs().toDate()] },
  { text: '近 30 天', value: () => [dayjs().subtract(30, 'day').toDate(), dayjs().toDate()] },
  { text: '近 90 天', value: () => [dayjs().subtract(90, 'day').toDate(), dayjs().toDate()] },
]

// —— 粒度/数据源自动推断（仅用于前端 UI 提示与 x 轴格式，不随请求发送） ——
const spanHours = computed(() => {
  if (!range.value) return 7 * 24
  return dayjs(range.value[1]).diff(dayjs(range.value[0]), 'hour')
})
const spanDays = computed(() => Math.round(spanHours.value / 24))
const hourMode = computed(() => spanHours.value <= 48)
/** 是否带 Key/上游筛选（usage_hourly 预聚合表无这些列，命中即强制明细源） */
const hasKeyUpstreamFilter = computed(() => !!keyId.value || !!upstreamId.value)
/** 后端 series 可能走预聚合小时表的条件：按天粒度 + 无 Key/上游筛选 + 仅总量/model 维度 */
const mayUseHourly = computed(
  () => !hasKeyUpstreamFilter.value && (dimension.value === '' || dimension.value === 'model'),
)

const granularityHint = computed(() => {
  if (hourMode.value) {
    // 自动 hour 粒度仅出现在 ≤2 天区间，且 pick_source 恒回退明细源（无 span>7 命中）。
    return '区间 ≤2 天：自动按小时粒度（回退明细聚合）。'
  }
  const base = spanDays.value > 7 ? '区间 >7 天：自动按天粒度。' : '区间 2~7 天：自动按天粒度。'
  if (mayUseHourly.value) {
    return `${base} 未筛选 Key/上游，后端读取每小时预聚合表。`
  }
  return `${base} 已筛选 Key/上游或按 Key/上游/协议分组，后端回退明细聚合。`
})

/** 最近一次加载失败的原因（从未成功加载时用于空态提示） */
const loadError = ref('')
const emptyText = computed(() => {
  if (loadedOnce.value) return '当前条件下暂无数据'
  return loadError.value ? `加载失败：${loadError.value}` : '加载中…'
})

// —— 下拉选项加载 ——
async function loadFilterOptions() {
  try {
    const [k, u] = await Promise.all([keyApi.list(), upstreamApi.list()])
    keys.value = k
    upstreams.value = u
  } catch (e) {
    ElMessage.error(`Key/上游选项加载失败：${errMsg(e)}`)
  }
}

// —— 展示辅助 ——
const sortedPoints = computed<SeriesPoint[]>(() =>
  [...points.value].sort((a, b) => a.bucket.localeCompare(b.bucket)),
)
const hasData = computed(() => sortedPoints.value.length > 0)

function buildBaseQuery(): StatsQuery {
  const q: StatsQuery = { currency: currency.value }
  if (range.value) {
    q.from_ts = dayjs(range.value[0]).toISOString()
    q.to_ts = dayjs(range.value[1]).toISOString()
  }
  if (keyId.value) q.key_id = keyId.value
  if (upstreamId.value) q.upstream_id = upstreamId.value
  return q
}

function buildSeriesQuery(): StatsQuery {
  const q = buildBaseQuery()
  const d = dimension.value
  if (d) q.dimension = d
  return q
}

let reqSeq = 0

// 查询期间不清空旧数据：失败保留旧 summary/points 仅提示，空态只在从未成功加载过时出现。
async function loadData() {
  const my = ++reqSeq
  loading.value = true
  try {
    const [sum, ser] = await Promise.all([
      statsApi.summary(buildBaseQuery()),
      statsApi.series(buildSeriesQuery()),
    ])
    if (my !== reqSeq) return
    summary.value = sum
    points.value = ser.points
    loadedOnce.value = true
    loadError.value = ''
  } catch (e) {
    if (my !== reqSeq) return
    // 保留旧数据，不重置为空态
    const m = errMsg(e)
    loadError.value = m
    ElMessage.error(m)
  } finally {
    if (my === reqSeq) loading.value = false
  }
}

/** 「查询」按钮：手动立即触发（先取消待执行的自动防抖查询） */
let autoTimer: ReturnType<typeof setTimeout> | null = null
function onQuery() {
  if (autoTimer) {
    clearTimeout(autoTimer)
    autoTimer = null
  }
  loadData()
}

// 时间/币种/维度/Key/上游 变化自动查询：300ms 防抖
watch([range, currency, dimension, keyId, upstreamId], () => {
  if (autoTimer) clearTimeout(autoTimer)
  autoTimer = setTimeout(loadData, 300)
})

onMounted(() => {
  loadFilterOptions()
  loadData()
})

function fmtBucket(bucket: string): string {
  const d = dayjs(bucket)
  if (!d.isValid()) return bucket
  return hourMode.value ? d.format('HH:mm') : d.format('MM-DD')
}

function fmtBucketFull(bucket: string): string {
  const d = dayjs(bucket)
  if (!d.isValid()) return bucket
  return hourMode.value ? d.format('MM-DD HH:mm') : d.format('YYYY-MM-DD')
}

function keyLabel(k: ApiKeyRow): string {
  return `${k.name || '(未命名)'} · ${k.prefix}`
}

const keyNameMap = computed(() => new Map(keys.value.map((k) => [k.id, k.name || k.prefix])))
const upstreamNameMap = computed(() => new Map(upstreams.value.map((u) => [u.id, u.name])))

/** 维度原始值 → 展示名：协议映射常量表；key/upstream 的 UUID 解析为名称，未知保留原值 */
function dimLabel(d: string): string {
  if (!d) return '—'
  if (dimension.value === 'protocol') return PROTOCOL_IN_LABELS[d] ?? d
  if (dimension.value === 'key') return keyNameMap.value.get(d) ?? d
  if (dimension.value === 'upstream') return upstreamNameMap.value.get(d) ?? d
  return d
}

function truncateLabel(s: string, max = 16): string {
  return s.length > max ? `${s.slice(0, max)}…` : s
}

// 仪表/图表标题
const chartTitle = computed(() => {
  const opt = DIMENSION_OPTIONS.find((o) => o.value === dimension.value)
  return `趋势 · ${opt?.label ?? '总量'}`
})

// —— 维度明细表（中文数值感知排序：模型名等含数字时稳定） ——
const detailCollator = new Intl.Collator('zh-Hans-CN', { numeric: true })

const detailRows = computed(() =>
  sortedPoints.value
    .map((p) => ({
      ...p,
      bucketText: fmtBucketFull(p.bucket),
      dimLabel: dimLabel(p.dimension),
    }))
    .sort((a, b) => detailCollator.compare(a.dimLabel, b.dimLabel) || a.bucket.localeCompare(b.bucket)),
)

// —— 趋势图 ——
const metricName = computed(() => {
  if (metric.value === 'tokens') return 'Token 数'
  if (metric.value === 'cost') return `成本(${currency.value})`
  return '请求数'
})

/** 当前指标在维度分组下的数值（请求/Token/成本），null 表示无值 */
function metricValue(p: SeriesPoint): number | null {
  if (metric.value === 'tokens') return (p.prompt_tokens ?? 0) + (p.completion_tokens ?? 0)
  if (metric.value === 'cost') return p.cost_display != null ? Number(p.cost_display) : null
  return p.requests
}

/** 指标值格式化：成本用货币千分位，计数用千分位（tooltip / 图例共用） */
function fmtMetricValue(v: unknown): string {
  if (v === null || v === undefined || v === '') return '-'
  const n = Number(v)
  if (!Number.isFinite(n)) return String(v)
  return metric.value === 'cost' ? fmtMoney(n) : fmtInt(n)
}

/** 坐标轴数字：大数缩写 + 千分位；成本保持货币格式 */
function fmtAxisValue(n: number): string {
  if (metric.value === 'cost') return fmtMoney(n)
  const abs = Math.abs(n)
  if (abs >= 1e8) return `${(n / 1e8).toFixed(1)}亿`
  if (abs >= 1e4) return `${(n / 1e4).toFixed(1)}万`
  return n.toLocaleString('zh-CN')
}

interface TooltipParam {
  axisValueLabel?: string | number
  name?: string
  marker?: string
  seriesName?: string
  value?: unknown
}

/** 自定义 tooltip：请求数/Token 千分位、成本货币格式 */
function axisTooltip(params: unknown): string {
  const list = (Array.isArray(params) ? params : [params]) as TooltipParam[]
  const head = list.find((p) => p.axisValueLabel !== undefined)?.axisValueLabel
  const title = head !== undefined && head !== null ? String(head) : ''
  const rows: string[] = []
  for (const it of list) {
    const v = it.value
    if (v === null || v === undefined) continue
    const marker = it.marker ?? ''
    const name = it.seriesName ?? ''
    rows.push(`${marker}${name}<span style="float:right;margin-left:16px">${fmtMetricValue(v)}</span>`)
  }
  const body = rows.join('<br/>')
  if (!body) return title
  return title ? `${title}<br/>${body}` : body
}

const grid = { left: 10, right: 24, top: 40, bottom: 10, containLabel: true }

const chartOption = computed<EChartsOption>(() => {
  const pts = sortedPoints.value
  const labels = pts.map((p) => fmtBucket(p.bucket))

  // 单指标语义：每个 metric 只画自身序列；单轴即可，双轴仅未来出现双单位序列时才引入。
  const axisLabel = {
    formatter: (n: number) => fmtAxisValue(n),
  }

  if (!dimension.value) {
    let seriesName: string
    let seriesType: 'bar' | 'line'
    let data: (number | null)[]
    if (metric.value === 'requests') {
      seriesName = '请求数'
      seriesType = 'bar'
      data = pts.map((p) => p.requests)
    } else if (metric.value === 'tokens') {
      seriesName = '总 Token 数'
      seriesType = 'line'
      data = pts.map((p) => (p.prompt_tokens ?? 0) + (p.completion_tokens ?? 0))
    } else {
      seriesName = `成本(${currency.value})`
      seriesType = 'line'
      data = pts.map((p) => (p.cost_display != null ? Number(p.cost_display) : null))
    }
    return {
      tooltip: { trigger: 'axis', formatter: axisTooltip },
      legend: { data: [seriesName], top: 8 },
      grid,
      xAxis: { type: 'category', boundaryGap: seriesType === 'bar', data: labels },
      yAxis: { type: 'value', axisLabel },
      series: [
        seriesType === 'bar'
          ? { name: seriesName, type: 'bar', barMaxWidth: 48, data }
          : { name: seriesName, type: 'line', smooth: true, connectNulls: true, data },
      ],
    }
  }

  // 分组：按维度聚合，取请求数 Top 8，当前指标单独成线
  // 维度分组返回的是 (bucket × dimension) 逐行展开：labels 必须先按 bucket 去重，
  // 否则同一时间点渲染 N 次且各线数据错位（发布审阅前端 M1）。
  const buckets = [...new Set(pts.map((p) => p.bucket))]
  const dimLabels = buckets.map(fmtBucket)

  const totals = new Map<string, number>()
  for (const p of pts) totals.set(p.dimension, (totals.get(p.dimension) ?? 0) + p.requests)
  const topDims = [...totals.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 8)
    .map(([raw]) => ({ raw, label: dimLabel(raw) }))

  const series = topDims.map(({ raw, label }) => {
    const byBuck = new Map<string, number | null>()
    for (const p of pts) {
      if (p.dimension !== raw) continue
      byBuck.set(p.bucket, metricValue(p))
    }
    return {
      name: label,
      type: 'line' as const,
      smooth: true,
      connectNulls: true,
      data: buckets.map((b) => byBuck.get(b) ?? null),
    }
  })

  return {
    tooltip: { trigger: 'axis', formatter: axisTooltip },
    legend: {
      data: topDims.map((d) => d.label),
      formatter: (name: string) => truncateLabel(name),
      top: 8,
    },
    grid,
    xAxis: { type: 'category', boundaryGap: false, data: dimLabels },
    yAxis: { type: 'value', name: metricName.value, axisLabel },
    series,
  }
})

// —— CSV 导出 ——
function csvCell(v: unknown): string {
  if (v === null || v === undefined) return '""'
  let s = String(v)
  // 公式注入中和（CWE-1236）：= + - @ 及制表符开头前置单引号
  if (/^[=+\-@\t\r]/.test(s)) s = `'${s}`
  return `"${s.replace(/"/g, '""')}"`
}

/** 导出文件名：stats_起止日期_币种.csv */
function statsCsvFilename(): string {
  const fmt = 'YYYYMMDD'
  const f = range.value ? dayjs(range.value[0]).format(fmt) : dayjs().format(fmt)
  const t = range.value ? dayjs(range.value[1]).format(fmt) : dayjs().format(fmt)
  return `stats_${f}_${t}_${currency.value}.csv`
}

async function exportCsv() {
  exporting.value = true
  try {
    const header = ['bucket', 'dimension', 'requests', 'errors', 'prompt_tokens', 'completion_tokens', 'cost_display']
    const lines = ['# nextapi stats', header.map(csvCell).join(',')]
    for (const p of sortedPoints.value) {
      lines.push(
        [p.bucket, dimLabel(p.dimension), p.requests, p.errors, p.prompt_tokens ?? '', p.completion_tokens ?? '', p.cost_display ?? '']
          .map(csvCell)
          .join(','),
      )
    }
    const blob = new Blob(['\ufeff' + lines.join('\n')], { type: 'text/csv;charset=utf-8' })
    const filename = statsCsvFilename()
    downloadBlob(blob, filename)
    ElMessage.success(`已导出 ${filename}`)
  } catch (e) {
    ElMessage.error(`导出失败：${errMsg(e)}`)
  } finally {
    exporting.value = false
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
  margin-left: auto;
}

.granularity-hint {
  margin-top: 12px;
}

.metric-switch {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
}
</style>
