<template>
  <div v-if="resp" class="preview-panel">
    <el-alert
      v-if="!resp.priced"
      type="warning"
      :closable="false"
      show-icon
      title="该模型未设置价格：本次请求不计价（成本记 NULL 并提示未定价）"
      style="margin-bottom: 14px"
    />

    <el-table :data="resp.lines" border size="small" empty-text="无可计价行">
      <el-table-column label="计价单位" min-width="180">
        <template #default="{ row }">{{ UNIT_LABEL(row.unit) }}</template>
      </el-table-column>
      <el-table-column label="数量" min-width="110" align="right">
        <template #default="{ row }">{{ fmtInt(Number(row.quantity)) }}</template>
      </el-table-column>
      <el-table-column label="单价" min-width="140" align="right">
        <template #default="{ row }">{{ fmtMoney(row.price) }} {{ priceDenom(row.unit) }}</template>
      </el-table-column>
      <el-table-column label="成本" min-width="120" align="right">
        <template #default="{ row }">{{ fmtMoney(row.cost) }}</template>
      </el-table-column>
      <el-table-column label="命中分段" min-width="160">
        <template #default="{ row }">
          <el-tooltip v-if="segDetailLines(row)" placement="top">
            <template #content>
              <div v-for="(l, i) in segDetailLines(row)" :key="i" class="seg-tip">{{ l }}</div>
            </template>
            <span>{{ row.matched_segment }}</span>
          </el-tooltip>
          <span v-else-if="row.matched_segment">{{ row.matched_segment }}</span>
          <span v-else class="hint">无（base_price）</span>
        </template>
      </el-table-column>
    </el-table>

    <div class="total-card">
      <div class="total-item">
        <span class="total-label">合计（CNY）</span>
        <span class="total-amount">{{ fmtMoney(resp.cost_cny) }}</span>
      </div>
      <div class="total-item">
        <span class="total-label">合计（USD）</span>
        <span class="total-amount">{{ fmtMoney(resp.cost_usd) }}</span>
      </div>
    </div>

    <el-collapse style="margin-top: 12px">
      <el-collapse-item name="price_used" title="price_used（命中的价格规则明细）">
        <pre class="mono json-pre">{{ jsonOf(resp.price_used) }}</pre>
      </el-collapse-item>
      <el-collapse-item name="fx_snapshot" title="fx_snapshot（汇率快照）">
        <pre class="mono json-pre">{{ jsonOf(resp.fx_snapshot) }}</pre>
      </el-collapse-item>
    </el-collapse>
  </div>
  <!-- 无结果时引导空态 -->
  <el-empty
    v-else
    class="preview-empty"
    description="填写左侧用量后点击试算"
    :image-size="80"
  />
</template>

<script setup lang="ts">
import { UNIT_LABEL } from '@/utils/consts'
import { fmtInt, fmtMoney } from '@/utils/format'
import type { PreviewLine, PreviewResp, PriceUnit } from '@/api/types'

const props = defineProps<{ resp: PreviewResp | null }>()

function priceDenom(unit: PriceUnit): string {
  switch (unit) {
    case 'token_in':
    case 'token_out':
    case 'token_cache_write':
    case 'token_cache_read':
      return '/1M'
    case 'image':
      return '/张'
    case 'video_second':
      return '/秒'
    default:
      return ''
  }
}

/**
 * 命中分段的条件摘要（数据可得时）：后端不返回 weekday/时间窗等条件原文，
 * 只能从 price_used 中按 rule_id 匹配到命中行的 base_price/price 等字段做摘要。
 */
function segDetailLines(row: PreviewLine): string[] | null {
  if (!row.matched_segment) return null
  const used = props.resp?.price_used ?? []
  const hit = used.find((v) => {
    if (!v || typeof v !== 'object') return false
    const o = v as Record<string, unknown>
    return o.rule_id === row.rule_id && o.matched_segment === row.matched_segment
  })
  if (!hit || typeof hit !== 'object') return null
  const o = hit as Record<string, unknown>
  const lines: string[] = []
  lines.push(`命中分段：${row.matched_segment}`)
  if (o.price !== undefined && o.price !== null) {
    lines.push(`分段单价：${fmtMoney(String(o.price))} ${row.currency}`)
  }
  if (o.base_price !== undefined && o.base_price !== null) {
    lines.push(`基础单价：${fmtMoney(String(o.base_price))} ${row.currency}`)
  }
  if (typeof o.rule_id === 'string') lines.push(`规则 ID：${o.rule_id}`)
  return lines.length > 1 ? lines : null
}

function jsonOf(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2)
  } catch {
    return String(v)
  }
}
</script>

<style scoped>
.preview-empty {
  padding: 12px 0 4px;
}
.seg-tip {
  font-size: 12px;
  line-height: 1.6;
  white-space: nowrap;
}
.total-card {
  display: flex;
  gap: 24px;
  margin-top: 14px;
  padding: 12px 16px;
  background: #f5f7fa;
  border-radius: 8px;
}
.total-item {
  display: flex;
  align-items: baseline;
  gap: 8px;
}
.total-label {
  font-size: 12px;
  color: #6b7280;
}
.total-amount {
  font-size: 20px;
  font-weight: 600;
  color: #1f2329;
  font-variant-numeric: tabular-nums;
}
.json-pre {
  max-height: 260px;
  overflow: auto;
  background: #1e1e1e;
  color: #d4d4d4;
  padding: 10px;
  border-radius: 6px;
  font-size: 12px;
}
</style>
