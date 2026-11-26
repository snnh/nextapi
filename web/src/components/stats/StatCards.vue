<template>
  <!-- 核心指标：只保留请求数/成功率/Token/成本四张同权重卡片 -->
  <div class="stat-grid">
    <el-card v-for="m in cards" :key="m.key" class="stat-card" shadow="never">
      <div class="stat-label">
        <el-tooltip :content="m.tip" placement="top">
          <span>{{ m.label }}</span>
        </el-tooltip>
      </div>

      <!-- 首屏加载：用骨架占位代替整块遮罩，避免布局跳动 -->
      <template v-if="showSkeleton">
        <SkeletonBlock width="64%" height="30" radius="6px" />
        <div class="stat-sub"><SkeletonBlock width="82%" height="12" /></div>
        <div class="stat-spark is-placeholder"><SkeletonBlock height="30" /></div>
      </template>

      <template v-else>
        <div class="stat-value">{{ m.value }}</div>
        <div class="stat-sub hint">{{ m.sub }}</div>
        <div v-if="m.spark" class="stat-spark" :style="{ color: m.color }">
          <svg
            class="stat-spark-svg"
            viewBox="0 0 100 30"
            preserveAspectRatio="none"
            aria-hidden="true"
          >
            <path :d="m.spark.area" class="spark-area" />
            <path :d="m.spark.line" class="spark-line" vector-effect="non-scaling-stroke" />
          </svg>
        </div>
      </template>
    </el-card>
  </div>

  <!-- 缓存与延迟：二级信息区，不单独占用指标卡 -->
  <div class="stat-secondary" aria-label="缓存与延迟指标">
    <div class="secondary-item">
      <span class="secondary-label">缓存命中 Token</span>
      <span class="secondary-value">{{ fmtTokens(summary?.cache_read_tokens) }}</span>
      <span class="secondary-sub">命中率 {{ fmtPctOrNull(summary?.cache_hit_rate) }}</span>
    </div>
    <div class="secondary-item secondary-latency">
      <span class="secondary-label">延迟</span>
      <span class="secondary-value latency-values">
        <span>P50 <b>{{ fmtDur(summary?.p50_ms) }}</b></span>
        <span>P95 <b>{{ fmtDur(summary?.p95_ms) }}</b></span>
        <span>P99 <b>{{ fmtDur(summary?.p99_ms) }}</b></span>
      </span>
    </div>
  </div>
</template>

<script setup lang="ts">
// 统计汇总：四张核心指标卡（含 24 桶迷你趋势）+ 缓存/延迟二级信息区（仪表盘与统计页共用）
import { computed } from 'vue'
import type { SeriesPoint, StatsCurrency, StatsSummary } from '@/api/types'
import { fmtDur, fmtInt, fmtMoney, fmtPct, fmtTokens } from '@/utils/format'
import { STAT_LABEL_TIPS } from '@/utils/consts'
import SkeletonBlock from '@/components/common/SkeletonBlock.vue'

const props = withDefaults(
  defineProps<{
    summary: StatsSummary | null
    currency: StatsCurrency
    /** 趋势桶（与上方图表同一份 series），用于卡片底部迷你折线 */
    points?: SeriesPoint[]
    /** 首屏加载中：仅在完全无数据时显示骨架，避免刷新时闪白 */
    loading?: boolean
  }>(),
  { points: () => [], loading: false },
)

/** 迷你趋势取最近 24 个桶：既保留形状，又不至于在窄卡里糊成一片 */
const SPARK_BUCKETS = 24
const SPARK_W = 100
const SPARK_H = 30

type SparkPath = { line: string; area: string }

interface CardItem {
  key: string
  label: string
  value: string
  sub: string
  tip: string
  color: string
  spark: SparkPath | null
}

/** Token 构成子标题（无数据时省略，避免全 0 噪声） */
function tokenSub(s: StatsSummary | null): string {
  if (!s) return ''
  const parts = [`输入 ${fmtTokens(s.prompt_tokens)}`, `输出 ${fmtTokens(s.completion_tokens)}`]
  if (s.cache_read_tokens > 0) parts.push(`缓存读 ${fmtTokens(s.cache_read_tokens)}`)
  if (s.cache_write_tokens > 0) parts.push(`缓存写 ${fmtTokens(s.cache_write_tokens)}`)
  return parts.join(' · ')
}

/** 命中率：分母为 0（后端 null）→ '-'，不误报 0% */
function fmtPctOrNull(v: number | null | undefined): string {
  return v === null || v === undefined ? '-' : fmtPct(v)
}

/**
 * 把 series 按 bucket 折叠成单条序列。
 * dimension 非空时同一 bucket 会有多行（每条模型/Key 一行），这里必须求和后再取形状。
 */
const sparkValues = computed(() => {
  const map = new Map<string, { req: number; err: number; tokens: number; cost: number }>()
  for (const p of props.points) {
    const cur = map.get(p.bucket) ?? { req: 0, err: 0, tokens: 0, cost: 0 }
    cur.req += p.requests ?? 0
    cur.err += p.errors ?? 0
    cur.tokens +=
      (p.prompt_tokens ?? 0) +
      (p.completion_tokens ?? 0) +
      (p.cache_read_tokens ?? 0) +
      (p.cache_write_tokens ?? 0)
    cur.cost += Number(p.cost_display ?? 0) || 0
    map.set(p.bucket, cur)
  }
  const ordered = [...map.entries()]
    .sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0))
    .slice(-SPARK_BUCKETS)
    .map(([, v]) => v)

  return {
    requests: ordered.map((v) => v.req),
    // 成功率按桶算百分比：桶内无请求时记 100（平线），不制造假下跌
    success: ordered.map((v) => (v.req > 0 ? ((v.req - v.err) / v.req) * 100 : 100)),
    tokens: ordered.map((v) => v.tokens),
    cost: ordered.map((v) => v.cost),
  }
})

