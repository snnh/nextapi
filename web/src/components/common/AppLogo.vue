<template>
  <svg
    class="app-logo"
    :class="{ 'app-logo-mono': monochrome }"
    xmlns="http://www.w3.org/2000/svg"
    :width="size"
    :height="size"
    viewBox="0 0 64 64"
    fill="none"
    aria-hidden="true"
    focusable="false"
  >
    <!-- 浅色 API 尖括号 + 小型几何 N，组成 <N> -->
    <rect x="2" y="2" width="60" height="60" rx="18" fill="var(--logo-bg)" />
    <path
      d="M8 12H14L20 6H28M36 58H44L50 52H56"
      stroke="var(--logo-traces)"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    />
    <path
      d="M17 19L8 32L17 45M47 19L56 32L47 45"
      stroke="var(--logo-pattern)"
      stroke-width="4"
      stroke-linecap="round"
      stroke-linejoin="round"
    />
    <g stroke-width="6" stroke-linecap="round" stroke-linejoin="round">
      <path d="M23 44V20M41 44V20" stroke="var(--logo-n)" />
      <path d="M23 20L41 44" stroke="var(--logo-next)" />
    </g>
  </svg>
</template>

<script setup lang="ts">
withDefaults(defineProps<{ size?: number; monochrome?: boolean }>(), {
  size: 32,
  monochrome: false,
})
</script>

<style scoped>
.app-logo {
  --logo-bg: #f0f5f1;
  --logo-traces: #e1eae3;
  --logo-pattern: #a9bbb0;
  --logo-n: #1b231e;
  --logo-next: #35a65f;
}

.app-logo-mono {
  --logo-bg: #f1f3f1;
  --logo-traces: #e2e5e2;
  --logo-pattern: #afb6b0;
  --logo-n: currentColor;
  --logo-next: color-mix(in srgb, currentColor 58%, transparent);
}

:global(html.dark) .app-logo {
  --logo-bg: #262b27;
  --logo-traces: #313832;
  --logo-pattern: #5c6760;
  --logo-n: currentColor;
  --logo-next: color-mix(in srgb, currentColor 62%, transparent);
}
</style>
