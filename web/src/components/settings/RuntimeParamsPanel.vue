<template>
  <div class="runtime-panel">
    <!-- 顶部工具栏：保存 + 关键词搜索 + 统计 -->
    <div class="toolbar">
      <el-button type="primary" :loading="saving" :disabled="!dirtyCount" :icon="Check" @click="save">
        保存修改
      </el-button>
      <el-input
        v-model="keyword"
        class="search-input"
        clearable
        :prefix-icon="Search"
        placeholder="搜索参数名称 / 键名"
      />
      <span class="hint">仅提交有变化的键，热生效；「需重启」项参见标注。</span>
      <div class="spacer" />
      <span class="source-stats">
        file {{ sourceStats.file }} / ui {{ sourceStats.ui }} / env {{ sourceStats.env }}
      </span>
    </div>

    <el-empty v-if="!groups.length" description="暂无运行参数" />

    <template v-else>
      <!-- 搜索无命中 -->
      <el-empty
        v-if="searching && !filteredItems.length"
        :image-size="80"
        description="未找到匹配的参数"
      />

      <!-- 分组展示：普通模式折叠分组，搜索模式平铺命中项 -->
      <el-collapse
        v-else-if="displayGroups.length"
        v-model="collapseOpen"
        class="group-collapse"
      >
        <el-collapse-item v-for="g in displayGroups" :key="g.prefix" :name="g.prefix">
          <template #title>
            <span class="group-title">{{ g.title }}</span>
            <span v-if="g.desc" class="group-desc">{{ g.desc }}</span>
            <span class="group-count">{{ g.items.length }} 项</span>
          </template>

          <div
            v-for="it in g.items"
            :key="it.key"
            :ref="(el) => setRowEl(it.key, el)"
            class="setting-row"
            :class="{ 'row-flash': flashKey === it.key }"
          >
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
                  :min="numMin(it)"
                  :max="numMax(it)"
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

                <!-- 数值单位 -->
                <span v-if="kindOf(it) === 'number' && unitOf(it)" class="unit-text">{{ unitOf(it) }}</span>

                <!-- 恢复默认 -->
                <el-tooltip v-if="canRestore(it)" :content="restoreTip(it)" placement="top">
                  <el-button
                    class="restore-btn"
                    size="small"
                    text
                    type="primary"
                    :disabled="isEnv(it)"
                    @click="restoreDefault(it)"
                  >
                    恢复默认
                  </el-button>
                </el-tooltip>
              </span>
            </div>

            <div class="setting-source">
              <el-tooltip v-if="isEnv(it)" placement="top" content="环境变量覆盖，仅启动时读取">
                <el-tag :type="sourceType(it.source)" size="small" effect="plain">{{ sourceLabel(it.source) }}</el-tag>
              </el-tooltip>
              <el-tag v-else :type="sourceType(it.source)" size="small" effect="plain">{{ sourceLabel(it.source) }}</el-tag>
            </div>
          </div>
        </el-collapse-item>
      </el-collapse>
    </template>

    <!-- 吸底保存栏 -->
    <div v-if="groups.length" class="savebar">
      <span class="dirty-count" :class="{ 'is-dirty': dirtyCount > 0 }">
        <template v-if="dirtyCount">变更 {{ dirtyCount }} 项</template>
        <template v-else>无变更</template>
      </span>
      <div class="spacer" />
      <el-button :disabled="!dirtyCount" @click="resetAll">重置改动</el-button>
      <el-button type="primary" :loading="saving" :disabled="!dirtyCount" @click="save">保存修改</el-button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { Check, QuestionFilled, Search } from '@element-plus/icons-vue'
import { settingsApi } from '@/api'
import { errMsg } from '@/api/http'
import type { SettingItem } from '@/api/types'
import { FALLBACK_GROUP_TITLE, SETTING_GROUPS, SETTING_FIELDS, SETTING_SCENARIOS, shortKey } from './fields'
import type { SettingField } from './fields'

const props = defineProps<{ settings: SettingItem[] }>()
const emit = defineEmits<{ (e: 'saved'): void }>()

const saving = ref(false)
const keyword = ref('')
const flashKey = ref<string | null>(null)

type Val = SettingItem['value']
const local = reactive<Record<string, Val>>({})
const originalVals: Record<string, Val> = {}
const secretVals = reactive<Record<string, string>>({})
const tagInput = reactive<Record<string, string>>({})

