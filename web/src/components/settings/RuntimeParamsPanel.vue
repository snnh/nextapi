<template>
  <div class="runtime-panel">
    <div class="toolbar">
      <el-button type="primary" :loading="saving" :icon="Check" @click="save">保存修改</el-button>
      <span class="hint">仅提交有变化的键，热生效；「需重启」项参见标注。</span>
      <div class="spacer" />
      <span class="source-stats">
        file {{ sourceStats.file }} / ui {{ sourceStats.ui }} / env {{ sourceStats.env }}
      </span>
    </div>

    <el-empty v-if="!groups.length" description="暂无运行参数" />

    <el-card v-for="g in groups" :key="g.prefix" shadow="never" class="group-card">
      <template #header>
        <div class="group-title">
          <span>{{ g.title }}</span>
          <span class="group-count">{{ g.items.length }}</span>
        </div>
      </template>

      <div v-for="it in g.items" :key="it.key" class="setting-row">
        <div class="setting-label">
          <span class="key-mono">{{ shortKey(it.key) }}</span>
          <span class="label-text">{{ labelOf(it) }}</span>
          <el-tooltip v-if="helpOf(it)" placement="top">
            <template #content>
              <div style="max-width: 380px; line-height: 1.6">{{ helpOf(it) }}</div>
            </template>
            <el-icon class="help-icon" :size="14"><QuestionFilled /></el-icon>
          </el-tooltip>
          <el-tag v-if="it.restart_required" type="danger" size="small" effect="light">需重启</el-tag>
          <el-tag v-if="it.secret" type="warning" size="small" effect="light">敏感</el-tag>
        </div>

        <div class="setting-control">
          <el-tooltip
            :disabled="!isEnv(it)"
            placement="bottom"
            content="环境变量覆盖，仅启动时读取"
          >
            <span class="control-wrap">
              <el-switch
                v-if="kindOf(it) === 'switch'"
                :model-value="boolVal(it)"
                :disabled="isEnv(it)"
                @update:model-value="(v: string | number | boolean) => setBool(it, v)"
              />
              <el-input-number
                v-else-if="kindOf(it) === 'number'"
                :model-value="numVal(it)"
                :disabled="isEnv(it)"
                :min="0"
                :controls-position="'right'"
                style="width: 200px"
                @update:model-value="(v: number | undefined) => setNum(it, v)"
              />
              <el-select
                v-else-if="kindOf(it) === 'select'"
                :model-value="strVal(it)"
                :disabled="isEnv(it)"
                style="width: 220px"
                @update:model-value="(v: unknown) => setSelect(it, v)"
              >
                <el-option v-for="o in enumOptions(it) || []" :key="o.value" :value="o.value" :label="o.label" />
              </el-select>
              <el-input
                v-else-if="kindOf(it) === 'password'"
                :model-value="secretVals[it.key] || ''"
                type="password"
                show-password
                :disabled="isEnv(it)"
                :placeholder="secretPlaceholder(it)"
                class="secret-input"
                @update:model-value="(v: string) => setSecret(it, v)"
              />
              <div v-else-if="kindOf(it) === 'tags'" class="tags-editor">
                <el-tag
                  v-for="(t, i) in arrVal(it.key)"
                  :key="i"
                  closable
                  size="small"
                  :disabled="isEnv(it)"
                  @close="removeTag(it.key, i)"
                >
                  {{ t }}
                </el-tag>
                <el-input
                  v-model="tagInput[it.key]"
                  :disabled="isEnv(it)"
                  placeholder="回车添加"
                  size="small"
                  class="tag-input"
                  @keyup.enter="addTag(it.key)"
                />
              </div>
              <el-input
                v-else
                :model-value="strVal(it)"
                :disabled="isEnv(it)"
                clearable
                class="text-input"
                @update:model-value="(v: string) => setStr(it, v)"
              />
            </span>
          </el-tooltip>
        </div>

        <div class="setting-source">
          <el-tooltip v-if="isEnv(it)" placement="top" content="环境变量覆盖，仅启动时读取">
            <el-tag :type="sourceType(it.source)" size="small" effect="plain">{{ sourceLabel(it.source) }}</el-tag>
          </el-tooltip>
          <el-tag v-else :type="sourceType(it.source)" size="small" effect="plain">{{ sourceLabel(it.source) }}</el-tag>
        </div>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { Check, QuestionFilled } from '@element-plus/icons-vue'
