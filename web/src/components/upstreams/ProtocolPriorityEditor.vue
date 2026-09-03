<template>
  <div class="priority-editor">
    <div class="hint" style="margin-bottom: 8px">
      标准协议请求优先级：越靠前处理优先级越高。「已选」协议可通过上下按钮调整顺序，「未选」协议固定排在最后。
    </div>
    <div
      v-for="(p, idx) in order"
      :key="p"
      class="priority-row"
      :class="{ 'is-unselected': !selectedSet.has(p) }"
    >
      <el-tag :type="selectedSet.has(p) ? 'primary' : 'info'" size="small">{{ PROTOCOL_LABELS[p] }}</el-tag>
      <span class="priority-state">{{ selectedSet.has(p) ? '已选' : '未选' }}</span>
      <span class="priority-ops">
        <el-button size="small" :icon="ArrowUp" :disabled="!canMove(idx, -1)" @click="move(idx, -1)" />
        <el-button size="small" :icon="ArrowDown" :disabled="!canMove(idx, 1)" @click="move(idx, 1)" />
      </span>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ArrowDown, ArrowUp } from '@element-plus/icons-vue'
import { DEFAULT_PRIORITY, PROTOCOL_LABELS, STANDARD_PROTOCOLS } from '@/utils/consts'
import type { ProtocolName } from '@/api/types'

const props = defineProps<{
  /** 当前完整标准协议顺序数组 */
  modelValue: ProtocolName[]
  /** 已在 protocols 中选中的标准协议 */
  selected: ProtocolName[]
}>()

const emit = defineEmits<{ (e: 'update:modelValue', v: ProtocolName[]): void }>()

const selectedSet = computed(() => new Set(props.selected))

const order = ref<ProtocolName[]>(normalize(props.modelValue, props.selected))

watch(
  () => [props.modelValue, props.selected],
  () => {
    order.value = normalize(props.modelValue, props.selected)
  },
  { deep: true },
)

function normalize(list: ProtocolName[], selected: ProtocolName[]): ProtocolName[] {
  const selSet = new Set(selected.filter((p) => STANDARD_PROTOCOLS.includes(p)))
  const base: ProtocolName[] = list.length ? list : [...DEFAULT_PRIORITY]
  const indexIn = (arr: ProtocolName[]) => (p: ProtocolName) => {
    const i = arr.indexOf(p)
    return i === -1 ? arr.length : i
  }
  const selOrdered = STANDARD_PROTOCOLS.filter((p) => selSet.has(p)).sort(
    (a, b) => indexIn(base)(a) - indexIn(base)(b),
  )
  const unselOrdered = STANDARD_PROTOCOLS.filter((p) => !selSet.has(p)).sort(
    (a, b) => indexIn(DEFAULT_PRIORITY)(a) - indexIn(DEFAULT_PRIORITY)(b),
  )
  return [...selOrdered, ...unselOrdered]
}

function selectedPositions(): number[] {
  return order.value.map((v, i) => (selectedSet.value.has(v) ? i : -1)).filter((i) => i >= 0)
}

function canMove(idx: number, delta: number): boolean {
  if (!selectedSet.value.has(order.value[idx])) return false
  const pos = selectedPositions()
  const cur = pos.indexOf(idx)
  const nxt = cur + delta
  return nxt >= 0 && nxt < pos.length
}

function move(idx: number, delta: number) {
  const pos = selectedPositions()
  const cur = pos.indexOf(idx)
  const nxt = cur + delta
  if (nxt < 0 || nxt >= pos.length) return
  const other = pos[nxt]
  const arr = [...order.value]
  ;[arr[idx], arr[other]] = [arr[other], arr[idx]]
  order.value = arr
  emit('update:modelValue', arr)
}
</script>

<style scoped>
.priority-editor {
  width: 100%;
}
.priority-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 8px;
  border: 1px solid #ebeef5;
  border-radius: 6px;
  margin-bottom: 6px;
  background: #fff;
}
.priority-row.is-unselected {
  opacity: 0.65;
  background: #fafafa;
}
.priority-state {
  font-size: 12px;
  color: #909399;
}
.priority-ops {
  margin-left: auto;
  display: inline-flex;
  gap: 4px;
}
</style>
