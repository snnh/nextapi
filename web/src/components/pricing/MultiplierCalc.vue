<template>
  <div class="multiplier-calc">
    <div class="mc-row">
      <div class="mc-field">
        <span class="mc-label">参考单价</span>
        <el-input-number
          v-model="reference"
          :min="0"
          :precision="6"
          :controls="false"
          placeholder="参考单价"
        />
        <span class="mc-unit-hint">token 类按每 1M token、图片按每张、视频按每秒</span>
      </div>
      <span class="mc-op">×</span>
      <div class="mc-field">
        <span class="mc-label">倍率</span>
        <el-input-number
          v-model="multiplier"
          :min="0"
          :precision="4"
          :controls="false"
          placeholder="倍率，如 1.25"
        />
      </div>
      <span class="mc-op">＝</span>
      <span class="mc-result">{{ resultText }}</span>
    </div>
    <div class="mc-actions">
      <el-button size="small" type="primary" :disabled="!canFill" @click="fill">填入 base_price</el-button>
      <el-button size="small" :disabled="!hasValue" @click="clear">清空</el-button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { ElMessage } from 'element-plus'

const emit = defineEmits<{ (e: 'fill', v: number): void }>()

const reference = ref<number | null>(null)
const multiplier = ref<number | null>(null)

const hasValue = computed(() => reference.value !== null || multiplier.value !== null)

const result = computed<number | null>(() => {
  const a = reference.value
  const b = multiplier.value
  if (a === null || b === null) return null
  return a * b
})

const resultText = computed(() => (result.value === null ? '—' : trimNum(result.value)))

const canFill = computed(() => result.value !== null)

function trimNum(n: number): string {
  // 保留至多 10 位小数，去掉末尾 0
  return Number(n.toFixed(10)).toString()
}

function fill() {
  if (result.value === null) return
  emit('fill', result.value)
  // 目标 base_price 在父级表单中，这里用 success 提示确认已填入
  ElMessage.success(`已填入 base_price：${resultText.value}`)
}

function clear() {
  reference.value = null
  multiplier.value = null
}
</script>

<style scoped>
.mc-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  row-gap: 6px;
}
.mc-field {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}
.mc-label {
  font-size: 12px;
  color: #4c5f69;
  flex-shrink: 0;
}
.mc-unit-hint {
  font-size: 12px;
  color: #9ca3af;
}
.mc-op {
  color: #6b7280;
}
.mc-result {
  font-size: 15px;
  font-weight: 600;
  color: #409eff;
  font-variant-numeric: tabular-nums;
}
.mc-actions {
  display: flex;
  gap: 8px;
  margin-top: 10px;
}
</style>
