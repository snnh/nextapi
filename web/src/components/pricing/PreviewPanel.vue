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
      <el-table-column prop="quantity" label="数量" min-width="100" align="right" />
      <el-table-column label="单价" min-width="140" align="right">
        <template #default="{ row }">{{ fmtMoney(row.price) }} {{ priceDenom(row.unit) }}</template>
      </el-table-column>
      <el-table-column label="成本" min-width="120" align="right">
        <template #default="{ row }">{{ fmtMoney(row.cost) }}</template>
      </el-table-column>
      <el-table-column label="命中分段" min-width="140">
        <template #default="{ row }">
          <span v-if="row.matched_segment">{{ row.matched_segment }}</span>
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
</template>

<script setup lang="ts">
import { UNIT_LABEL } from '@/utils/consts'
import { fmtMoney } from '@/utils/format'
import type { PreviewResp, PriceUnit } from '@/api/types'

defineProps<{ resp: PreviewResp | null }>()

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

function jsonOf(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2)
  } catch {
    return String(v)
  }
}
</script>

<style scoped>
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
