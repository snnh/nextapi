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
      <el-button :icon="Refresh" :loading="loading" @click="loadAll">刷新</el-button>
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
import { Refresh } from '@element-plus/icons-vue'
import { ElMessage } from 'element-plus'
import EChart from '@/components/EChart.vue'
import type { EChartsOption } from 'echarts'
import { statsApi } from '@/api'
import { errMsg } from '@/api/http'
import type { SeriesDimension, SeriesGranularity, SeriesPoint, StatsCurrency, StatsQuery, StatsSummary } from '@/api/types'
import { fmtDur, fmtInt, fmtMoney, fmtPct } from '@/utils/format'

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

onMounted(loadAll)
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