function cloneVal(v: Val): Val {
  return Array.isArray(v) ? [...v] : v
}

function syncFromProps() {
  for (const k of Object.keys(local)) delete local[k]
  for (const k of Object.keys(secretVals)) delete secretVals[k]
  for (const k of Object.keys(tagInput)) delete tagInput[k]
  for (const k of Object.keys(originalVals)) delete originalVals[k]
  for (const it of props.settings) {
    local[it.key] = cloneVal(it.value)
    originalVals[it.key] = cloneVal(it.value)
    secretVals[it.key] = ''
    tagInput[it.key] = ''
  }
}

watch(() => props.settings, syncFromProps, { immediate: true, deep: false })

// —— 分组 ——
interface GroupView {
  prefix: string
  title: string
  desc?: string
  items: SettingItem[]
}

/**
 * 分组渲染顺序（M13 §4.8 低风险 UI 优化，仅改展示结构，不影响保存/校验逻辑）：
 * 1) 网关运行参数先按场景卡归纳（请求处理 / 流量控制 / 日志与审计 / 路由策略 /
 *    计价展示），覆盖 fields.ts 中已收录的全部 gateway.* 键；
 * 2) 其余参数仍按配置前缀分组（fx_auto_fetch / price_import / media_* / proxy /
 *    update_check / server / database / 未来新增前缀），保留原折叠入口。
 */
const groups = computed<GroupView[]>(() => {
  const result: GroupView[] = []
  const consumed = new Set<string>()

  for (const s of SETTING_SCENARIOS) {
    const items = props.settings.filter((it) => s.keys.includes(it.key))
    if (!items.length) continue
    for (const it of items) consumed.add(it.key)
    result.push({ prefix: s.id, title: s.title, desc: s.desc, items })
  }

  const restBuckets = new Map<string, SettingItem[]>()
  for (const it of props.settings) {
    if (consumed.has(it.key)) continue
    const prefix = it.key.split('.')[0]
    if (!restBuckets.has(prefix)) restBuckets.set(prefix, [])
    restBuckets.get(prefix)!.push(it)
  }
  for (const g of SETTING_GROUPS) {
    const items = restBuckets.get(g.prefix)
    if (items?.length) result.push({ prefix: g.prefix, title: g.title, items })
  }
  for (const [prefix, items] of restBuckets) {
    if (!SETTING_GROUPS.some((g) => g.prefix === prefix)) {
      result.push({ prefix, title: FALLBACK_GROUP_TITLE, items })
    }
  }
  return result
})

// —— 关键词搜索 ——
const searching = computed(() => keyword.value.trim() !== '')
const filteredItems = computed<SettingItem[]>(() => {
  const q = keyword.value.trim().toLowerCase()
  if (!q) return props.settings
  return props.settings.filter((it) => {
    const hay = [labelOf(it), shortKey(it.key), it.key]
    return hay.some((s) => s.toLowerCase().includes(q))
  })
})

/** 搜索模式下的伪分组（占位前缀，不会与真实分组冲突） */
const SEARCH_PREFIX = '__search__'

const displayGroups = computed<GroupView[]>(() => {
  if (searching.value) {
    return [{ prefix: SEARCH_PREFIX, title: '搜索结果', items: filteredItems.value }]
  }
  return groups.value
})

