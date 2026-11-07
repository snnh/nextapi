<template>
  <div class="runtime-panel">
    <!-- 顶部工具栏：搜索 + 来源/改动筛选 + 展开收起；保存统一在吸底栏 -->
    <div class="toolbar settings-toolbar">
      <el-input
        v-model="keyword"
        class="search-input"
        clearable
        :prefix-icon="Search"
        placeholder="搜索名称、键名或说明"
      />
      <el-select v-model="sourceFilter" class="source-filter" aria-label="来源筛选">
        <el-option label="全部来源" value="all" />
        <el-option label="配置文件" value="file" />
        <el-option label="界面" value="ui" />
        <el-option label="环境变量" value="env" />
      </el-select>
      <el-checkbox v-model="onlyDirty" class="dirty-filter" :disabled="!dirtyCount">
        仅看已修改
      </el-checkbox>
      <el-button @click="toggleAll">{{ allOpen ? '全部收起' : '全部展开' }}</el-button>
      <div class="spacer" />
      <div class="source-stats" aria-label="设置项来源统计">
        <span class="stat-chip"><b>{{ settings.length }}</b> 项</span>
        <span class="stat-chip"><i class="dot file" />配置文件 {{ sourceStats.file }}</span>
        <span class="stat-chip"><i class="dot ui" />界面 {{ sourceStats.ui }}</span>
        <span class="stat-chip"><i class="dot env" />环境 {{ sourceStats.env }}</span>
      </div>
    </div>

    <el-empty v-if="!groups.length" description="暂无运行参数" />

    <template v-else>
      <!-- 搜索无命中 -->
      <el-empty
        v-if="searching && !filteredItems.length"
        :image-size="80"
        :description="emptySearchDesc"
      />

      <!-- 分组展示：普通模式折叠分组，搜索模式平铺命中项 -->
      <el-collapse
        v-else-if="displayGroups.length"
        v-model="collapseOpen"
        class="group-collapse"
      >
        <el-collapse-item v-for="g in displayGroups" :key="g.prefix" :name="g.prefix">
          <template #title>
            <div class="group-head">
              <span class="group-title">{{ g.title }}</span>
              <span v-if="g.desc" class="group-desc">{{ g.desc }}</span>
              <span class="group-count">{{ g.items.length }} 项</span>
              <el-tag
                v-if="dirtyCountOfGroup(g)"
                size="small"
                type="warning"
                effect="light"
                class="group-dirty"
              >
                未保存 {{ dirtyCountOfGroup(g) }}
              </el-tag>
              <el-button
                v-if="dirtyCountOfGroup(g)"
                link
                type="primary"
                size="small"
                class="group-reset"
                @click.stop="resetGroup(g)"
              >
                撤销本组
              </el-button>
            </div>
          </template>

          <div
            v-for="it in g.items"
            :key="it.key"
            :ref="(el) => setRowEl(it.key, el)"
            class="setting-row"
            :class="{
              'row-flash': flashKey === it.key,
              'is-dirty': dirtyKeys.has(it.key),
              'is-locked': isLocked(it),
            }"
          >
            <div class="setting-main">
              <div class="setting-title">
                <span class="label-text">{{ labelOf(it) }}</span>
                <el-tag v-if="dirtyKeys.has(it.key)" size="small" type="warning" effect="light">未保存</el-tag>
                <el-tag v-if="needsRestart(it)" size="small" type="danger" effect="plain">下次启动</el-tag>
                <el-tag v-if="it.secret" size="small" type="warning" effect="plain">敏感</el-tag>
                <el-tooltip v-if="helpOf(it)" placement="top">
                  <template #content>
                    <div style="max-width: 380px; line-height: 1.6">{{ helpOf(it) }}</div>
                  </template>
                  <el-icon class="help-icon" :size="14"><QuestionFilled /></el-icon>
                </el-tooltip>
              </div>
              <div class="setting-meta">
                <span class="key-mono">{{ it.key }}</span>
                <span class="meta-sep" aria-hidden="true">·</span>
                <span class="effective-text">{{ effectiveOf(it) }}</span>
                <span class="meta-sep" aria-hidden="true">·</span>
                <span class="source-text" :class="`is-${it.source}`">{{ sourceLabel(it.source) }}</span>
                <span v-if="isReadonly(it)" class="readonly-text">只读</span>
                <el-tooltip v-if="isEnv(it)" placement="top" content="环境变量覆盖，界面不可修改">
                  <el-icon class="lock-icon" :size="12"><Lock /></el-icon>
                </el-tooltip>
              </div>
              <p v-if="helpOf(it)" class="help-line">{{ helpOf(it) }}</p>
            </div>

            <div class="setting-control">
              <div class="control-wrap">
                <el-switch
                  v-if="kindOf(it) === 'switch'"
                  :model-value="boolVal(it)"
                  :disabled="isLocked(it)"
                  @update:model-value="(v: string | number | boolean) => setBool(it, v)"
                />
                <el-input-number
                  v-else-if="kindOf(it) === 'number'"
                  :model-value="numVal(it)"
                  :disabled="isLocked(it)"
                  :min="numMin(it)"
                  :max="numMax(it)"
                  :controls-position="'right'"
                  class="number-input"
                  @update:model-value="(v: number | undefined) => setNum(it, v)"
                />
                <el-select
                  v-else-if="kindOf(it) === 'select'"
                  :model-value="strVal(it)"
                  :disabled="isLocked(it)"
                  class="select-input"
                  @update:model-value="(v: unknown) => setSelect(it, v)"
                >
                  <el-option v-for="o in enumOptions(it) || []" :key="o.value" :value="o.value" :label="o.label" />
                </el-select>
                <el-input
                  v-else-if="kindOf(it) === 'password'"
                  :model-value="secretVals[it.key] || ''"
                  type="password"
                  show-password
                  autocomplete="new-password"
                  :disabled="isLocked(it)"
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
                    :disabled="isLocked(it)"
                    @close="removeTag(it.key, i)"
                  >
                    {{ t }}
                  </el-tag>
                  <el-input
                    v-model="tagInput[it.key]"
                    :disabled="isLocked(it)"
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
                  :readonly="isReadonly(it)"
                  :clearable="!isReadonly(it)"
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
                    :disabled="isLocked(it)"
                    @click="restoreDefault(it)"
                  >
                    恢复默认
                  </el-button>
                </el-tooltip>
              </div>
            </div>
          </div>
        </el-collapse-item>
      </el-collapse>
    </template>

    <!-- 吸底保存栏 -->
    <div v-if="groups.length" class="savebar">
      <span class="dirty-count" :class="{ 'is-dirty': dirtyCount > 0 }" aria-live="polite">
        <template v-if="dirtyCount">变更 {{ dirtyCount }} 项</template>
        <template v-else>无变更</template>
      </span>
      <span v-if="dirtyCount" class="save-shortcut"><kbd>Ctrl</kbd>/<kbd>⌘</kbd> + <kbd>S</kbd> 保存</span>
      <div class="spacer" />
      <el-button :disabled="!dirtyCount" @click="resetAll">重置改动</el-button>
      <el-button type="primary" :loading="saving" :disabled="!dirtyCount" @click="save">保存修改</el-button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Lock, QuestionFilled, Search } from '@element-plus/icons-vue'
