<template>
  <div class="segments-editor">
    <div class="hint" style="margin-bottom: 12px">
      {{ hintText }}
    </div>

    <div v-if="inner.length" class="seg-list">
      <div
        v-for="(seg, idx) in inner"
        :key="idx"
        class="seg-card"
        :class="{ 'seg-media': isMediaUnit(unit) }"
      >
        <div class="seg-head">
          <span class="seg-title">分段 {{ idx + 1 }}</span>
          <el-input v-model="seg.name" placeholder="名称，如 peak / offpeak / long_context（可空）" clearable class="seg-name" />
          <span class="seg-ops">
            <el-button size="small" :icon="ArrowUp" :disabled="idx === 0" @click="move(idx, -1)" />
            <el-button size="small" :icon="ArrowDown" :disabled="idx === inner.length - 1" @click="move(idx, 1)" />
            <el-button size="small" type="danger" :icon="Delete" @click="remove(idx)" />
          </span>
        </div>

        <div class="seg-row">
          <span class="seg-label">单价</span>
          <el-input-number v-model="seg.price" :min="0" :precision="6" :controls="false" style="width: 200px" />
          <span class="hint">必填，按当前 unit 计价（如每 1M token）</span>
        </div>

        <div class="seg-row">
          <span class="seg-label">星期</span>
          <el-checkbox-group v-model="seg.weekdays">
            <el-checkbox-button v-for="w in WEEKDAYS" :key="w.value" :value="w.value" size="small">
              {{ w.label }}
            </el-checkbox-button>
          </el-checkbox-group>
          <span class="hint">不选 = 任意星期</span>
        </div>

        <div class="seg-row seg-row-top">
          <span class="seg-label">时间窗口</span>
          <div class="seg-windows">
            <div v-for="(win, wi) in seg.windows" :key="wi" class="seg-window">
              <el-time-select v-model="win.start" start="00:00" step="00:30" end="23:30" placeholder="开始" />
              <span class="seg-window-arrow">→</span>
              <el-time-select v-model="win.end" start="00:00" step="00:30" end="23:30" placeholder="结束" />
              <el-tag v-if="isCrossMidnight(win)" size="small" type="warning" effect="plain">跨午夜</el-tag>
              <el-button size="small" :icon="Close" @click="removeWindow(seg, wi)" />
            </div>
            <el-button size="small" :icon="Plus" @click="addWindow(seg)">新增时间窗口</el-button>
          </div>
        </div>

        <div class="seg-row">
          <span class="seg-label">上下文 Token</span>
          <span class="seg-token">
            最小
            <el-input-number v-model="seg.min_prompt_tokens" :min="0" :controls="false" style="width: 140px" placeholder="可空" />
          </span>
          <span class="seg-token">
            最大
            <el-input-number v-model="seg.max_prompt_tokens" :min="0" :controls="false" style="width: 140px" placeholder="可空" />
          </span>
          <span class="hint">按 context_basis（prompt/total tokens）判定，双空则不校验</span>
        </div>
      </div>
    </div>
    <div v-else class="hint">暂未添加分段。纯兜底段可不配任何条件（仅填写单价）。</div>

    <el-button size="small" :icon="Plus" style="margin-top: 10px" @click="add">添加分段</el-button>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ArrowDown, ArrowUp, Close, Delete, Plus } from '@element-plus/icons-vue'
import { WEEKDAYS } from '@/utils/consts'
import type { PriceUnit } from '@/api/types'
import { isMediaUnit, type SegWindow, type SegmentForm } from './types'

const props = defineProps<{
  modelValue: SegmentForm[]
  unit: PriceUnit
}>()
const emit = defineEmits<{ (e: 'update:modelValue', v: SegmentForm[]): void }>()

function cloneList(list: SegmentForm[]): SegmentForm[] {
  return list.map((s) => ({
    ...s,
    weekdays: [...s.weekdays],
    windows: s.windows.map((w) => ({ ...w })),
  }))
}

const inner = ref<SegmentForm[]>(cloneList(props.modelValue))

// 双向 deep-watch 防自激守卫（review P4）：inner 变化 → emit → 父回写 props（引用必变）
// → 无守卫时会再次同步 inner 生成新引用 → 再 emit……无限循环（Vue 递归更新告警/卡死）。
// 以「最后一次 emit 的序列化快照」为准：回写内容相同时 props watch 不再同步、不再 emit。
const lastEmittedJson = ref(JSON.stringify(props.modelValue))

watch(
  () => props.modelValue,
  (v) => {
    const s = JSON.stringify(v)
    if (s !== lastEmittedJson.value) {
      inner.value = cloneList(v)
      lastEmittedJson.value = s
    }
  },
)

watch(
  inner,
  () => {
    const s = JSON.stringify(inner.value)
    if (s !== lastEmittedJson.value) {
      lastEmittedJson.value = s
      emit('update:modelValue', cloneList(inner.value))
    }
  },
  { deep: true },
)

const hintText = computed(() => {
  if (isMediaUnit(props.unit)) {
    return '按数组顺序取第一个命中；无命中回落 base_price；纯兜底段可不配任何条件。图片/视频按维度（dimensions）计价，建议不使用上下文分段。'
  }
  return '按数组顺序取第一个命中；无命中回落 base_price；纯兜底段可不配任何条件（仅填写单价即可命中）。'
})

function blankSegment(): SegmentForm {
  return {
    name: '',
    price: null,
    weekdays: [],
    windows: [],
    min_prompt_tokens: null,
    max_prompt_tokens: null,
  }
}

function add() {
  inner.value.push(blankSegment())
}

function remove(idx: number) {
  inner.value.splice(idx, 1)
}

function move(idx: number, delta: number) {
  const nxt = idx + delta
  if (nxt < 0 || nxt >= inner.value.length) return
  const arr = [...inner.value]
  ;[arr[idx], arr[nxt]] = [arr[nxt], arr[idx]]
  inner.value = arr
}

function addWindow(seg: SegmentForm) {
  seg.windows.push({ start: '09:00', end: '12:00' })
}

function removeWindow(seg: SegmentForm, wi: number) {
  seg.windows.splice(wi, 1)
}

function isCrossMidnight(win: SegWindow): boolean {
  return !!win.start && !!win.end && win.start > win.end
}
</script>

<style scoped>
.seg-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.seg-card {
  border: 1px solid #ebeef5;
  border-radius: 8px;
  padding: 12px;
  background: #fafafa;
}
.seg-card.seg-media {
  background: #fbf8ef;
}
.seg-head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
}
.seg-title {
  font-size: 13px;
  font-weight: 600;
  color: #1f2329;
  white-space: nowrap;
}
.seg-name {
  flex: 1;
}
.seg-ops {
  display: inline-flex;
  gap: 4px;
}
.seg-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
  flex-wrap: wrap;
}
.seg-row-top {
  align-items: flex-start;
}
.seg-label {
  font-size: 12px;
  color: #6b7280;
  width: 76px;
  flex-shrink: 0;
}
.seg-windows {
  flex: 1;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.seg-window {
  display: flex;
  align-items: center;
  gap: 6px;
}
.seg-window-arrow {
  color: #6b7280;
}
.seg-token {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: #4b5563;
}
</style>