import { settingsApi } from '@/api'
import { errMsg } from '@/api/http'
import type { SettingItem } from '@/api/types'
import { FALLBACK_GROUP_TITLE, SETTING_GROUPS, SETTING_FIELDS, shortKey } from './fields'

const props = defineProps<{ settings: SettingItem[] }>()
const emit = defineEmits<{ (e: 'saved'): void }>()

const saving = ref(false)

type Val = SettingItem['value']
const local = reactive<Record<string, Val>>({})
const originalVals: Record<string, Val> = {}
const secretVals = reactive<Record<string, string>>({})
const tagInput = reactive<Record<string, string>>({})

function syncFromProps() {
  for (const k of Object.keys(local)) delete local[k]
  for (const k of Object.keys(secretVals)) delete secretVals[k]
  for (const k of Object.keys(tagInput)) delete tagInput[k]
  for (const k of Object.keys(originalVals)) delete originalVals[k]
  for (const it of props.settings) {
    local[it.key] = it.value
    originalVals[it.key] = it.value
    secretVals[it.key] = ''
    tagInput[it.key] = ''
  }
}

watch(() => props.settings, syncFromProps, { immediate: true, deep: false })

// —— 分组 ——
interface GroupView {
  prefix: string
  title: string
  items: SettingItem[]
}

const groups = computed<GroupView[]>(() => {
  const map = new Map<string, SettingItem[]>()
  for (const it of props.settings) {
    const prefix = it.key.split('.')[0]
    if (!map.has(prefix)) map.set(prefix, [])
    map.get(prefix)!.push(it)
  }
  const result: GroupView[] = []
  for (const g of SETTING_GROUPS) {
    const items = map.get(g.prefix)
    if (items?.length) result.push({ prefix: g.prefix, title: g.title, items })
  }
  for (const [prefix, items] of map) {
    if (!SETTING_GROUPS.some((g) => g.prefix === prefix)) {
      result.push({ prefix, title: FALLBACK_GROUP_TITLE, items })
    }
  }
  return result
})

// —— 来源统计 ——
const sourceStats = computed(() => {
  const s = { file: 0, ui: 0, env: 0 }
  for (const it of props.settings) {
    if (it.source === 'file') s.file++
    else if (it.source === 'ui') s.ui++
    else if (it.source === 'env') s.env++
  }
  return s
})

// —— 展示辅助 ——
function fieldOf(it: SettingItem) {
  return SETTING_FIELDS[it.key] ?? null
}
function labelOf(it: SettingItem): string {
  return fieldOf(it)?.label ?? it.key
}
function helpOf(it: SettingItem): string {
  return fieldOf(it)?.help ?? ''
}
function isEnv(it: SettingItem): boolean {
  return it.source === 'env'
}
function sourceLabel(src: SettingItem['source']): string {
  return src
}
function sourceType(src: SettingItem['source']): 'primary' | 'success' | 'warning' {
  if (src === 'file') return 'primary'
  if (src === 'ui') return 'success'
  return 'warning'
}

interface EnumOption {
  value: string
  label: string
}
function enumOptions(it: SettingItem): EnumOption[] | null {
  const k = shortKey(it.key)
  if (k === 'display_currency') return [{ value: 'CNY', label: 'CNY' }, { value: 'USD', label: 'USD' }]
  if (k === 'provider') return ['frankfurter', 'ecb', 'custom'].map((v) => ({ value: v, label: v }))
  if (k === 'quota_exceed_action') return ['block', 'warn'].map((v) => ({ value: v, label: v }))
  return null
}

function kindOf(it: SettingItem): 'switch' | 'number' | 'select' | 'input' | 'password' | 'tags' {
  if (it.secret) return 'password'
  const v = local[it.key]
  if (typeof v === 'boolean') return 'switch'
  if (typeof v === 'number') return 'number'
  if (Array.isArray(v)) return 'tags'
  if (typeof v === 'string' && enumOptions(it)) return 'select'
  return 'input'
}

