<template>
  <el-dialog
    v-model="visible"
    :title="`额度 · ${row?.name ?? ''}`"
    width="560px"
    :close-on-click-modal="false"
    destroy-on-close
    @open="load(false)"
  >
    <div v-if="!isSupported" class="hint">
      该渠道不支持额度探测（当前支持 Codex OAuth 与 Devin 渠道）。
    </div>
    <template v-else>
      <div class="toolbar">
        <el-button
          size="small"
          type="primary"
          :loading="probing"
          @click="load(true)"
        >
          立即探测
        </el-button>
        <span class="hint">
          {{
            row?.kind === 'codex'
              ? '探测会发一次极小请求以获取上游额度响应头（5 分钟内已有快照则直接复用）。'
              : '探测读取 Devin GetUserStatus 的额度块（不消耗额度）。'
          }}
        </span>
      </div>

      <el-alert
        v-if="error"
        type="warning"
        :closable="false"
        show-icon
        class="alert"
        :title="error"
      />

      <template v-if="quota">
        <div class="meta">
          <span>来源：{{ sourceLabel }}</span>
          <span>更新：{{ capturedAt }}</span>
        </div>

        <!-- Codex：ChatGPT 订阅额度（5h / 周窗口 + 积分） -->
        <template v-if="row?.kind === 'codex'">
          <el-descriptions :column="2" border size="small">
            <el-descriptions-item label="套餐">{{ quota.plan_type || '—' }}</el-descriptions-item>
            <el-descriptions-item label="当前限制级别">
              {{ quota.active_limit || '—' }}
            </el-descriptions-item>
            <el-descriptions-item label="主窗口已用">
              {{ fmtPct(quota.primary?.used_percent) }}
            </el-descriptions-item>
            <el-descriptions-item label="主窗口长度">
              {{ fmtWindow(quota.primary?.window_minutes) }}
            </el-descriptions-item>
            <el-descriptions-item label="主窗口重置">
              {{ fmtReset(quota.primary?.reset_at, quota.primary?.reset_after_seconds) }}
            </el-descriptions-item>
            <el-descriptions-item label="跨窗口限制">
              {{ fmtPct(quota.primary_over_secondary_limit_percent) }}
            </el-descriptions-item>
            <el-descriptions-item label="次窗口已用">
              {{ fmtPct(quota.secondary?.used_percent) }}
            </el-descriptions-item>
            <el-descriptions-item label="次窗口长度">
              {{ fmtWindow(quota.secondary?.window_minutes) }}
            </el-descriptions-item>
            <el-descriptions-item label="次窗口重置" :span="2">
              {{ fmtReset(quota.secondary?.reset_at, quota.secondary?.reset_after_seconds) }}
            </el-descriptions-item>
            <el-descriptions-item label="积分余额">
              {{ quota.credits?.balance ?? '—' }}
            </el-descriptions-item>
            <el-descriptions-item label="积分状态">
              {{ creditsLabel }}
            </el-descriptions-item>
          </el-descriptions>
        </template>

        <!-- Devin：日/周额度 + 百分数 + 重置时刻 -->
        <template v-else>
          <el-descriptions :column="2" border size="small">
            <el-descriptions-item label="套餐">{{ quota.plan || '—' }}</el-descriptions-item>
            <el-descriptions-item label="账号">{{ quota.email || '—' }}</el-descriptions-item>
            <el-descriptions-item label="日额度">{{ fmtInt(quota.daily_quota) }}</el-descriptions-item>
            <el-descriptions-item label="周额度">{{ fmtInt(quota.weekly_quota) }}</el-descriptions-item>
            <el-descriptions-item label="额度余量">
              {{ quota.percent_value ?? '—' }} / {{ quota.percent_total ?? '—' }}
            </el-descriptions-item>
            <el-descriptions-item label="可用模型">
              {{ fmtInt(quota.available_models) }}
            </el-descriptions-item>
            <el-descriptions-item label="每日重置">
              {{ fmtReset(quota.daily_reset_at) }}
            </el-descriptions-item>
            <el-descriptions-item label="每周重置">
              {{ fmtReset(quota.weekly_reset_at) }}
            </el-descriptions-item>
          </el-descriptions>
          <div class="hint footnote">
            额度以「日 + 周」双窗口计（官方文档：日额度大于周额度的 1/7）；上游未返回额度块时仅显示套餐与模型数。
          </div>
        </template>
      </template>
      <el-empty
        v-else-if="!probing"
        :image-size="70"
        description="暂无额度快照；点击「立即探测」，或等真实流量自动抓取"
      />
    </template>

    <template #footer>
      <el-button @click="visible = false">关闭</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import { fmtInt, fmtPct } from '@/utils/format'