/** 数值序列 → SVG 折线 + 填充路径；不足两点或全 0 时不出图 */
function buildSpark(values: number[]): SparkPath | null {
  const vals = values.filter((v) => Number.isFinite(v))
  if (vals.length < 2) return null
  const max = Math.max(...vals)
  const min = Math.min(...vals)
  if (max === 0 && min === 0) return null
  const span = max - min || 1
  const stepX = SPARK_W / (vals.length - 1)
  const pts = vals.map((v, i) => {
    const y = SPARK_H - 3 - ((v - min) / span) * (SPARK_H - 6)
    return `${(i * stepX).toFixed(2)},${y.toFixed(2)}`
  })
  const line = pts.map((p, i) => `${i === 0 ? 'M' : 'L'}${p}`).join(' ')
  return { line, area: `${line} L${SPARK_W},${SPARK_H} L0,${SPARK_H} Z` }
}

const showSkeleton = computed(() => props.loading && !props.summary)

const cards = computed<CardItem[]>(() => {
  const s = props.summary
  const v = sparkValues.value
  return [
    {
      key: 'requests',
      label: '请求数',
      value: fmtInt(s?.requests),
      sub: '',
      tip: STAT_LABEL_TIPS.requests,
      color: 'var(--nx-chart-1)',
      spark: buildSpark(v.requests),
    },
    {
      key: 'success',
      label: '成功率',
      value: fmtPct(s?.success_rate),
      sub: '',
      tip: STAT_LABEL_TIPS.success,
      color: 'var(--nx-chart-7)',
      spark: buildSpark(v.success),
    },
    {
      key: 'tokens',
      label: '总 Token（含缓存）',
      value: fmtTokens(s?.total_tokens),
      sub: tokenSub(s),
      tip: STAT_LABEL_TIPS.tokens,
      color: 'var(--nx-chart-3)',
      spark: buildSpark(v.tokens),
    },
    {
      key: 'cost',
      label: `成本（${props.currency}）`,
      value: fmtMoney(s?.cost_display),
      sub: s && s.cost_na_count > 0 ? `${s.cost_na_count} 条未计价` : '',
      tip: STAT_LABEL_TIPS.cost,
      color: 'var(--nx-chart-2)',
      spark: buildSpark(v.cost),
    },
  ]
})
</script>

<style scoped>
/* 四张核心卡：宽屏一行，窄屏两列 */
.stat-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 16px;
  margin-bottom: 12px;
}

.stat-card {
  min-width: 0; /* 允许内部省略号收缩 */
  box-sizing: border-box;
}

.stat-label {
  font-size: var(--nx-font-small);
  color: var(--el-text-color-secondary);
  margin-bottom: 8px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.stat-value {
  font-size: 28px;
  font-weight: 600;
  color: var(--el-text-color-primary);
  line-height: 1.2;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.stat-sub {
  margin-top: 6px;
  min-height: 18px;
}

/* 迷你趋势：贴在卡片底部，颜色由卡片语义色控制 */
.stat-spark {
  margin-top: 8px;
  height: 30px;
}

.stat-spark.is-placeholder {
  display: flex;
  align-items: center;
}

.stat-spark-svg {
  display: block;
  width: 100%;
  height: 30px;
  overflow: visible;
}

.spark-line {
  fill: none;
  stroke: currentColor;
  stroke-width: 1.5;
  stroke-linecap: round;
  stroke-linejoin: round;
  opacity: 0.9;
}

.spark-area {
  fill: currentColor;
  opacity: 0.12;
  stroke: none;
}

/* 缓存与延迟：单行信息条，减少三张同权重卡片 */
.stat-secondary {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px 28px;
  padding: 10px 16px;
  margin-bottom: 16px;
  border: 1px solid var(--nx-border);
  border-radius: var(--nx-radius-md);
  background: var(--nx-surface);
  font-size: var(--nx-font-small);
  line-height: 1.6;
}

.secondary-item {
  display: flex;
  align-items: baseline;
  flex-wrap: wrap;
  gap: 4px 8px;
  min-width: 0;
}

.secondary-label {
  color: var(--nx-text-3);
}

.secondary-value {
  color: var(--nx-text);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
}

.secondary-sub {
  color: var(--nx-text-3);
}

.latency-values {
  display: inline-flex;
  flex-wrap: wrap;
  gap: 4px 14px;
  font-weight: 400;
  color: var(--nx-text-2);
}

.latency-values b {
  color: var(--nx-text);
  font-weight: 600;
}

@media (max-width: 1100px) {
  .stat-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}

@media (max-width: 800px) {
  .stat-grid {
    gap: 12px;
  }
  .stat-value {
    font-size: var(--nx-font-stat);
  }
  .stat-secondary {
    align-items: flex-start;
    flex-direction: column;
    gap: 6px;
  }
}
</style>