import { proxyApi, settingsApi } from '@/api'
import { errMsg } from '@/api/http'
import type { ProxyOut, SettingItem } from '@/api/types'
import { FALLBACK_GROUP_TITLE, SETTING_GROUPS, SETTING_FIELDS, SETTING_SCENARIOS, STARTUP_ONLY_KEYS } from './fields'
import type { SettingField } from './fields'

const props = defineProps<{ settings: SettingItem[] }>()
const emit = defineEmits<{ (e: 'saved'): void; (e: 'dirty-change', count: number): void }>()

const saving = ref(false)
const keyword = ref('')
const sourceFilter = ref<'all' | SettingItem['source']>('all')
const flashKey = ref<string | null>(null)
const onlyDirty = ref(false)
const proxies = ref<ProxyOut[]>([])
const proxyLoadFailed = ref(false)

type Val = SettingItem['value']
const local = reactive<Record<string, Val>>({})
const originalVals: Record<string, Val> = {}
const secretVals = reactive<Record<string, string>>({})
const tagInput = reactive<Record<string, string>>({})

function cloneVal(v: Val): Val {
  return Array.isArray(v) ? [...v] : v
}

/**
 * 数值型字段的值归一：启动类键在库里按字符串存（如 database.max_connections = "16"），
 * 若元数据标了 number 就转成数字，否则 el-input-number 会显示空白。
 */
