<template>
  <el-dialog
    v-model="visible"
    title="按模型定价"
    width="920px"
    :close-on-click-modal="false"
    destroy-on-close
    top="4vh"
  >
    <el-form label-width="110px" @submit.prevent>
      <div class="head-grid">
        <el-form-item label="上游" required>
          <el-select v-model="upstreamId" filterable placeholder="请选择上游" style="width: 100%" @change="onPairChange">
            <el-option v-for="u in upstreams" :key="u.id" :value="u.id" :label="u.name" />
          </el-select>
        </el-form-item>
        <el-form-item label="模型 ID" required>
          <el-input v-model="modelId" placeholder="如 gpt-4o / gemini-2.0-flash" clearable @blur="onPairChange" />
        </el-form-item>
        <el-form-item label="币种">
          <el-radio-group v-model="currency">
            <el-radio-button v-for="c in CURRENCIES" :key="c" :value="c">{{ c }}</el-radio-button>
          </el-radio-group>
        </el-form-item>
        <el-form-item label="上下文基准">
          <el-select v-model="contextBasis" style="width: 100%">
            <el-option value="prompt_tokens" label="按 prompt_tokens（仅输入）" />
            <el-option value="total_tokens" label="按 total_tokens（输入+输出）" />
          </el-select>
        </el-form-item>
      </div>

      <!-- 参考建议：一键填充全部单位 -->
      <div v-if="suggestGroups.length" class="suggest-bar">
        <span class="hint">参考定价：</span>
        <el-dropdown v-for="g in suggestGroups" :key="g.upstream_id" trigger="click" @command="(cmd: string) => adoptGroup(g, cmd)">
          <el-button size="small" plain>
            {{ g.upstream_name }}
            <el-tag v-if="g.upstream_id !== upstreamId" size="small" type="warning" effect="plain" style="margin-left: 4px">其他上游</el-tag>
          </el-button>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item v-for="r in g.rules" :key="r.id" :command="r.unit">
                {{ UNIT_LABEL(r.unit) }} · {{ fmtMoney(r.base_price) }} {{ r.currency }}
                <span v-if="r.segments?.length">（{{ r.segments.length }} 段）</span>
              </el-dropdown-item>
              <el-dropdown-item divided command="__all__">采用全部单位</el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
      </div>

      <el-alert
        type="info"
        :closable="false"
        show-icon
        title="同一模型的四种 token 计价在此一次配置：勾选即创建/更新规则，取消勾选即删除对应规则。图片/视频等按维度计价请用「高级规则」。"
        style="margin-bottom: 14px"
      />

      <!-- 四种 token 计价单位卡片 -->
      <div class="unit-grid">
        <div v-for="u in units" :key="u.unit" class="unit-card" :class="{ off: !u.enabled }">
          <div class="unit-head">
            <span class="unit-title">{{ UNIT_LABEL(u.unit) }}</span>
            <el-switch v-model="u.enabled" />
          </div>
          <div v-if="u.enabled" class="unit-body">
            <div class="unit-row">
              <span class="unit-label">单价</span>
              <el-input-number v-model="u.base_price" :min="0" :precision="10" :controls="false" style="width: 200px" placeholder="必填" />
              <span class="hint">每 1M token（{{ currency }}）</span>
            </div>
            <el-collapse class="unit-segs">
              <el-collapse-item name="segs">
                <template #title>
                  <span class="segs-title">分段计价{{ u.segments.length ? `（${u.segments.length} 段）` : '（可选）' }}</span>
                </template>
                <SegmentsEditor v-model="u.segments" :unit="u.unit" />
              </el-collapse-item>
            </el-collapse>
          </div>
          <div v-else class="unit-off hint">不计价</div>
        </div>
      </div>
    </el-form>

    <template #footer>
      <el-button @click="visible = false">取消</el-button>
      <el-button type="primary" :loading="saving" @click="submit">保存</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { reactive, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { pricingApi } from '@/api'
import { errMsg } from '@/api/http'
import { CURRENCIES, UNIT_LABEL } from '@/utils/consts'
import { fmtMoney } from '@/utils/format'
import type { PriceRuleIn, PriceUnit, StatsCurrency, SuggestGroup, UpstreamOut } from '@/api/types'
import SegmentsEditor from './SegmentsEditor.vue'
import { numToDecimal, priceToSegForm, segToPrice, type SegmentForm } from './types'

interface UnitState {
  unit: PriceUnit
  enabled: boolean
  base_price: number | null
  segments: SegmentForm[]
  /** 已存在的规则 id（提交时据此 update/delete） */
  existingId: string | null
}

const TOKEN_UNITS: PriceUnit[] = ['token_in', 'token_out', 'token_cache_write', 'token_cache_read']

defineProps<{ upstreams: UpstreamOut[] }>()
const emit = defineEmits<{ (e: 'saved'): void }>()

const visible = ref(false)
const saving = ref(false)
const upstreamId = ref('')
const modelId = ref('')
const currency = ref<StatsCurrency>('CNY')
const contextBasis = ref('prompt_tokens')
const suggestGroups = ref<SuggestGroup[]>([])

function blankUnits(): UnitState[] {
  return TOKEN_UNITS.map((u) => ({ unit: u, enabled: false, base_price: null, segments: [], existingId: null }))
}
const units = reactive<UnitState[]>(blankUnits())

async function loadExisting() {
  if (!upstreamId.value || !modelId.value.trim()) return
  try {
    const rules = await pricingApi.list(upstreamId.value, modelId.value.trim())
    for (const u of units) {
      const hit = rules.find((r) => r.unit === u.unit)
      if (hit) {
        u.existingId = hit.id
        u.enabled = hit.enabled
        u.base_price = Number(hit.base_price)
        u.segments = (hit.segments ?? []).map(priceToSegForm)
        currency.value = hit.currency
        contextBasis.value = hit.context_basis || 'prompt_tokens'
      } else {
        u.existingId = null
      }
    }
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

async function loadSuggest() {
  suggestGroups.value = []
  if (!upstreamId.value || !modelId.value.trim()) return
  try {
    const resp = await pricingApi.suggest(upstreamId.value, modelId.value.trim())
    suggestGroups.value = resp.items
  } catch {
    /* 建议不可用时静默 */
  }
}

function onPairChange() {
  loadExisting()
  loadSuggest()
}

function adoptGroup(g: SuggestGroup, unit: string) {
  const targets = unit === '__all__' ? g.rules : g.rules.filter((r) => r.unit === unit)
  let n = 0
  for (const r of targets) {
    const u = units.find((x) => x.unit === r.unit)
    if (!u) continue
    u.enabled = true
    u.base_price = Number(r.base_price)
    u.segments = (r.segments ?? []).map(priceToSegForm)
    currency.value = r.currency
    n++
  }
  if (n) ElMessage.info(`已填充 ${n} 个单位的价格，请核对后保存`)
}

function open(prefill?: { upstream_id?: string; model_id?: string }) {
  upstreamId.value = prefill?.upstream_id ?? ''
  modelId.value = prefill?.model_id ?? ''
  currency.value = 'CNY'
  contextBasis.value = 'prompt_tokens'
  suggestGroups.value = []
  units.splice(0, units.length, ...blankUnits())
  visible.value = true
  if (upstreamId.value && modelId.value) onPairChange()
}

async function submit() {
  if (saving.value) return
  if (!upstreamId.value) return ElMessage.warning('请选择上游')
  if (!modelId.value.trim()) return ElMessage.warning('请输入模型 ID')
  for (const u of units) {
    if (u.enabled && (u.base_price === null || Number.isNaN(Number(u.base_price)) || u.base_price <= 0)) {
      return ElMessage.warning(`「${UNIT_LABEL(u.unit)}」已启用，请填写大于 0 的单价`)
    }
  }
  saving.value = true
  let created = 0
  let updated = 0
  let removed = 0
  try {
    for (const u of units) {
      if (u.enabled) {
        const body: PriceRuleIn = {
          upstream_id: upstreamId.value,
          model_id: modelId.value.trim(),
          unit: u.unit,
          currency: currency.value,
          base_price: numToDecimal(u.base_price ?? 0),
          context_basis: contextBasis.value,
          segments: u.segments.length ? u.segments.map(segToPrice) : null,
          enabled: true,
        }
        if (u.existingId) {
          await pricingApi.update(u.existingId, body)
          updated++
        } else {
          await pricingApi.create(body)
          created++
        }
      } else if (u.existingId) {
        await pricingApi.remove(u.existingId)
        removed++
      }
    }
    ElMessage.success(`已保存：新建 ${created} / 更新 ${updated} / 删除 ${removed}`)
    visible.value = false
    emit('saved')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    saving.value = false
  }
}

defineExpose({ open })
</script>

<style scoped>
.head-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  column-gap: 18px;
}
.suggest-bar {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  margin: 0 0 12px 110px;
}
.unit-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 12px;
}
.unit-card {
  border: 1px solid #e2e8eb;
  border-radius: 8px;
  padding: 12px;
  background: #fff;
}
.unit-card.off {
  background: #f7f8f9;
}
.unit-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 4px;
}
.unit-title {
  font-size: 13px;
  font-weight: 600;
  color: #17202a;
}
.unit-body {
  margin-top: 8px;
}
.unit-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.unit-label {
  font-size: 12px;
  color: #4c5f69;
  width: 30px;
  flex-shrink: 0;
}
.unit-segs {
  margin-top: 8px;
  --el-collapse-header-height: 34px;
}
.segs-title {
  font-size: 12px;
  color: #4c5f69;
}
.unit-off {
  padding: 10px 0 4px;
  text-align: center;
}
@media (max-width: 768px) {
  .head-grid,
  .unit-grid {
    grid-template-columns: 1fr;
  }
  .suggest-bar {
    margin-left: 0;
  }
}
</style>
