<template>
  <div class="fx-panel" v-loading="loading">
    <el-alert
      type="info"
      :closable="false"
      show-icon
      title="汇率说明"
      style="margin-bottom: 16px"
    >
      <template #default>
        <div class="hint">
          汇率采用「手动优先」策略：同一币对若存在 manual 记录则优先使用手动值；手动缺失时回退 auto 自动汇率。自动汇率超过陈旧阈值（运行参数
          <span class="mono">fx_stale_max_minutes</span>，默认 600 分钟）会被标记为「已过期」，网关将按降级链处理。可在下方手动设定 USD→CNY 与 CNY→USD 汇率。
        </div>
      </template>
    </el-alert>

    <el-card shadow="never" class="fx-manual">
      <template #header>
        <div class="card-head">
          <span>手动设定汇率</span>
          <span class="hint">输入框留空 = 该方向不修改；均需 &gt; 0</span>
        </div>
      </template>
      <div class="manual-row">
        <span class="manual-pair">USD → CNY</span>
        <el-input-number
          v-model="usdCny"
          :min="0"
          :precision="6"
          :controls="false"
          placeholder="US Dollar → RMB"
          style="width: 220px"
        />
      </div>
      <div class="manual-row">
        <span class="manual-pair">CNY → USD</span>
        <el-input-number
          v-model="cnyUsd"
          :min="0"
          :precision="6"
          :controls="false"
          placeholder="RMB → US Dollar"
          style="width: 220px"
        />
      </div>
      <div class="manual-actions">
        <el-button type="primary" :loading="saving" @click="saveManual">保存</el-button>
        <el-button :icon="RefreshRight" :loading="refreshing" @click="refreshAuto">立即刷新自动汇率</el-button>
      </div>
    </el-card>

    <el-card shadow="never" style="margin-top: 16px">
      <template #header>
        <div class="card-head"><span>汇率列表</span></div>
      </template>
      <el-table :data="fxRows" empty-text="暂无汇率数据">
        <el-table-column label="币对" min-width="140">
          <template #default="{ row }">{{ row.currency_from }} → {{ row.currency_to }}</template>
        </el-table-column>
        <el-table-column label="汇率" min-width="140" align="right">
          <template #default="{ row }">
            <span class="mono">{{ fmtMoney(row.rate) }}</span>
          </template>
        </el-table-column>
        <el-table-column label="来源" width="90" align="center">
          <template #default="{ row }">
            <el-tag :type="row.source === 'manual' ? 'primary' : 'success'" size="small" effect="plain">
              {{ row.source === 'manual' ? '手动' : '自动' }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column label="拉取时间" min-width="170">
          <template #default="{ row }">{{ fmtTime(row.fetched_at) }}</template>
        </el-table-column>
        <el-table-column label="状态" width="100" align="center">
          <template #default="{ row }">
            <el-tag v-if="row.stale" type="danger" size="small">已过期</el-tag>
            <el-tag v-else type="success" size="small" effect="plain">有效</el-tag>
          </template>
        </el-table-column>
        <el-table-column label="更新时间" min-width="170">
          <template #default="{ row }">{{ fmtTime(row.updated_at) }}</template>
        </el-table-column>
      </el-table>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { RefreshRight } from '@element-plus/icons-vue'
import { fxApi } from '@/api'
import { errMsg } from '@/api/http'
import { fmtMoney, fmtTime } from '@/utils/format'
import type { FxRow } from '@/api/types'

const loading = ref(false)
const fxRows = ref<FxRow[]>([])
const usdCny = ref<number | null>(null)
const cnyUsd = ref<number | null>(null)
const saving = ref(false)
const refreshing = ref(false)

async function load() {
  loading.value = true
  try {
    fxRows.value = await fxApi.list()
    const usd = fxRows.value.find((r) => r.currency_from === 'USD' && r.currency_to === 'CNY')
    const cny = fxRows.value.find((r) => r.currency_from === 'CNY' && r.currency_to === 'USD')
    usdCny.value = usd ? Number(usd.rate) : null
    cnyUsd.value = cny ? Number(cny.rate) : null
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    loading.value = false
  }
}

onMounted(load)

async function saveManual() {
  const items: { from: string; to: string; rate: number }[] = []
  if (usdCny.value && usdCny.value > 0) {
    items.push({ from: 'USD', to: 'CNY', rate: usdCny.value })
  }
  if (cnyUsd.value && cnyUsd.value > 0) {
    items.push({ from: 'CNY', to: 'USD', rate: cnyUsd.value })
  }
  if (!items.length) {
    ElMessage.warning('请至少填写一个方向（且需大于 0）')
    return
  }
  saving.value = true
  try {
    const r = await fxApi.put(items)
    ElMessage.success(`已保存 ${r.updated} 条`)
    await load()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    saving.value = false
  }
}

async function refreshAuto() {
  refreshing.value = true
  try {
    const r = await fxApi.refresh()
    ElMessage.success(`已更新 ${r.report.updated} 对`)
    if (r.report.pairs.length) {
      ElMessage.info(`涉及币对：${r.report.pairs.join(', ')}`)
    }
    await load()
  } catch (e) {
    // 502 时后端保留旧汇率（联网失败降级），仅提示错误
    ElMessage.error(errMsg(e))
  } finally {
    refreshing.value = false
  }
}
</script>

<style scoped>
.card-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 14px;
  font-weight: 600;
  color: #303133;
}
.manual-row {
  display: flex;
  align-items: center;
  gap: 16px;
  margin-bottom: 12px;
}
.manual-pair {
  font-size: 13px;
  font-weight: 600;
  color: #303133;
  width: 110px;
}
.manual-actions {
  display: flex;
  gap: 12px;
  margin-top: 6px;
}
</style>