import type { QuotaResp, UpstreamOut } from '@/api/types'

const props = defineProps<{
  modelValue: boolean
  row: UpstreamOut | null
}>()

const emit = defineEmits<{ 'update:modelValue': [boolean] }>()

const visible = computed({
  get: () => props.modelValue,
  set: (v: boolean) => emit('update:modelValue', v),
})

const quota = ref<QuotaResp['quota'] | null>(null)
const probing = ref(false)
const error = ref('')

const isSupported = computed(() => props.row?.kind === 'codex' || props.row?.kind === 'devin')

const sourceLabel = computed(() => {
  const s = (quota.value as { source?: string } | null)?.source
  if (s === 'live') return '真实流量自动抓取'
  if (s === 'probe') return '主动探测'
  return '上游返回'
})

const capturedAt = computed(() => {
  const ts = (quota.value as { captured_at?: number } | null)?.captured_at
  if (!ts) return '—'
  return new Date(ts * 1000).toLocaleString()
})

const creditsLabel = computed(() => {
  const c = quota.value?.credits
  if (!c) return '—'
  if (c.unlimited) return '无限'
  return c.has_credits ? '有积分' : '无积分'
})

/** 窗口长度：分钟 → 人读（<120 分钟按分钟，否则按小时/天） */
function fmtWindow(minutes?: number | null): string {
  if (minutes == null) return '—'
  if (minutes <= 0) return '—'
  if (minutes < 120) return `${minutes} 分钟`
  if (minutes < 2880) return `${Math.round(minutes / 60)} 小时`
  return `${Math.round(minutes / 1440)} 天`
}

/** 重置时间：epoch 秒 → 本地时间（无 epoch 时回落剩余秒数） */
function fmtReset(at?: number | null, afterSeconds?: number | null): string {
  if (at) return new Date(at * 1000).toLocaleString()
  if (afterSeconds && afterSeconds > 0) {
    const h = Math.floor(afterSeconds / 3600)
    const m = Math.floor((afterSeconds % 3600) / 60)
    return `${h} 小时 ${m} 分钟`
  }
  return '—'
}

/** 读取最近快照；probe=true 时先主动探测 */
async function load(probe: boolean) {
  if (!props.row || !isSupported.value) return
  error.value = ''
  if (probe) probing.value = true
  try {
    const r = probe
      ? await upstreamApi.probeQuota(props.row.id)
      : await upstreamApi.getQuota(props.row.id)
    quota.value = r.quota ?? null
    if (probe) {
      if (r.cached) ElMessage.info('已复用最近 5 分钟内的快照（未再请求上游）')
      else if (r.ok) ElMessage.success('探测完成')
      else if (r.error) error.value = r.error
    }
  } catch (e) {
    error.value = errMsg(e)
  } finally {
    probing.value = false
  }
}
</script>

<style scoped>
.toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 12px;
  flex-wrap: wrap;
}
.hint {
  color: #6b7280;
  font-size: 12px;
  line-height: 1.6;
}
.alert {
  margin-bottom: 12px;
}
.meta {
  display: flex;
  gap: 16px;
  color: #6b7280;
  font-size: 12px;
  margin-bottom: 8px;
}
.footnote {
  margin-top: 8px;
}
</style>
