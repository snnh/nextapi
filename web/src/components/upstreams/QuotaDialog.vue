<template>
  <el-dialog
    v-model="visible"
    :title="`余额/额度 · ${row?.name ?? ''}`"
    class="dlg"
    :close-on-click-modal="false"
    destroy-on-close
    @open="load(false)"
  >
    <div v-if="!isSupported" class="hint">
      该渠道未配置余额/额度查询。可在渠道编辑中启用「余额/额度」，或选择对应适配器。
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
          {{ probeHint }}
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

        <!-- 通用余额/额度（DeepSeek/Kimi/OpenRouter/Zenmux/智谱/OpenAI billing/自定义） -->
        <template v-if="quota.kind === 'balance'">
          <div v-if="quota.balances?.length" class="balance-cards">
            <div
              v-for="(b, i) in quota.balances"
              :key="`${b.currency}-${i}`"
              class="balance-card"
            >
              <div class="balance-currency">{{ b.currency }}</div>
              <div class="balance-amount">{{ b.available ?? '—' }}</div>
              <div class="balance-sub">
                <span v-if="b.total != null">总额 {{ b.total }}</span>
                <span v-if="b.used != null">已用 {{ b.used }}</span>
              </div>
            </div>
          </div>

          <div v-if="quota.plan" class="plan-line">
            套餐：<span class="mono">{{ quota.plan }}</span>
          </div>

          <div v-if="quota.windows?.length" class="windows">
            <div v-for="w in quota.windows" :key="w.name" class="window-card">
              <div class="window-head">
                <span>{{ w.label || w.name }}</span>
                <span class="mono">{{ fmtUsedPct(w.used_percent) }} 已用</span>
              </div>
              <el-progress
                :percentage="clampPct(w.used_percent)"
                :stroke-width="8"
                :show-text="false"
                :status="clampPct(w.used_percent) >= 90 ? 'exception' : undefined"
              />
              <div class="window-reset">重置：{{ fmtReset(w.reset_at) }}</div>
            </div>
          </div>

          <el-descriptions v-if="quota.details?.length" :column="1" border size="small" class="details">
            <el-descriptions-item
              v-for="(d, i) in quota.details"
              :key="`${d.label}-${i}`"
              :label="d.label"
            >
              {{ d.value }}<span v-if="d.currency" class="mono"> {{ d.currency }}</span>
            </el-descriptions-item>
          </el-descriptions>

          <div v-if="quota.reset_at" class="hint footnote">
            上游返回的重置时间：{{ fmtReset(quota.reset_at) }}
          </div>
          <el-empty
            v-if="!quota.balances?.length && !quota.windows?.length && !quota.details?.length"
            :image-size="70"
            description="上游未返回可展示的余额字段"
          />
        </template>

        <!-- Codex：ChatGPT 订阅额度（5h / 周窗口 + 积分） -->
        <template v-else-if="row?.kind === 'codex'">
          <el-descriptions :column="2" border size="small">
            <el-descriptions-item label="套餐">{{ quota.plan_type || '—' }}</el-descriptions-item>
            <el-descriptions-item label="当前限制级别">
              {{ quota.active_limit || '—' }}
            </el-descriptions-item>
            <el-descriptions-item label="主窗口已用">
              {{ fmtUsedPct(quota.primary?.used_percent) }}
            </el-descriptions-item>
            <el-descriptions-item label="主窗口长度">
              {{ fmtWindow(quota.primary?.window_minutes) }}
            </el-descriptions-item>
            <el-descriptions-item label="主窗口重置">
              {{ fmtReset(quota.primary?.reset_at, quota.primary?.reset_after_seconds) }}
            </el-descriptions-item>
            <el-descriptions-item label="跨窗口限制">
              {{ fmtUsedPct(quota.primary_over_secondary_limit_percent) }}
            </el-descriptions-item>
            <el-descriptions-item label="次窗口已用">
              {{ fmtUsedPct(quota.secondary?.used_percent) }}
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
        :description="
          row?.kind === 'codex'
            ? '暂无额度快照；点击「立即探测」，或等真实流量自动抓取'
            : '暂无余额/额度快照；点击「立即探测」'
        "
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
import { fmtInt } from '@/utils/format'
import type { QuotaResp, UpstreamOut } from '@/api/types'

