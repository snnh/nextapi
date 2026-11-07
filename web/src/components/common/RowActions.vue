<template>
  <div class="row-actions" role="group" :aria-label="label">
    <slot />
  </div>
</template>

<script setup lang="ts">
withDefaults(
  defineProps<{
    label?: string
  }>(),
  { label: '行操作' },
)
</script>

<style scoped>
.row-actions {
  display: inline-flex;
  align-items: center;
  justify-content: flex-end;
  gap: 4px;
  flex-wrap: wrap;
}
</style>