<template>
  <div class="table-pager">
    <el-pagination
      v-model:current-page="page"
      v-model:page-size="size"
      :total="total"
      :page-sizes="pageSizes"
      :layout="pagerLayout"
      :small="isMobile"
      :pager-count="isMobile ? 5 : 7"
      background
    />
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { useBreakpoint } from '@/composables/useBreakpoint'

const props = withDefaults(
  defineProps<{
    page: number
    size: number
    total: number
    pageSizes?: number[]
    layout?: string
  }>(),
  {
    pageSizes: () => [10, 20, 50, 100],
    layout: 'total, sizes, prev, pager, next, jumper',
  },
)

const emit = defineEmits<{
  'update:page': [value: number]
  'update:size': [value: number]
}>()

const { isMobile } = useBreakpoint()
const pagerLayout = computed(() =>
  isMobile.value ? 'prev, pager, next' : props.layout,
)

const page = computed({
  get: () => props.page,
  set: (value: number) => emit('update:page', value),
})
const size = computed({
  get: () => props.size,
  set: (value: number) => emit('update:size', value),
})
</script>

<style scoped>
.table-pager {
  display: flex;
  justify-content: flex-end;
  margin-top: 16px;
}
@media (max-width: 800px) {
  .table-pager {
    justify-content: center;
    overflow-x: auto;
    padding-bottom: 2px;
  }
}
</style>