// —— 控件读写 ——
function boolVal(it: SettingItem): boolean {
  return typeof local[it.key] === 'boolean' ? (local[it.key] as boolean) : false
}
function numVal(it: SettingItem): number | null {
  return typeof local[it.key] === 'number' ? (local[it.key] as number) : null
}
function strVal(it: SettingItem): string {
  const v = local[it.key]
  if (typeof v === 'string') return v
  return v == null ? '' : String(v)
}
function setBool(it: SettingItem, v: string | number | boolean) {
  local[it.key] = Boolean(v)
}
function setNum(it: SettingItem, v: number | undefined | null) {
  local[it.key] = v ?? null
}
function setStr(it: SettingItem, v: string) {
  local[it.key] = v
}
function setSelect(it: SettingItem, v: unknown) {
  local[it.key] = String(v ?? '')
}
function setSecret(it: SettingItem, v: string) {
  secretVals[it.key] = v
}
function secretPlaceholder(it: SettingItem): string {
  const v = local[it.key]
  const has = v === '***' || (typeof v === 'string' && v !== '')
  return has ? '已设置(***)，输入新值以修改' : '未设置，输入新值'
}

// —— tag 编辑器 ——
function arrVal(key: string): string[] {
  const v = local[key]
  return Array.isArray(v) ? v : []
}
function addTag(key: string) {
  const v = (tagInput[key] || '').trim()
  if (!v) return
  const arr = arrVal(key)
  if (!arr.includes(v)) {
    arr.push(v)
    local[key] = [...arr]
  }
  tagInput[key] = ''
}
function removeTag(key: string, idx: number) {
  const arr = arrVal(key)
  arr.splice(idx, 1)
  local[key] = [...arr]
}

// —— 保存 ——
function sameValue(a: Val, b: Val): boolean {
  if (Array.isArray(a) && Array.isArray(b)) return JSON.stringify(a) === JSON.stringify(b)
  return a === b
}

async function save() {
  const diff: Record<string, string | number | boolean | string[] | null> = {}
  for (const it of props.settings) {
    const key = it.key
    if (it.secret) {
      const input = (secretVals[key] || '').trim()
      if (input && input !== '***') diff[key] = input
      continue
    }
    const cur = local[key]
    const orig = originalVals[key]
    if (!sameValue(cur, orig)) diff[key] = cur as string | number | boolean | string[] | null
  }
  if (!Object.keys(diff).length) {
    ElMessage.info('没有可保存的修改')
    return
  }
  saving.value = true
  try {
    await settingsApi.put(diff)
    ElMessage.success('已保存并热生效（需重启项见标注）')
    emit('saved')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    saving.value = false
  }
}
</script>

<style scoped>
.toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin-bottom: 14px;
}
.toolbar .hint {
  font-size: 12px;
  color: #909399;
}
.toolbar .spacer {
  flex: 1;
}
.source-stats {
  font-size: 12px;
  color: #909399;
}
.group-card {
  margin-bottom: 16px;
}
.group-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  font-weight: 600;
  color: #303133;
}
.group-count {
  font-size: 12px;
  font-weight: 400;
  color: #909399;
}
.setting-row {
  display: flex;
  align-items: flex-start;
  gap: 16px;
  padding: 10px 0;
  border-bottom: 1px solid #f0f2f5;
}
.setting-row:last-child {
  border-bottom: none;
}
.setting-label {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.key-mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12px;
  color: #909399;
  background: #f5f7fa;
  border-radius: 4px;
  padding: 1px 6px;
  white-space: nowrap;
}
.label-text {
  font-size: 14px;
  color: #303133;
}
.help-icon {
  color: #c0c4cc;
  cursor: help;
}
.setting-control {
  width: 360px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
}
.control-wrap {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
}
.setting-source {
  width: 70px;
  flex-shrink: 0;
  text-align: right;
}
.tags-editor {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  width: 100%;
}
.tag-input {
  width: 120px;
}
.secret-input,
.text-input {
  width: 100%;
}
@media (max-width: 900px) {
  .setting-row {
    flex-direction: column;
    gap: 8px;
  }
  .setting-control {
    width: 100%;
  }
  .setting-source {
    width: auto;
    text-align: left;
  }
}
</style>
