<template>
  <div class="overrides-editor">
    <div class="hint" style="margin-bottom: 12px">
      请求头 / 请求体覆盖（add = 追加，set = 覆盖）。空组不会提交。value 若为合法 JSON（对象 / 数组 / 数字 / 布尔）会自动解析，其余按字符串处理；
      想保留字符串形式的数字 / 布尔（如 <span class="mono">"true"</span>），请加英文双引号。每行右侧标签实时显示解析后的类型。
    </div>
    <div v-for="g in groupKeys" :key="g" class="ov-group">
      <div class="ov-group-head">{{ groupLabel(g) }}</div>
      <div v-for="(row, ri) in groups[g]" :key="ri" class="ov-row">
        <el-input
          v-model="row.key"
          placeholder="覆盖的 key"
          class="ov-key"
          :class="{ 'ov-key-dup': isDup(g, row.key), 'ov-key-empty': !row.key.trim() && !!row.value.trim() }"
          clearable
        />
        <el-input v-model="row.value" placeholder="覆盖的值" class="ov-value" clearable />
        <el-tag v-if="row.value.trim()" size="small" type="info" effect="plain" class="ov-type">
          {{ valueType(row.value) }}
        </el-tag>
        <el-tooltip content="删除该行" placement="top">
          <el-button class="ov-del" size="small" :icon="Delete" aria-label="删除该行" @click="removeRow(g, ri)" />
        </el-tooltip>
      </div>
      <div v-if="dupKeys(g).length" class="ov-warn">
        重复 key：{{ dupKeys(g).join('、') }}（保存时后者覆盖前者）
      </div>
      <div v-if="hasValueWithoutKey(g)" class="ov-warn">存在填了值但未填 key 的行，保存时会被忽略</div>
      <el-button size="small" :icon="Plus" @click="addRow(g)">新增键值</el-button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { reactive, watch } from 'vue'
import { Delete, Plus } from '@element-plus/icons-vue'
import type { Overrides } from '@/api/types'

type GroupKey = 'headers.add' | 'headers.set' | 'body.add' | 'body.set'

interface KVRows {
  key: string
  value: string
}

const groupKeys: GroupKey[] = ['headers.add', 'headers.set', 'body.add', 'body.set']

const groupLabels: Record<GroupKey, string> = {
  'headers.add': '请求头 · add（追加）',
  'headers.set': '请求头 · set（覆盖）',
  'body.add': '请求体 · add（追加）',
  'body.set': '请求体 · set（覆盖）',
}

const props = defineProps<{ modelValue: Overrides | null }>()
const emit = defineEmits<{ (e: 'update:modelValue', v: Overrides): void }>()

function valToText(v: unknown): string {
  if (typeof v === 'string') return v
  try {
    return JSON.stringify(v)
  } catch {
    return String(v)
  }
}

function initRows(rec?: Record<string, unknown>): KVRows[] {
  if (!rec) return []
  return Object.entries(rec).map(([k, v]) => ({ key: k, value: valToText(v) }))
}

const groups = reactive<Record<GroupKey, KVRows[]>>({
  'headers.add': initRows(props.modelValue?.headers?.add),
  'headers.set': initRows(props.modelValue?.headers?.set),
  'body.add': initRows(props.modelValue?.body?.add),
  'body.set': initRows(props.modelValue?.body?.set),
})

function parseValue(text: string): unknown {
  const t = text.trim()
  if (!t) return ''
  try {
    return JSON.parse(t)
  } catch {
    return text
  }
}

function build(): Overrides {
  const out: Overrides = {}
  const addTo = (path: 'headers' | 'body', sub: 'add' | 'set', rec: Record<string, unknown>) => {
    if (path === 'headers') out.headers = out.headers ?? {}
    else out.body = out.body ?? {}
    const block = path === 'headers' ? out.headers! : out.body!
    block[sub] = rec
  }
  for (const g of groupKeys) {
    const [path, sub] = g.split('.') as ['headers' | 'body', 'add' | 'set']
    const rec: Record<string, unknown> = {}
    for (const r of groups[g]) {
      const k = r.key.trim()
      if (!k) continue
      rec[k] = parseValue(r.value)
    }
    if (Object.keys(rec).length) addTo(path, sub, rec)
  }
  return out
}

watch(
  groups,
  () => {
    emit('update:modelValue', build())
  },
  { deep: true },
)

function groupLabel(g: GroupKey): string {
  return groupLabels[g]
}

/** 实时显示 value 的解析结果类型（与 build() 的 parseValue 同一口径） */
function valueType(text: string): string {
  const t = text.trim()
  if (!t) return ''
  try {
    const v = JSON.parse(t)
    if (v === null) return 'null'
    if (Array.isArray(v) || typeof v === 'object') return 'JSON'
    if (typeof v === 'number') return 'number'
    if (typeof v === 'boolean') return 'bool'
    return 'string'
  } catch {
    return 'string'
  }
}

/** 同组内重复 key（保存时后者覆盖前者，需提示用户） */
function dupKeys(g: GroupKey): string[] {
  const seen = new Set<string>()
  const dup = new Set<string>()
  for (const r of groups[g]) {
    const k = r.key.trim()
    if (!k) continue
    if (seen.has(k)) dup.add(k)
    seen.add(k)
  }
  return [...dup]
}

function isDup(g: GroupKey, key: string): boolean {
  const k = key.trim()
  return !!k && dupKeys(g).includes(k)
}

function hasValueWithoutKey(g: GroupKey): boolean {
  return groups[g].some((r) => !r.key.trim() && !!r.value.trim())
}

function addRow(g: GroupKey) {
  groups[g].push({ key: '', value: '' })
}

function removeRow(g: GroupKey, i: number) {
  groups[g].splice(i, 1)
}
</script>

<style scoped>
.ov-group {
  border: 1px solid #ebeef5;
  border-radius: 6px;
  padding: 10px;
  margin-bottom: 10px;
  background: #fafafa;
}
.ov-group-head {
  font-size: 13px;
  font-weight: 600;
  color: #1f2329;
  margin-bottom: 8px;
}
.ov-row {
  display: flex;
  gap: 8px;
  margin-bottom: 8px;
}
.ov-key {
  width: 40%;
}
.ov-value {
  flex: 1;
}
.ov-del {
  align-self: center;
}
.ov-type {
  align-self: center;
  flex-shrink: 0;
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
}
.ov-warn {
  color: #e6a23c;
  font-size: 12px;
  margin: -4px 0 8px;
}
.ov-key-dup :deep(.el-input__wrapper) {
  box-shadow: 0 0 0 1px #f56c6c inset;
}
.ov-key-empty :deep(.el-input__wrapper) {
  box-shadow: 0 0 0 1px #e6a23c inset;
}
</style>
