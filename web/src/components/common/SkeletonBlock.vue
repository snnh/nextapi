<template>
  <!-- 骨架块：加载期占位，避免 v-loading 整块遮罩导致布局跳动 -->
  <span class="skeleton-block" :style="boxStyle" aria-hidden="true" />
</template>

<script setup lang="ts">
import { computed } from 'vue'

const props = withDefaults(
  defineProps<{
    /** CSS 宽度；数字按 px 处理 */
    width?: string | number
    /** CSS 高度；数字按 px 处理 */
    height?: string | number
    /** 圆角；圆形头像传 '50%' */
    radius?: string
  }>(),
  { width: '100%', height: 14, radius: 'var(--nx-radius-sm)' },
)

function toCss(v: string | number): string {
  return typeof v === 'number' ? `${v}px` : v
}

const boxStyle = computed(() => ({
  width: toCss(props.width),
  height: toCss(props.height),
  borderRadius: props.radius,
}))
</script>

<style scoped>
.skeleton-block {
  display: block;
  background: linear-gradient(
    90deg,
    var(--nx-skeleton-base) 0%,
    var(--nx-skeleton-base) 33%,
    var(--nx-skeleton-highlight) 50%,
    var(--nx-skeleton-base) 67%,
    var(--nx-skeleton-base) 100%
  );
  background-size: 300% 100%;
  animation: skeleton-sweep 1.6s ease-in-out infinite;
}

@keyframes skeleton-sweep {
  0% {
    background-position: 100% 50%;
  }
  100% {
    background-position: -100% 50%;
  }
}

/* 尊重系统「减少动态效果」：静态色块，不做扫光 */
@media (prefers-reduced-motion: reduce) {
  .skeleton-block {
    animation: none;
    background: var(--nx-skeleton-base);
  }
}
</style>