/** 分组折叠：默认展开第一组，其余收起；搜索时仅保留伪分组并自动展开 */
const collapseOpen = ref<string[]>([])
watch(
  displayGroups,
  (gs) => {
    if (!gs.length) {
      collapseOpen.value = []
      return
    }
    if (searching.value) {
      collapseOpen.value = [SEARCH_PREFIX]
      return
    }
    const prefixes = new Set(gs.map((g) => g.prefix))
    const kept = collapseOpen.value.filter((p) => prefixes.has(p) && p !== SEARCH_PREFIX)
    collapseOpen.value = kept.length ? kept : [gs[0].prefix]
  },
  { immediate: true },
)

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
function fieldOf(it: SettingItem): SettingField | null {
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

/**
 * 控件类型判定：
 * 优先依据 fields.ts 元数据的 type 判定（数值清空后 local 值为 null，
 * 仍落回数字控件），再以运行时值兜底细分 boolean/array/select。
 */
function kindOf(it: SettingItem): 'switch' | 'number' | 'select' | 'input' | 'password' | 'tags' {
  if (it.secret) return 'password'
  const v = local[it.key]
  const f = fieldOf(it)
  if (f?.type === 'boolean' || typeof v === 'boolean') return 'switch'
  if (Array.isArray(v)) return 'tags'
  if (f?.type === 'number' || typeof v === 'number') return 'number'
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
function numMin(it: SettingItem): number {
  return fieldOf(it)?.min ?? 0
}
function numMax(it: SettingItem): number | undefined {
  return fieldOf(it)?.max
}
function unitOf(it: SettingItem): string {
  const u = fieldOf(it)?.unit
  return u ?? ''
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

// —— 默认值 / 恢复 ——
function fmtDefault(v: string | number | boolean | string[] | null | undefined): string {
  if (v === undefined || v === null) return '空'
  if (Array.isArray(v)) return v.length ? v.join(', ') : '空'
  if (typeof v === 'boolean') return v ? 'true' : 'false'
  if (v === '') return '空'
  return String(v)
}
function canRestore(it: SettingItem): boolean {
  if (it.secret || isEnv(it)) return false
  const f = fieldOf(it)
  return !!f && f.default !== undefined
}
function restoreTip(it: SettingItem): string {
  const f = fieldOf(it)
  const base = f && f.default !== undefined ? fmtDefault(f.default) : ''
  return `恢复默认：${base}${f?.unit ? ` ${f.unit}` : ''}`
}
function restoreDefault(it: SettingItem) {
  const f = fieldOf(it)
  if (!f || f.default === undefined || it.secret) return
  local[it.key] = cloneVal(f.default)
  secretVals[it.key] = ''
  tagInput[it.key] = ''
}

// —— 脏状态 ——
function sameValue(a: Val, b: Val): boolean {
  if (Array.isArray(a) && Array.isArray(b)) return JSON.stringify(a) === JSON.stringify(b)
  return a === b
}
function isItemDirty(it: SettingItem): boolean {
  if (it.secret) {
    const input = (secretVals[it.key] || '').trim()
    return input !== '' && input !== '***'
  }
  return !sameValue(local[it.key], originalVals[it.key])
}
const dirtyCount = computed(() => props.settings.reduce((n, it) => n + (isItemDirty(it) ? 1 : 0), 0))

/** 暴露给父组件的脏状态（Settings.vue 切换 Tab / 离开页面前确认用） */
const isDirty = computed(() => dirtyCount.value > 0)
defineExpose({ isDirty, dirtyCount })

function resetAll() {
  for (const it of props.settings) {
    local[it.key] = cloneVal(originalVals[it.key])
    secretVals[it.key] = ''
    tagInput[it.key] = ''
  }
  ElMessage.info('已重置全部改动')
}

// —— 行定位（校验失败滚动到对应行并高亮）——
const rowEls = new Map<string, HTMLElement>()
function setRowEl(key: string, el: unknown) {
  const node = el as HTMLElement | null
  if (node) rowEls.set(key, node)
  else rowEls.delete(key)
}

function focusKey(key: string) {
  // 确保该行所属分组已展开
  keyword.value = ''
  const grp = groups.value.find((g) => g.items.some((it) => it.key === key))
  if (grp && !collapseOpen.value.includes(grp.prefix)) collapseOpen.value = [grp.prefix]
  flashKey.value = key
  nextTick(() => {
    const el = rowEls.get(key)
    el?.scrollIntoView({ behavior: 'smooth', block: 'center' })
    window.setTimeout(() => {
      if (flashKey.value === key) flashKey.value = null
    }, 1600)
  })
}

// —— 逐字段校验（fields.ts 元数据 min/max，仅校验即将提交的变更项）——
function invalidOf(key: string, value: Val): { key: string; reason: string } | null {
  const it = props.settings.find((s) => s.key === key)
  if (!it || it.secret) return null
  const f = fieldOf(it)
  if (!f || f.type !== 'number') return null
  if (typeof value !== 'number') return null
  if (f.min !== undefined && value < f.min) return { key, reason: `不能小于 ${f.min}` }
  if (f.max !== undefined && value > f.max) return { key, reason: `不能大于 ${f.max}` }
  return null
}

// —— 保存 ——
interface ChangeEntry {
  key: string
  value: string | number | boolean | string[] | null
}

function buildDiff(): ChangeEntry[] {
  const out: ChangeEntry[] = []
  for (const it of props.settings) {
    const key = it.key
    if (it.secret) {
      const input = (secretVals[key] || '').trim()
      if (input && input !== '***') out.push({ key, value: input })
      continue
    }
    const cur = local[key]
    const orig = originalVals[key]
    if (!sameValue(cur, orig)) out.push({ key, value: cur as string | number | boolean | string[] | null })
  }
  return out
}

async function save() {
  if (saving.value) return
  const changes = buildDiff()
  if (!changes.length) {
    ElMessage.info('没有可保存的修改')
    return
  }
  // 逐字段 min/max 校验，出错指出键名并滚动定位
  for (const c of changes) {
    const bad = invalidOf(c.key, c.value)
    if (bad) {
      ElMessage.error(`「${bad.key}」数值未通过校验：${bad.reason}`)
      focusKey(bad.key)
      return
    }
  }
  saving.value = true
  try {
    const payload: Record<string, string | number | boolean | string[] | null> = {}
    for (const c of changes) payload[c.key] = c.value
    await settingsApi.put(payload)
    const hasRestart = changes.some((c) => {
      const it = props.settings.find((s) => s.key === c.key)
      return !!it?.restart_required
    })
    const names = changes.map((c) => c.key)
    const shown = names.length > 5 ? `${names.slice(0, 5).join('、')} 等 ${names.length} 项` : names.join('、')
    ElMessage.success(hasRestart ? `已保存：${shown}（部分项需重启后生效）` : `已保存：${shown}（热生效）`)
    emit('saved')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    saving.value = false
  }
}

// —— 未保存离开提醒 ——
function onBeforeUnload(e: BeforeUnloadEvent) {
  if (isDirty.value) {
    e.preventDefault()
    e.returnValue = ''
  }
}
onMounted(() => window.addEventListener('beforeunload', onBeforeUnload))
onBeforeUnmount(() => window.removeEventListener('beforeunload', onBeforeUnload))
</script>

<style scoped>
.toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin-bottom: 14px;
}
.toolbar .search-input {
  width: 260px;
  flex-shrink: 0;
}
.toolbar .hint {
  font-size: 12px;
  color: #6b7280;
}
.toolbar .spacer {
  flex: 1;
}
.source-stats {
  font-size: 12px;
  color: #6b7280;
}

.group-collapse {
  border: none;
}
.group-collapse :deep(.el-collapse-item__header) {
  height: 44px;
  font-size: 14px;
  font-weight: 600;
  color: #1f2329;
}
.group-collapse :deep(.el-collapse-item__content) {
  padding: 0 8px 8px;
}
.group-title {
  display: inline-flex;
  align-items: center;
}
.group-desc {
  font-size: 12px;
  font-weight: 400;
  color: #909399;
  margin-left: 10px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 320px;
}
.group-count {
  font-size: 12px;
  font-weight: 400;
  color: #6b7280;
  margin-left: 10px;
}

.setting-row {
  display: flex;
  align-items: flex-start;
  gap: 16px;
  padding: 10px 12px;
  border-bottom: 1px solid #f0f2f5;
  border-radius: 6px;
  transition: background-color 0.3s ease;
}
.setting-row:last-child {
  border-bottom: none;
}
.setting-row.row-flash {
  background-color: #fff7e6;
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
  color: #6b7280;
  background: #f5f7fa;
  border-radius: 4px;
  padding: 1px 6px;
  white-space: nowrap;
}
.label-text {
  font-size: 14px;
  color: #1f2329;
}
.help-icon {
  color: #c0c4cc;
  cursor: help;
}
.setting-control {
  width: 380px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
}
.control-wrap {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  width: 100%;
}
.unit-text {
  font-size: 12px;
  color: #909399;
  white-space: nowrap;
}
.restore-btn {
  margin-left: auto;
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

/* 吸底保存栏 */
.savebar {
  position: sticky;
  bottom: 0;
  z-index: 6;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 4px;
  margin-top: 16px;
  background: #fff;
  border-top: 1px solid #ebeef5;
  box-shadow: 0 -3px 10px rgba(31, 35, 41, 0.06);
}
.savebar .spacer {
  flex: 1;
}
.dirty-count {
  font-size: 13px;
  color: #909399;
}
.dirty-count.is-dirty {
  color: #e6a23c;
  font-weight: 600;
}

@media (max-width: 1100px) {
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