const props = defineProps<{
  modelValue: boolean
  row: UpstreamOut | null
}>()

const emit = defineEmits<{ 'update:modelValue': [boolean]; probed: [] }>()

const visible = computed({
  get: () => props.modelValue,
  set: (v: boolean) => emit('update:modelValue', v),
})

const quota = ref<QuotaResp['quota'] | null>(null)
const probing = ref(false)
const error = ref('')

const isSupported = computed(
  () =>
    props.row?.balance_supported === true ||
    props.row?.kind === 'codex' ||
    props.row?.kind === 'devin',
)

const adapter = computed(() => props.row?.balance_adapter ?? props.row?.kind ?? '')

const probeHint = computed(() => {
  if (props.row?.kind === 'codex') {
    return '探测会发一次极小请求以获取上游额度响应头；「立即探测」强制刷新，不受 5 分钟缓存限制。'
  }
  if (props.row?.kind === 'devin') {
    return '探测读取 Devin GetUserStatus 的额度块（不消耗额度）；「立即探测」强制刷新。'
  }
  switch (adapter.value) {
    case 'openrouter':
    case 'zenmux':
      return '这两个渠道需要 Management Key；普通推理 Key 会返回 403。'
    case 'zhipu':
      return '智谱额度接口为实验性契约（GLM Coding Plan）；失败只提示，不影响渠道。'
    case 'openai_billing':
      return '按 OpenAI billing 兼容语义读取：hard_limit_usd 为美元，total_usage 按分。'
    case 'custom':
      return '读取自定义余额端点；相对路径走渠道同源，绝对地址仅允许公网 HTTPS。'
    default:
      return '「立即探测」会强制实时查询；不点则读取最近一次快照。'
  }
})

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

function fmtUsedPct(v?: number | null): string {
  if (v == null) return '—'
  return `${Number(v).toFixed(1)}%`
}

function clampPct(v?: number | null): number {
  return Math.max(0, Math.min(100, Number(v ?? 0)))
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
      ? await upstreamApi.probeQuota(props.row.id, true)
      : await upstreamApi.getQuota(props.row.id)
    quota.value = r.quota ?? null
    if (probe) {
      if (r.cached) ElMessage.info('已复用缓存快照')
      else if (r.ok) {
        ElMessage.success('探测完成')
        emit('probed')
      } else if (r.error) error.value = r.error
    }
  } catch (e) {
    error.value = errMsg(e)
  } finally {
    probing.value = false
  }
}
</script>

<style scoped>
.alert {
  margin-bottom: 12px;
}
.meta {
  display: flex;
  gap: 16px;
  color: var(--el-text-color-secondary);
  font-size: var(--nx-font-caption);
  margin-bottom: 8px;
}
.footnote {
  margin-top: 8px;
}
.balance-cards {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  margin-bottom: 12px;
}
.balance-card {
  flex: 1 1 160px;
  border: 1px solid var(--el-border-color-lighter);
  border-radius: var(--el-border-radius-base);
  padding: 12px 14px;
  background: var(--el-fill-color-lighter);
}
.balance-currency {
  font-size: var(--nx-font-caption);
  color: var(--el-text-color-secondary);
}
.balance-amount {
  font-size: 24px;
  font-weight: 600;
  line-height: 1.25;
  margin: 2px 0 4px;
  word-break: break-all;
}
.balance-sub {
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
  font-size: var(--nx-font-caption);
  color: var(--el-text-color-secondary);
}
.plan-line {
  margin-bottom: 10px;
}
.windows {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 10px;
  margin-bottom: 12px;
}
.window-card {
  border: 1px solid var(--el-border-color-lighter);
  border-radius: var(--el-border-radius-base);
  padding: 10px 12px;
}
.window-head {
  display: flex;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 8px;
  font-size: var(--nx-font-body);
}
.window-reset {
  margin-top: 6px;
  font-size: var(--nx-font-caption);
  color: var(--el-text-color-secondary);
}
.details {
  margin-top: 4px;
}
</style>
