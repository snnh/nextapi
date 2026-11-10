<template>
  <!-- 核心指标：只保留请求数/成功率/Token/成本四张同权重卡片 -->
  <div class="stat-grid">
    <el-card v-for="m in cards" :key="m.label" class="stat-card" shadow="never">
      <div class="stat-label">
        <el-tooltip :content="m.tip" placement="top">
          <span>{{ m.label }}</span>
        </el-tooltip>
      </div>
      <div class="stat-value">{{ m.value }}</div>
      <div v-if="m.sub" class="stat-sub hint">{{ m.sub }}</div>
    </el-card>
  </div>

  <!-- 缓存与延迟：二级信息区，不单独占用指标卡 -->
  <div class="stat-secondary" aria-label="缓存与延迟指标">
    <div class="secondary-item">
      <span class="secondary-label">缓存命中 Tokens</span>
      <span class="secondary-value">{{ fmtInt(summary?.cache_read_tokens) }}</span>
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
// 统计汇总：四张核心指标卡 + 缓存/延迟二级信息区（仪表盘与统计页共用）
import { computed } from 'vue'
import type { StatsCurrency, StatsSummary } from '@/api/types'
import { fmtDur, fmtInt, fmtMoney, fmtPct } from '@/utils/format'
import { STAT_LABEL_TIPS } from '@/utils/consts'

const props = defineProps<{
  summary: StatsSummary | null
  currency: StatsCurrency
}>()

interface CardItem {
  label: string
  value: string
  sub: string
  tip: string
}

/** Token 构成子标题（无数据时省略，避免全 0 噪声） */
function tokenSub(s: StatsSummary | null): string {
  if (!s) return ''
  const parts = [`输入 ${fmtInt(s.prompt_tokens)}`, `输出 ${fmtInt(s.completion_tokens)}`]
  if (s.cache_read_tokens > 0) parts.push(`缓存读 ${fmtInt(s.cache_read_tokens)}`)
  if (s.cache_write_tokens > 0) parts.push(`缓存写 ${fmtInt(s.cache_write_tokens)}`)
  return parts.join(' · ')
}

/** 命中率：分母为 0（后端 null）→ '-'，不误报 0% */
function fmtPctOrNull(v: number | null | undefined): string {
  return v === null || v === undefined ? '-' : fmtPct(v)
}

const cards = computed<CardItem[]>(() => {
  const s = props.summary
  return [
    { label: '请求数', value: fmtInt(s?.requests), sub: '', tip: STAT_LABEL_TIPS.requests },
    { label: '成功率', value: fmtPct(s?.success_rate), sub: '', tip: STAT_LABEL_TIPS.success },
    {
      label: '总 Tokens（含缓存）',
      value: fmtInt(s?.total_tokens),
      sub: tokenSub(s),
      tip: STAT_LABEL_TIPS.tokens,
    },
    {
      label: `成本（${props.currency}）`,
      value: fmtMoney(s?.cost_display),
      sub: s && s.cost_na_count > 0 ? `${s.cost_na_count} 条未计价` : '',
      tip: STAT_LABEL_TIPS.cost,
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