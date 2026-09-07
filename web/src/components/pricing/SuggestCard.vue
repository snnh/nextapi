<template>
  <div class="suggest-card">
    <div class="suggest-head">
      <el-icon><MagicStick /></el-icon>
      <span>猜你想用</span>
      <span class="hint">参考其它上游对该模型的定价，一键复制所需字段（需保存后生效）</span>
    </div>

    <div v-if="loading" class="hint">正在生成建议…</div>

    <!-- 加载失败：与真空态区分，给出错误提示与重试入口 -->
    <div v-else-if="loadError" class="suggest-error">
      <span class="hint">建议加载失败，请稍后重试</span>
      <el-link type="primary" :underline="false" @click="load">重试</el-link>
    </div>

    <div v-else-if="groups.length === 0" class="hint">
      {{ ready ? '暂无相似上游定价可参考' : '请先选择上游并填写 model_id 后再生成建议' }}
    </div>

    <div v-else class="suggest-groups">
      <div v-for="g in visibleGroups" :key="g.upstream_id" class="suggest-group">
        <div class="suggest-upstream">
          <span class="suggest-u-name" :class="{ 'is-other': g.upstream_id !== currentUpstreamId }">
            {{ g.upstream_name }}
          </span>
          <el-tag v-if="g.upstream_id !== currentUpstreamId" size="small" type="warning" effect="plain">其他上游</el-tag>
          <span class="hint">{{ fmtTime(g.updated_at) }}</span>
        </div>
        <div v-for="rule in g.rules" :key="rule.id" class="suggest-rule">
          <div class="suggest-rule-info">
            <span class="suggest-unit">{{ UNIT_LABEL(rule.unit) }}</span>
            <el-tag size="small" type="info">{{ rule.currency }}</el-tag>
            <span class="suggest-price">单价 {{ fmtMoney(rule.base_price) }}</span>
            <span v-if="rule.segments && rule.segments.length" class="suggest-segs">共 {{ rule.segments.length }} 段</span>
            <span v-else class="suggest-no-seg">无分段</span>
          </div>
          <!-- 采用：同上游规则置灰并说明原因 -->
          <span v-if="!canAdopt(rule)" class="adopt-anchor">
            <el-tooltip content="该规则已属于当前上游，无需采用" placement="top">
              <span class="adopt-anchor">
                <el-button size="small" type="primary" plain disabled>采用</el-button>
              </span>
            </el-tooltip>
          </span>
          <el-button v-else size="small" type="primary" plain @click="adopt(rule)">采用</el-button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { MagicStick } from '@element-plus/icons-vue'
import { pricingApi } from '@/api'
import { UNIT_LABEL } from '@/utils/consts'
import { fmtMoney, fmtTime } from '@/utils/format'
import type { PriceRuleRow, SuggestGroup } from '@/api/types'

const props = defineProps<{
  upstream: string
  model: string
  currentUpstreamId: string
}>()
const emit = defineEmits<{ (e: 'adopt', rule: PriceRuleRow): void }>()

const groups = ref<SuggestGroup[]>([])
const loading = ref(false)
// 加载失败（非真空态）：展示错误与「重试」，不再静默伪装成「暂无参考」
const loadError = ref(false)

const ready = computed(() => !!props.upstream && !!props.model)

const visibleGroups = computed<SuggestGroup[]>(() =>
  groups.value.filter((g) => g.upstream_id !== props.currentUpstreamId),
)

let timer: ReturnType<typeof setTimeout> | null = null

watch(
  () => [props.upstream, props.model],
  () => {
    if (timer) clearTimeout(timer)
    timer = setTimeout(load, 300)
  },
  { immediate: true },
)

// 请求序号：300ms 防抖之外仍可能先发慢返回覆盖后发结果（发布审阅前端 M2）
let loadSeq = 0

async function load() {
  const seq = ++loadSeq
  if (!props.upstream || !props.model) {
    groups.value = []
    loadError.value = false
    return
  }
  loading.value = true
  try {
    const resp = await pricingApi.suggest(props.upstream, props.model)
    if (seq !== loadSeq) return
    groups.value = resp.items ?? []
    loadError.value = false
  } catch {
    if (seq !== loadSeq) return
    groups.value = []
    loadError.value = true
  } finally {
    if (seq === loadSeq) loading.value = false
  }
}

function canAdopt(rule: PriceRuleRow): boolean {
  // 已是当前上游自己的规则时无需采用
  return rule.upstream_id !== props.currentUpstreamId
}

function adopt(rule: PriceRuleRow) {
  emit('adopt', rule)
}
</script>

<style scoped>
.suggest-card {
  border: 1px dashed #d9ecff;
  border-radius: 8px;
  padding: 12px;
  background: #f0f9ff;
}
.suggest-head {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 10px;
  font-size: 13px;
  font-weight: 600;
  color: #1f2329;
}
.suggest-error {
  display: flex;
  align-items: center;
  gap: 8px;
}
.adopt-anchor {
  display: inline-block;
}
.suggest-groups {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.suggest-group {
  border: 1px solid #ebeef5;
  border-radius: 6px;
  padding: 8px 10px;
  background: #fff;
}
.suggest-upstream {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 6px;
}
.suggest-u-name {
  font-size: 13px;
  font-weight: 600;
}
.suggest-u-name.is-other {
  color: #e6a23c;
}
.suggest-rule {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 6px 0;
  border-top: 1px dashed #f0f0f0;
}
.suggest-rule-info {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.suggest-unit {
  font-size: 12px;
}
.suggest-price {
  font-size: 12px;
  color: #409eff;
}
.suggest-segs {
  font-size: 12px;
  color: #6b7280;
}
.suggest-no-seg {
  font-size: 12px;
  color: #c0c4cc;
}
</style>
