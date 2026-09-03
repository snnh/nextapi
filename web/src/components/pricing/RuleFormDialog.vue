<template>
  <el-dialog
    v-model="visible"
    :title="isEdit ? '编辑价格规则' : '新建价格规则'"
    width="820px"
    :close-on-click-modal="false"
    destroy-on-close
  >
    <el-form ref="formRef" :model="form" :rules="rules" label-width="120px" @submit.prevent>
      <el-form-item label="上游" prop="upstream_id">
        <el-select v-model="form.upstream_id" filterable placeholder="请选择上游" style="width: 100%">
          <el-option v-for="u in upstreams" :key="u.id" :value="u.id" :label="u.name" />
        </el-select>
      </el-form-item>

      <el-form-item label="模型 ID" prop="model_id">
        <el-input v-model="form.model_id" placeholder="如 gpt-4o / gemini-2.0-flash" clearable />
      </el-form-item>

      <el-form-item label="计价单位" prop="unit">
        <el-select v-model="form.unit" style="width: 100%">
          <el-option v-for="o in UNIT_OPTIONS" :key="o.value" :value="o.value" :label="o.label" />
        </el-select>
      </el-form-item>

      <el-form-item label="币种" prop="currency">
        <el-radio-group v-model="form.currency">
          <el-radio-button v-for="c in CURRENCIES" :key="c" :value="c">{{ c }}</el-radio-button>
        </el-radio-group>
      </el-form-item>

      <el-form-item label="基础单价" prop="base_price">
        <el-input-number v-model="form.base_price" :min="0" :precision="10" :controls="false" style="width: 240px" />
        <span class="hint" style="margin-left: 10px">按计价单位设置（如每 1M token / 每张 / 每秒）</span>
      </el-form-item>

      <el-form-item label="上下文基准" prop="context_basis">
        <el-select v-model="form.context_basis" style="width: 240px">
          <el-option value="prompt_tokens" label="按 prompt_tokens（仅输入 token）" />
          <el-option value="total_tokens" label="按 total_tokens（输入 + 输出）" />
        </el-select>
        <span class="hint" style="margin-left: 10px">分段「min/max_prompt_tokens」按此基准判定</span>
      </el-form-item>

      <template v-if="isMediaUnit(form.unit)">
        <el-form-item label="维度" prop="dimensions">
          <div class="dim-editor">
            <div class="hint" style="margin-bottom: 8px">
              图片/视频按维度（dimensions）计价。维度键建议：
              <el-tag v-for="k in DIMENSION_KEY_HINTS" :key="k" size="small" effect="plain" class="dim-khint">
                {{ k }}
              </el-tag>
              。dimension_key 由服务端根据请求自动归一化，请勿手工填写。
            </div>
            <div v-for="(d, di) in form.dimensions" :key="di" class="dim-row">
              <el-input v-model="d.key" placeholder="维度键" class="dim-key" clearable />
              <el-input v-model="d.value" placeholder="维度值" class="dim-value" clearable />
              <el-button size="small" :icon="Delete" @click="removeDim(di)" />
            </div>
            <el-button size="small" :icon="Plus" @click="addDim">新增维度</el-button>
          </div>
        </el-form-item>
      </template>

      <el-form-item label="分段" prop="segments">
        <div style="width: 100%">
          <SegmentsEditor v-model="form.segments" :unit="form.unit" />
        </div>
      </el-form-item>

      <el-form-item label="有效期开始" prop="effective_from">
        <el-date-picker
          v-model="form.effective_from"
          type="datetime"
          value-format="YYYY-MM-DD HH:mm:ss"
          placeholder="（可选）"
          clearable
          style="width: 240px"
        />
      </el-form-item>

      <el-form-item label="有效期结束" prop="effective_to">
        <el-date-picker
          v-model="form.effective_to"
          type="datetime"
          value-format="YYYY-MM-DD HH:mm:ss"
          placeholder="（可选）"
          clearable
          style="width: 240px"
        />
      </el-form-item>

      <el-form-item label="优先级">
        <el-input-number v-model="form.priority" :min="0" style="width: 160px" />
        <span class="hint" style="margin-left: 10px">数值大优先</span>
      </el-form-item>

      <el-form-item label="排序">
        <el-input-number v-model="form.sort_order" :min="0" style="width: 160px" />
      </el-form-item>

      <el-form-item label="启用">
        <el-switch v-model="form.enabled" />
      </el-form-item>

      <el-divider content-position="left">猜你想用</el-divider>
      <div style="margin-left: 120px; margin-bottom: 16px">
        <SuggestCard
          :upstream="form.upstream_id"
          :model="form.model_id"
          :current-upstream-id="form.upstream_id"
          @adopt="adoptRule"
        />
      </div>

      <el-divider content-position="left">倍率试算</el-divider>
      <el-collapse v-model="calcOpen" style="margin-left: 120px">
        <el-collapse-item name="calc">
          <template #title>倍率试算器</template>
          <MultiplierCalc @fill="fillBasePrice" />
        </el-collapse-item>
      </el-collapse>
    </el-form>

    <template #footer>
      <el-button @click="visible = false">取消</el-button>
      <el-button type="primary" :loading="saving" @click="submit">保存</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { reactive, ref } from 'vue'
import { ElMessage, type FormInstance, type FormRules } from 'element-plus'
import { Delete, Plus } from '@element-plus/icons-vue'
import dayjs from 'dayjs'
import { pricingApi } from '@/api'
import { errMsg } from '@/api/http'
import { CURRENCIES, DIMENSION_KEY_HINTS, UNIT_OPTIONS } from '@/utils/consts'
import type { PriceRuleIn, PriceRuleRow, UpstreamOut } from '@/api/types'
import SegmentsEditor from './SegmentsEditor.vue'
import SuggestCard from './SuggestCard.vue'
import MultiplierCalc from './MultiplierCalc.vue'
import {
  isMediaUnit,
  numToDecimal,
  parseDimValue,
  priceToSegForm,
  segToPrice,
  type RuleFormState,
} from './types'

