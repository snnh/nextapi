<template>
  <span class="copy-text">
    <el-tooltip :disabled="!tooltip" :content="text" placement="top" :show-after="200">
      <span class="copy-text-value mono">{{ display }}</span>
    </el-tooltip>
    <CopyButton :text="text" :label="copyLabel" />
  </span>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import CopyButton from '@/components/common/CopyButton.vue'

const props = withDefaults(
  defineProps<{
    /** 完整文本（复制内容与 tooltip 展示） */
    text: string
    /** 截断展示长度（超过则中间省略）；<=0 不截断 */
    truncate?: number
    /** hover 是否展示完整文本 tooltip */
    tooltip?: boolean
    /** 复制按钮的无障碍文案 */
    copyLabel?: string
  }>(),
  { truncate: 0, tooltip: true, copyLabel: '复制文本' },
)

const display = computed(() => {
  const t = props.text
  const n = props.truncate
  if (!n || n <= 0 || t.length <= n) return t
  const head = Math.ceil(n / 2)
  const tail = Math.floor(n / 2)
  return `${t.slice(0, head)}…${t.slice(t.length - tail)}`
})
</script>

<style scoped>
.copy-text {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  min-width: 0;
}
.copy-text-value {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>