function normalizeVal(key: string, v: Val): Val {
  if (
    SETTING_FIELDS[key]?.type === 'number' &&
    typeof v === 'string' &&
    v.trim() !== '' &&
    Number.isFinite(Number(v))
  ) {
    return Number(v)
  }
  return cloneVal(v)
}

function syncFromProps() {
  for (const k of Object.keys(local)) delete local[k]
  for (const k of Object.keys(secretVals)) delete secretVals[k]
  for (const k of Object.keys(tagInput)) delete tagInput[k]
  for (const k of Object.keys(originalVals)) delete originalVals[k]
  for (const it of props.settings) {
    local[it.key] = normalizeVal(it.key, it.value)
    originalVals[it.key] = normalizeVal(it.key, it.value)
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
const sourceSettings = computed(() =>
  props.settings.filter((it) => sourceFilter.value === 'all' || it.source === sourceFilter.value),
)

const groups = computed<GroupView[]>(() => {
  const result: GroupView[] = []
  const consumed = new Set<string>()

  for (const s of SETTING_SCENARIOS) {
    const items = sourceSettings.value.filter((it) => s.keys.includes(it.key))
    if (!items.length) continue
    for (const it of items) consumed.add(it.key)
    result.push({ prefix: s.id, title: s.title, desc: s.desc, items })
  }

  const restBuckets = new Map<string, SettingItem[]>()
  for (const it of sourceSettings.value) {
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


/** 当前有未保存改动的键集合（行标记 / 分组计数 / 仅看改动共用）。 */
const dirtyKeys = computed(() => {
  return new Set(props.settings.filter((it) => isItemDirty(it)).map((it) => it.key))
})

// —— 搜索与筛选 ——
// 关键词命中名称 / 键名 / 说明；「仅看已修改」可与来源筛选叠加。
const searching = computed(
  () => keyword.value.trim() !== '' || sourceFilter.value !== 'all' || onlyDirty.value,
)
const filteredItems = computed<SettingItem[]>(() => {
  const q = keyword.value.trim().toLowerCase()
  return sourceSettings.value.filter((it) => {
    if (onlyDirty.value && !dirtyKeys.value.has(it.key)) return false
    if (!q) return true
    const hay = [labelOf(it), it.key, helpOf(it)]
    return hay.some((s) => s.toLowerCase().includes(q))
  })
})
const emptySearchDesc = computed(() =>
  onlyDirty.value && !dirtyCount.value ? '没有未保存的修改' : '未找到匹配的参数',
)

/** 搜索模式下的伪分组（占位前缀，不会与真实分组冲突） */
const SEARCH_PREFIX = '__search__'

const displayGroups = computed<GroupView[]>(() => {
  if (searching.value) {
    return [{ prefix: SEARCH_PREFIX, title: '筛选结果', items: filteredItems.value }]
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

const allOpen = computed(() => {
  const gs = displayGroups.value
  return gs.length > 0 && gs.every((g) => collapseOpen.value.includes(g.prefix))
})

function toggleAll() {
  collapseOpen.value = allOpen.value ? [] : displayGroups.value.map((g) => g.prefix)
}

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
  const base = fieldOf(it)?.help ?? ''
  if (it.key.endsWith('.proxy_id') && !it.key.startsWith('proxy.')) {
    const useKey = it.key.replace(/\.proxy_id$/, '.use_proxy')
    if (local[useKey] === false) {
      return `${base} 当前「使用代理」关闭，此项不生效。`
    }
  }
  return base
}
function isEnv(it: SettingItem): boolean {
  return it.source === 'env'
}
/** database.url 只由环境变量/配置文件决定，界面永不可写 */
function isReadonly(it: SettingItem): boolean {
  return it.key === 'database.url'
}
function isLocked(it: SettingItem): boolean {
  return isEnv(it) || isReadonly(it)
}
function needsRestart(it: SettingItem): boolean {
  return it.restart_required || STARTUP_ONLY_KEYS.has(it.key)
}
function effectiveOf(it: SettingItem): string {
  if (needsRestart(it)) return '下次启动'
  if (it.key === 'gateway.argon2_memory_kib') return '改密后生效'
  return '立即生效'
}
function sourceLabel(src: SettingItem['source']): string {
  if (src === 'env') return '环境变量'
  if (src === 'ui') return '界面'
  return '配置文件'
}

interface EnumOption {
  value: string
  label: string
}
function enumOptions(it: SettingItem): EnumOption[] | null {
  const k = it.key.slice(it.key.lastIndexOf('.') + 1)
  if (k === 'display_currency') return [{ value: 'CNY', label: 'CNY' }, { value: 'USD', label: 'USD' }]
  if (k === 'provider') return ['frankfurter', 'ecb', 'custom'].map((v) => ({ value: v, label: v }))
  if (k === 'quota_exceed_action') return ['block', 'warn'].map((v) => ({ value: v, label: v }))
  return null
}

/**
 * 代理 UUID 字段改为可读下拉：空值语义按场景区分；加载失败时回退文本框，
 * 避免把已有 UUID 变成不可编辑/误清空。
 */
function proxyOptions(it: SettingItem): EnumOption[] | null {
  if (!it.key.endsWith('.proxy_id') || proxyLoadFailed.value) return null
  const cur = strVal(it)
  const out: EnumOption[] = [
    {
      value: '',
      label: it.key === 'proxy.default_proxy_id' ? '直连（不设置默认代理）' : '跟随默认代理',
    },
  ]
  for (const p of proxies.value) {
    out.push({ value: p.id, label: `${p.name}${p.enabled ? '' : '（已停用）'} · ${p.kind}` })
  }
  if (cur && !out.some((o) => o.value === cur)) {
    out.push({ value: cur, label: `未知代理 ${cur.slice(0, 8)}…` })
  }
  return out
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
  if (typeof v === 'string' && (enumOptions(it) || proxyOptions(it))) return 'select'
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
async function setBool(it: SettingItem, v: string | number | boolean) {
  const next = Boolean(v)
  if (it.key === 'gateway.auto_disable_upstreams' && !next) {
    try {
      await ElMessageBox.confirm(
        '关闭后，系统会立即解除全部历史「自动禁用」渠道并让它们回到候选池；失败请求仍会照常记录日志与重试。',
        '关闭渠道自动禁用？',
        {
          type: 'warning',
          confirmButtonText: '关闭并解除',
          cancelButtonText: '保持开启',
        },
      )
    } catch {
      return
    }
  }
  local[it.key] = next
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
  if (it.secret || isLocked(it)) return false
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
const dirtyCount = computed(() => dirtyKeys.value.size)

/** 分组内未保存数量（折叠状态下也能看到改动分布）。 */
function dirtyCountOfGroup(g: GroupView): number {
  return g.items.reduce((n, it) => n + (dirtyKeys.value.has(it.key) ? 1 : 0), 0)
}

watch(dirtyCount, (n) => {
  emit('dirty-change', n)
  if (n === 0) onlyDirty.value = false
})

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

/** 撤销一个分组内的未保存改动（只回到已保存值，不修改服务端）。 */
function resetGroup(g: GroupView) {
  for (const it of g.items) {
    local[it.key] = cloneVal(originalVals[it.key])
    secretVals[it.key] = ''
    tagInput[it.key] = ''
  }
  ElMessage.info(`已撤销「${g.title}」的改动`)
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
  if (typeof value !== 'number') return { key, reason: '不能为空' }
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
      const it = props.settings.find((s) => s.key === bad.key)
      ElMessage.error(`「${it ? labelOf(it) : bad.key}」数值未通过校验：${bad.reason}`)
      focusKey(bad.key)
      return
    }
  }
  saving.value = true
  try {
    const payload: Record<string, string | number | boolean | string[] | null> = {}
    for (const c of changes) payload[c.key] = c.value
    await settingsApi.put(payload)
    const restartCount = changes.filter((c) => {
      const it = props.settings.find((s) => s.key === c.key)
      return !!it && needsRestart(it)
    }).length
    const passwordCount = changes.filter(
      (c) => c.key === 'gateway.argon2_memory_kib',
    ).length
    const names = changes.map((c) => c.key)
    const shown = names.length > 5 ? `${names.slice(0, 5).join('、')} 等 ${names.length} 项` : names.join('、')
    const effects = [
      restartCount ? `${restartCount} 项下次启动生效` : '',
      passwordCount ? `${passwordCount} 项改密后生效` : '',
    ].filter(Boolean)
    ElMessage.success(
      effects.length ? `已保存：${shown}（${effects.join('，')}）` : `已保存：${shown}（立即生效）`,
    )
    onlyDirty.value = false
    emit('saved')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    saving.value = false
  }
}

// —— 未保存离开提醒 + Ctrl/⌘+S 保存 ——
function onBeforeUnload(e: BeforeUnloadEvent) {
  if (isDirty.value) {
    e.preventDefault()
    e.returnValue = ''
  }
}
function onKeydown(e: KeyboardEvent) {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's' && dirtyCount.value) {
    e.preventDefault()
    void save()
  }
}
async function loadProxies() {
  try {
    proxies.value = await proxyApi.list()
  } catch {
    // 只影响 proxy_id 字段的控件形态：回退为文本框，避免阻断设置页
    proxyLoadFailed.value = true
  }
}

onMounted(() => {
  window.addEventListener('beforeunload', onBeforeUnload)
  window.addEventListener('keydown', onKeydown)
  void loadProxies()
})
onBeforeUnmount(() => {
  window.removeEventListener('beforeunload', onBeforeUnload)
  window.removeEventListener('keydown', onKeydown)
})
</script>

<style scoped>
/* ---------------------------------------------------------------------------
   工具栏：搜索 / 来源 / 改动筛选与统计
   --------------------------------------------------------------------------- */
.settings-toolbar {
  gap: 10px;
  margin-bottom: 10px;
}
.settings-toolbar .search-input {
  width: 300px;
}
.settings-toolbar .source-filter {
  width: 132px;
  flex-shrink: 0;
}
.dirty-filter {
  margin-left: 2px;
  white-space: nowrap;
}
.source-stats {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  font-size: var(--nx-font-caption);
  color: var(--nx-text-3);
}
.stat-chip {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 2px 8px;
  border: 1px solid var(--nx-border-subtle);
  border-radius: var(--nx-radius-round);
  background: var(--nx-subtle);
  line-height: 1.6;
}
.stat-chip b {
  color: var(--nx-text);
  font-weight: 600;
}
.dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: var(--nx-text-placeholder);
}
.dot.file {
  background: var(--nx-text-3);
}
.dot.ui {
  background: var(--nx-accent-solid);
}
.dot.env {
  background: var(--nx-warning);
}

/* ---------------------------------------------------------------------------
   分组：标题、说明、数量、未保存标记
   --------------------------------------------------------------------------- */
.group-collapse {
  border: none;
}
.group-collapse :deep(.el-collapse-item__header) {
  height: 48px;
  padding: 0 4px;
  font-size: var(--nx-font-body);
  font-weight: 600;
  color: var(--el-text-color-primary);
}
.group-collapse :deep(.el-collapse-item__content) {
  padding: 0 0 6px;
}
.group-head {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  min-width: 0;
}
.group-title {
  flex-shrink: 0;
  font-weight: 600;
}
.group-desc {
  min-width: 0;
  overflow: hidden;
  color: var(--el-text-color-secondary);
  font-size: var(--nx-font-caption);
  font-weight: 400;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.group-count {
  flex-shrink: 0;
  color: var(--el-text-color-secondary);
  font-size: var(--nx-font-caption);
  font-weight: 400;
}
.group-dirty {
  flex-shrink: 0;
}
.group-reset {
  flex-shrink: 0;
  margin-left: 2px;
}

/* ---------------------------------------------------------------------------
   设置行：左侧说明 + 右侧控件；来源/生效方式收敛到元信息行
   --------------------------------------------------------------------------- */
.setting-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(300px, 400px);
  align-items: start;
  gap: 24px;
  padding: 13px 12px;
  border-bottom: 1px solid var(--nx-border-subtle);
  border-radius: var(--nx-radius);
  transition:
    background-color var(--nx-dur-base) var(--nx-ease),
    box-shadow var(--nx-dur-base) var(--nx-ease);
}
.setting-row:last-child {
  border-bottom: none;
}
.setting-row:hover {
  background: var(--nx-subtle);
}
.setting-row.row-flash {
  background: var(--nx-warning-soft);
}
.setting-row.is-dirty {
  box-shadow: inset 3px 0 0 var(--nx-warning);
}
.setting-row.is-locked .setting-control {
  opacity: 0.78;
}
.setting-main {
  min-width: 0;
}
.setting-title {
  display: flex;
  align-items: center;
  gap: 7px;
  flex-wrap: wrap;
  min-width: 0;
}
.label-text {
  color: var(--nx-text);
  font-size: var(--nx-font-body);
  font-weight: 500;
  line-height: 1.4;
}
.help-icon {
  flex-shrink: 0;
  color: var(--nx-text-3);
  cursor: help;
}
.setting-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  margin-top: 4px;
  color: var(--nx-text-3);
  font-size: var(--nx-font-caption);
  line-height: 1.5;
}
.key-mono {
  color: var(--nx-text-3);
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: var(--nx-font-caption);
  overflow-wrap: anywhere;
}
.meta-sep {
  color: var(--nx-text-placeholder);
}
.effective-text,
.readonly-text,
.source-text {
  white-space: nowrap;
}
.readonly-text {
  color: var(--nx-warning);
}
.source-text {
  display: inline-flex;
  align-items: center;
  gap: 5px;
}
.source-text::before {
  content: '';
  display: inline-block;
  width: 5px;
  height: 5px;
  border-radius: 50%;
  background: var(--nx-text-placeholder);
}
.source-text.is-ui {
  color: var(--nx-text-2);
}
.source-text.is-ui::before {
  background: var(--nx-accent-solid);
}
.source-text.is-env {
  color: var(--nx-warning);
}
.source-text.is-env::before {
  background: var(--nx-warning);
}
.lock-icon {
  color: var(--nx-warning);
}
.help-line {
  display: -webkit-box;
  margin: 5px 0 0;
  overflow: hidden;
  color: var(--nx-text-3);
  font-size: var(--nx-font-small);
  line-height: 1.55;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}

/* ---------------------------------------------------------------------------
   控件列：统一宽度与对齐，避免长短控件左右跳动
   --------------------------------------------------------------------------- */
.setting-control {
  display: flex;
  align-items: flex-start;
  justify-content: flex-start;
  min-width: 0;
}
.control-wrap {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  width: 100%;
  min-width: 0;
}
.number-input {
  width: 180px;
}
.select-input {
  width: 220px;
}
.text-input,
.secret-input {
  flex: 1 1 200px;
  min-width: 0;
}
.tags-editor {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  width: 100%;
}
.tag-input {
  width: 128px;
}
.unit-text {
  color: var(--nx-text-3);
  font-size: var(--nx-font-caption);
  white-space: nowrap;
}
.restore-btn {
  margin-left: 0;
}

/* ---------------------------------------------------------------------------
   吸底保存栏
   --------------------------------------------------------------------------- */
.savebar {
  position: sticky;
  bottom: 0;
  z-index: 6;
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  padding: 12px 4px;
  margin-top: 12px;
  background: var(--nx-surface);
  border-top: 1px solid var(--nx-border-subtle);
  box-shadow: var(--nx-shadow-bar);
}
.savebar .spacer {
  flex: 1;
}
.dirty-count {
  color: var(--nx-text-3);
  font-size: var(--nx-font-small);
}
.dirty-count.is-dirty {
  color: var(--nx-warning);
  font-weight: 600;
}
.save-shortcut {
  color: var(--nx-text-3);
  font-size: var(--nx-font-caption);
}
.save-shortcut kbd {
  padding: 1px 4px;
  border: 1px solid var(--nx-border);
  border-bottom-width: 2px;
  border-radius: var(--nx-radius-sm);
  background: var(--nx-subtle);
  color: var(--nx-text-2);
  font-family: inherit;
  font-size: 11px;
}

/* ---------------------------------------------------------------------------
   响应式：≤1400 单列；≤1000 收紧分组说明；≤800 工具栏与控件全宽
   --------------------------------------------------------------------------- */
@media (max-width: 1400px) {
  .setting-row {
    grid-template-columns: 1fr;
    gap: 9px;
  }
  .setting-control {
    width: 100%;
  }
  .control-wrap {
    justify-content: flex-start;
  }
}
@media (max-width: 1000px) {
  .group-desc {
    display: none;
  }
}
@media (max-width: 800px) {
  .settings-toolbar .search-input,
  .settings-toolbar .source-filter {
    width: 100%;
  }
  .dirty-filter {
    margin-left: 0;
  }
  .source-stats {
    width: 100%;
  }
  .source-stats .stat-chip:first-child {
    display: none;
  }
  .help-line {
    display: block;
    -webkit-line-clamp: unset;
  }
  .number-input,
  .select-input,
  .text-input,
  .secret-input {
    flex: 1 1 100%;
    width: 100%;
  }
  .savebar {
    align-items: stretch;
  }
  .savebar .spacer {
    display: none;
  }
  .savebar .el-button {
    flex: 1;
  }
  .save-shortcut {
    display: none;
  }
}
</style>