defineProps<{ upstreams: UpstreamOut[] }>()
const emit = defineEmits<{ (e: 'saved'): void }>()

const visible = ref(false)
const saving = ref(false)
const isEdit = ref(false)
const editingId = ref('')
const calcOpen = ref<string[]>([])
const formRef = ref<FormInstance>()

function defaultForm(): RuleFormState {
  return {
    upstream_id: '',
    model_id: '',
    unit: 'token_in',
    currency: 'CNY',
    base_price: null,
    context_basis: 'prompt_tokens',
    dimensions: [],
    segments: [],
    effective_from: '',
    effective_to: '',
    priority: 10,
    sort_order: 0,
    enabled: true,
  }
}

const form = reactive<RuleFormState>(defaultForm())

function validateEffective(_r: unknown, _v: unknown, cb: (e?: Error) => void) {
  if (form.effective_from && form.effective_to) {
    if (dayjs(form.effective_to).isBefore(dayjs(form.effective_from))) {
      return cb(new Error('有效期结束需晚于开始'))
    }
  }
  cb()
}

const rules: FormRules = {
  upstream_id: [{ required: true, message: '请选择上游', trigger: 'change' }],
  model_id: [{ required: true, message: '请输入 model_id', trigger: 'blur' }],
  base_price: [
    {
      validator: (_r, v: number | null, cb) => {
        if (v === null || v === undefined) return cb(new Error('请输入基础单价'))
        if (Number.isNaN(Number(v)) || v <= 0) return cb(new Error('基础单价需大于 0'))
        cb()
      },
      trigger: 'blur',
    },
  ],
  effective_from: [{ validator: validateEffective, trigger: 'change' }],
  effective_to: [{ validator: validateEffective, trigger: 'change' }],
}

function open(rule?: PriceRuleRow, prefill?: { upstream_id?: string; model_id?: string }) {
  isEdit.value = !!rule
  editingId.value = rule?.id ?? ''
  calcOpen.value = []
  const base = defaultForm()
  if (rule) {
    base.upstream_id = rule.upstream_id
    base.model_id = rule.model_id
    base.unit = rule.unit
    base.currency = rule.currency
    base.base_price = Number(rule.base_price)
    base.context_basis = rule.context_basis || 'prompt_tokens'
    base.dimensions = Object.entries(rule.dimensions ?? {}).map(([k, v]) => ({ key: k, value: String(v) }))
    base.segments = (rule.segments ?? []).map(priceToSegForm)
    base.effective_from = rule.effective_from ? dayjs(rule.effective_from).format('YYYY-MM-DD HH:mm:ss') : ''
    base.effective_to = rule.effective_to ? dayjs(rule.effective_to).format('YYYY-MM-DD HH:mm:ss') : ''
    base.priority = rule.priority
    base.sort_order = rule.sort_order
    base.enabled = rule.enabled
  }
  if (prefill) {
    if (prefill.upstream_id) base.upstream_id = prefill.upstream_id
    if (prefill.model_id) base.model_id = prefill.model_id
  }
  Object.assign(form, base)
  visible.value = true
}

function addDim() {
  form.dimensions.push({ key: '', value: '' })
}

function removeDim(i: number) {
  form.dimensions.splice(i, 1)
}

function adoptRule(rule: PriceRuleRow) {
  form.unit = rule.unit
  form.currency = rule.currency
  form.base_price = Number(rule.base_price)
  form.context_basis = rule.context_basis || 'prompt_tokens'
  form.segments = (rule.segments ?? []).map(priceToSegForm)
  form.dimensions = Object.entries(rule.dimensions ?? {}).map(([k, v]) => ({ key: k, value: String(v) }))
  ElMessage.info('已复制参考字段，请核对后保存')
}

function fillBasePrice(v: number) {
  form.base_price = v
}

function buildBody(): PriceRuleIn {
  const body: PriceRuleIn = {
    upstream_id: form.upstream_id,
    model_id: form.model_id.trim(),
    unit: form.unit,
    currency: form.currency,
    base_price: numToDecimal(form.base_price ?? 0),
    context_basis: form.context_basis,
    priority: form.priority,
    sort_order: form.sort_order,
    enabled: form.enabled,
    effective_from: form.effective_from ? dayjs(form.effective_from).toISOString() : null,
    effective_to: form.effective_to ? dayjs(form.effective_to).toISOString() : null,
  }
  if (isMediaUnit(form.unit)) {
    const dims: Record<string, unknown> = {}
    for (const d of form.dimensions) {
      const k = d.key.trim()
      if (!k) continue
      dims[k] = parseDimValue(d.value)
    }
    body.dimensions = Object.keys(dims).length ? dims : null
  } else {
    body.dimensions = null
  }
  body.segments = form.segments.length ? form.segments.map(segToPrice) : null
  return body
}

async function submit() {
  if (!formRef.value || saving.value) return
  try {
    await formRef.value.validate()
  } catch {
    return
  }
  saving.value = true
  try {
    const body = buildBody()
    if (isEdit.value) {
      await pricingApi.update(editingId.value, body)
      ElMessage.success('规则已更新')
    } else {
      await pricingApi.create(body)
      ElMessage.success('规则已创建')
    }
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
.dim-editor {
  width: 100%;
}
.dim-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}
.dim-key {
  width: 200px;
}
.dim-value {
  flex: 1;
}
.dim-khint {
  margin: 0 4px;
}
</style>
