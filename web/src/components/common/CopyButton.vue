<template>
  <el-tooltip :content="copied ? '已复制' : label" placement="top">
    <el-button
      class="copy-button"
      link
      :icon="copied ? Check : CopyDocument"
      :aria-label="copied ? '已复制' : label"
      @click.stop="onCopy"
    />
  </el-tooltip>
</template>

<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue'
import { Check, CopyDocument } from '@element-plus/icons-vue'
import { copyText } from '@/utils/clipboard'

const props = withDefaults(
  defineProps<{
    text: string
    /** 无障碍与 tooltip 文案，例如「复制 request_id」 */
    label?: string
  }>(),
  { label: '复制' },
)

const copied = ref(false)
let timer: number | undefined

async function onCopy() {
  const ok = await copyText(props.text, '已复制', { toast: false })
  if (!ok) return
  copied.value = true
  if (timer) window.clearTimeout(timer)
  timer = window.setTimeout(() => {
    copied.value = false
  }, 1500)
}

onBeforeUnmount(() => {
  if (timer) window.clearTimeout(timer)
})
</script>

<style scoped>
.copy-button {
  flex-shrink: 0;
  width: 28px;
  height: 28px;
  padding: 0;
  color: var(--nx-text-3);
  border-radius: var(--nx-radius-sm);
}
.copy-button:hover {
  color: var(--nx-text);
  background: var(--nx-hover);
}
@media (max-width: 800px) {
  .copy-button {
    width: 36px;
    height: 36px;
  }
}
</style>