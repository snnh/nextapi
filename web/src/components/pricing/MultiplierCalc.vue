<template>
  <div class="multiplier-calc">
    <div class="hint" style="margin-bottom: 10px">
      纯前端试算，后端只存直接价，无倍率体系。输入「参考单价」与「倍率」，实时计算直接价，可一键填入 base_price。
    </div>
    <div class="mc-row">
      <el-input-number v-model="reference" :min="0" :precision="6" :controls="false" placeholder="参考单价" />
      <span class="mc-op">×</span>
      <el-input-number v-model="multiplier" :min="0" :precision="4" :controls="false" placeholder="倍率" />
      <span class="mc-op">＝</span>
      <span class="mc-result">{{ resultText }}</span>
      <el-button size="small" type="primary" :disabled="!canFill" @click="fill">填入 base_price</el-button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'

const emit = defineEmits<{ (e: 'fill', v: number): void }>()

const reference = ref<number | null>(null)
const multiplier = ref<number | null>(null)

const result = computed<number | null>(() => {
  const a = reference.value
  const b = multiplier.value
  if (a === null || b === null) return null
  return a * b
})

const resultText = computed(() =>
  result.value === null ? '—' : trimNum(result.value),
)

const canFill = computed(() => result.value !== null)

function trimNum(n: number): string {
  // 保留至多 10 位小数，去掉末尾 0
  return Number(n.toFixed(10)).toString()
}

function fill() {
  if (result.value === null) return
  emit('fill', result.value)
}
</script>

<style scoped>
.mc-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
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
</style>
