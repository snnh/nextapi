<template>
  <span class="status-tag" :class="`is-${tone}`">
    <span class="status-dot" aria-hidden="true" />
    <span>{{ label }}</span>
  </span>
</template>

<script setup lang="ts">
type StatusTone = 'success' | 'warning' | 'danger' | 'info' | 'neutral'

withDefaults(
  defineProps<{
    label: string
    tone?: StatusTone
  }>(),
  { tone: 'neutral' },
)
</script>

<style scoped>
.status-tag {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 1px 8px;
  border: 1px solid transparent;
  border-radius: var(--nx-radius-sm);
  font-size: var(--nx-font-caption);
  line-height: 20px;
  white-space: nowrap;
}
.status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
}
.is-success {
  color: var(--nx-success);
  background: var(--nx-success-soft);
  border-color: var(--nx-success-border);
}
.is-warning {
  color: var(--nx-warning);
  background: var(--nx-warning-soft);
  border-color: var(--nx-warning-border);
}
.is-danger {
  color: var(--nx-danger);
  background: var(--nx-danger-soft);
  border-color: var(--nx-danger-border);
}
.is-info {
  color: var(--nx-info);
  background: var(--nx-info-soft);
  border-color: var(--nx-info-border);
}
.is-neutral {
  color: var(--nx-text-2);
  background: var(--nx-subtle);
  border-color: var(--nx-border);
}
</style>