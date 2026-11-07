<template>
  <div class="page" v-loading="pageLoading">
    <!-- 页面说明与页级操作：标题由顶栏统一负责 -->
    <div class="page-head">
      <p class="page-desc">接入并管理实际提供模型的供应商（上游）。接入完成后，到「模型路由」页把这些上游绑定给对外模型。</p>
      <div class="page-head-actions">
        <el-dropdown trigger="click" @command="onAddCommand">
          <el-button type="primary" :icon="Plus">
            添加上游
            <el-icon class="el-icon--right"><ArrowDown /></el-icon>
          </el-button>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item command="preset">从预设开始</el-dropdown-item>
              <el-dropdown-item command="manual">手动配置</el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
        <el-button :icon="QuestionFilled" @click="helpVisible = true">使用说明</el-button>
      </div>
    </div>

    <!-- 上游表格 -->
    <el-card shadow="never" class="section-card">
      <template #header>
        <div class="table-head">
          <div class="table-title">
            <span>接入的上游列表</span>
            <span class="hint">{{ filteredUpstreams.length }} / {{ upstreams.length }} 个</span>
          </div>
          <div class="table-tools">
            <el-input
              v-model="search"
              class="search-input"
              clearable
              :prefix-icon="Search"
              placeholder="搜索名称 / 地址 / 类型"
            />
            <el-select v-model="statusFilter" class="status-filter" aria-label="状态筛选">
              <el-option label="全部状态" value="all" />
              <el-option label="启用中" value="enabled" />
              <el-option label="已禁用" value="disabled" />
              <el-option label="异常 / 熔断" value="error" />
              <el-option label="凭证失效" value="auth" />
            </el-select>
            <el-button :icon="Refresh" @click="loadUpstreams">刷新</el-button>
          </div>
        </div>
      </template>

      <ErrorState v-if="listError" :message="`上游列表加载失败：${listError}`" @retry="loadUpstreams" />
      <el-table v-else :data="filteredUpstreams">
        <template #empty>
          <EmptyState
            v-if="hasFilter"
            title="没有匹配的上游"
            description="试试清空搜索或切换状态筛选。"
            :icon="Search"
          >
            <template #actions>
              <el-button @click="clearFilters">清空筛选</el-button>
            </template>
          </EmptyState>
          <EmptyState
            v-else
            title="还没有接入任何上游"
            description="接入上游后，才能在「模型路由」页把模型请求转发到对应的上游。"
            :icon="Connection"
          >
            <template #actions>
              <el-button type="primary" :icon="MagicStick" @click="openWizard(null)">从预设接入</el-button>
              <el-button :icon="Plus" @click="openCreate">手动配置</el-button>
            </template>
          </EmptyState>
        </template>

        <!-- 上游：名称 + kind/协议/地址副行；窄屏把状态折到名称下方 -->
        <el-table-column prop="name" label="上游" :min-width="isNarrow ? 60 : isTablet ? 180 : 240">
          <template #default="{ row }">
            <div class="up-name">{{ row.name || '(未命名)' }}</div>
            <div v-if="!isTablet" class="meta-list up-meta">
              <span class="meta-item">{{ kindLabel(row.kind) }}</span>
              <span class="meta-item">{{ protocolSummary(row) }}</span>
              <span class="meta-item" :title="row.base_url">{{ hostOf(row.base_url) }}</span>
              <span v-if="testingId === row.id" class="meta-item">测试中…</span>
              <span v-if="capsTags(row).length" class="meta-item">{{ capsTags(row).join(' · ') }}</span>
            </div>
            <div v-else class="up-meta-compact">
              <div class="sub-line">
                <span>{{ kindLabel(row.kind) }}</span>
                <span class="mono"> · {{ hostOf(row.base_url) }}</span>
              </div>
              <div class="sub-line">
                <span
                  class="up-credential"
                  :class="{ 'is-missing': !row.has_oauth && !row.has_api_key, 'is-oauth': row.has_oauth }"
                >{{ credentialLabel(row) }}</span>
                <span v-if="testingId === row.id"> · 测试中…</span>
              </div>
            </div>
            <div v-if="isNarrow" class="mobile-status">
              <StatusTag :label="statusMeta(row).text" :tone="statusMeta(row).type" />
              <button
                v-if="row.auth_state"
                type="button"
                class="status-button"
                @click="showAuthDetail(row)"
              >
                <StatusTag :label="row.auth_blocked ? '凭证失效 · 已跳过' : '凭证失效'" tone="danger" />
              </button>
            </div>
          </template>
        </el-table-column>

        <el-table-column v-if="!isNarrow && !isTablet" label="凭证" width="100" align="center">
          <template #default="{ row }">
            <span
              class="credential"
              :class="{ 'is-missing': !row.has_oauth && !row.has_api_key, 'is-oauth': row.has_oauth }"
            >
              {{ credentialLabel(row) }}
            </span>
          </template>
        </el-table-column>

        <el-table-column v-if="!isNarrow" label="余额" width="160">
          <template #default="{ row }">
            <span
              v-if="quotaSummary(row)"
              class="quota-summary"
              :class="{ stale: quotaStale(row) }"
              :title="quotaSummaryTitle(row)"
            >
              {{ quotaSummary(row) }}
            </span>
            <span v-else class="quota-empty">—</span>
          </template>
        </el-table-column>

        <el-table-column v-if="!isNarrow" label="状态" :min-width="isTablet ? 140 : 210">
          <template #default="{ row }">
            <div class="status-cell">
              <StatusTag :label="statusMeta(row).text" :tone="statusMeta(row).type" />
              <span v-if="row.extra?.auto_disable === false" class="meta-item">不自动禁用</span>
              <button
                v-if="row.auth_state"
                type="button"
                class="status-button"
                @click="showAuthDetail(row)"
              >
                <StatusTag :label="row.auth_blocked ? '凭证失效 · 已跳过' : '凭证失效'" tone="danger" />
              </button>
            </div>
          </template>
        </el-table-column>

        <el-table-column v-if="!isNarrow && !isTablet" label="代理" width="110">
          <template #default="{ row }">{{ row.use_proxy ? proxyName(row.proxy_id) : '直连' }}</template>
        </el-table-column>

        <el-table-column label="启用" :width="isNarrow ? 70 : isTablet ? 56 : 80" align="center">
          <template #default="{ row }">
            <el-switch
              v-model="row.enabled"
              :loading="togglingSet.has(row.id)"
              :aria-label="`启用上游 ${row.name}`"
              @change="(val: boolean) => toggleEnabled(row, val)"
            />
          </template>
        </el-table-column>

        <!-- 操作列：桌面编辑 + 更多；窄屏全部收进下拉 -->
        <el-table-column
          label="操作"
          :width="isNarrow || isTablet ? 88 : 150"
          :fixed="isNarrow ? false : 'right'"
        >
          <template #default="{ row }">
            <el-dropdown
              v-if="isNarrow || isTablet"
              trigger="click"
              @command="(c: string) => onRowAction(c, row)"
            >
              <el-button size="small">
                操作<el-icon class="el-icon--right"><ArrowDown /></el-icon>
              </el-button>
              <template #dropdown>
                <el-dropdown-menu>
                  <el-dropdown-item command="edit">编辑</el-dropdown-item>
                  <el-dropdown-item v-if="row.has_api_key" command="reveal">查看 Key</el-dropdown-item>
                  <el-dropdown-item command="test">连通测试</el-dropdown-item>
                  <el-dropdown-item v-if="quotaSupported(row)" command="quota">余额/额度</el-dropdown-item>
                  <el-dropdown-item v-if="isDevin(row)" command="reauth">重新授权</el-dropdown-item>
                  <el-dropdown-item command="delete" divided style="color: var(--nx-danger)">删除</el-dropdown-item>
                </el-dropdown-menu>
              </template>
            </el-dropdown>
            <RowActions v-else label="上游操作">
              <el-button size="small" @click="openEdit(row)">编辑</el-button>
              <el-dropdown trigger="click" @command="(c: string) => onRowAction(c, row)">
                <el-button size="small">
                  更多<el-icon class="el-icon--right"><ArrowDown /></el-icon>
                </el-button>
                <template #dropdown>
                  <el-dropdown-menu>
                    <el-dropdown-item v-if="row.has_api_key" command="reveal">查看 Key</el-dropdown-item>
                    <el-dropdown-item command="test">连通测试</el-dropdown-item>
                    <el-dropdown-item v-if="quotaSupported(row)" command="quota">余额/额度</el-dropdown-item>
                    <el-dropdown-item v-if="isDevin(row)" command="reauth">重新授权</el-dropdown-item>
                    <el-dropdown-item command="delete" divided style="color: var(--nx-danger)">删除</el-dropdown-item>
                  </el-dropdown-menu>
                </template>
              </el-dropdown>
            </RowActions>
          </template>
        </el-table-column>
      </el-table>
    </el-card>

    <!-- 使用说明：从页尾技术说明移入弹窗 -->
    <el-dialog v-model="helpVisible" title="上游配置与凭证说明" class="dlg">
      <div class="help-body">
        <p>API Key 采用掩码存储，列表仅显示「已设置 / 未设置」；编辑时输入框「留空保持不变」，点「清除 Key」可移除。完整 Key 默认不可再次查看；如需查看，点行内「查看 Key」并输入管理员密码二次验证（已启用 TOTP 时还需动态码），验证通过后明文仅在弹窗内本次展示。</p>
        <p>默认值：<span class="mono">timeout_ms = 300000</span>、<span class="mono">breaker_threshold = 5</span>；图片媒体地址留空时回退 <span class="mono">base_url</span>。启用/禁用直接切换行内开关（禁用后 <span class="mono">disabled_by = manual</span>）。</p>
        <p>自动禁用（熔断）默认关闭（系统设置 <span class="mono">gateway.auto_disable_upstreams</span>）：打开后连续失败达阈值即自动禁用并冷却，成功后自动恢复；在渠道表单关闭「自动禁用」的渠道永不参与（标记「不自动禁用」）。</p>
        <p>凭证失效（如 Devin session token 过期）由网关自动识别：命中后临时跳过该上游 5 分钟并标注「凭证失效」，点「重新授权」用 Devin 账号换取新 token 即可恢复（无需手动冷却）。</p>
        <p>余额/额度为上游供应商侧只读监控，不涉及网关用户余额或扣费。DeepSeek/Kimi 可直接复用推理 Key；OpenRouter/Zenmux 需在渠道表单另填 Management Key。自定义端点支持同源相对路径或公网 HTTPS，凭证加密保存、不回传明文。</p>
      </div>
      <template #footer>
        <el-button type="primary" @click="helpVisible = false">知道了</el-button>
      </template>
    </el-dialog>

    <!-- 接入向导（选择供应商 → 填写凭证 → 连通与模型） -->
    <ProvisionWizard
      v-model="wizardVisible"
      :presets="presets"
      :initial-preset="wizardPreset"
      @saved="loadUpstreams"
      @goto-routes="goConfigureRoutes"
    />

    <!-- 额度探测（codex / devin） -->
    <QuotaDialog v-model="quotaVisible" :row="quotaRow" @probed="loadUpstreams" />

    <!-- Devin 重新授权（PKCE 换新 session token 并写回该上游） -->
    <DevinReauthDialog v-model="reauthVisible" :row="reauthRow" @done="loadUpstreams" />

    <!-- 查看明文 API Key（安全验证） -->
    <el-dialog
      v-model="revealVisible"
      :title="revealTitle"
      class="dlg"
      :close-on-click-modal="false"
      :close-on-press-escape="!revealApiKey"
      :show-close="!revealApiKey"
      destroy-on-close
      @closed="resetReveal"
    >
      <!-- 验证通过 → 明文展示（仅本次；禁止 X / Esc 关闭） -->
      <template v-if="revealApiKey">
        <div class="secret-view">
          <el-alert
            type="error"
            :closable="false"
            show-icon
            title="明文仅本次展示"
            description="已通过安全验证并解密展示，关闭后需再次验证才能查看；请勿在公共场合留存或截图。"
          />
          <div class="secret-key mono">{{ revealApiKey }}</div>
          <el-button type="primary" :icon="CopyDocument" @click="copyRevealedKey">一键复制</el-button>
        </div>
      </template>

      <!-- 安全验证表单 -->
      <el-form v-else label-width="110px" @submit.prevent @keyup.enter="submitReveal">
        <div class="hint" style="margin-bottom: 14px">
          查看上游「{{ revealRow?.name }}」的完整 API Key 属于敏感操作，需要管理员二次验证；验证通过后明文仅本次展示。
        </div>
        <el-form-item label="管理员密码">
          <el-input
            v-model="revealPassword"
            type="password"
            show-password
            autocomplete="current-password"
            placeholder="输入当前登录的管理员密码"
            clearable
          />
        </el-form-item>
        <el-form-item v-if="revealNeedTotp" label="二次验证码">
          <el-input v-model="revealTotp" maxlength="6" placeholder="认证器 6 位数字" clearable />
        </el-form-item>
        <div v-if="revealNeedTotp" class="hint" style="padding-left: 110px">
          该账号已启用 TOTP，需同时填写认证器动态码。
        </div>
      </el-form>

      <template #footer>
        <template v-if="revealApiKey">
          <el-button type="primary" @click="closeRevealSecret">我已保存，关闭</el-button>
        </template>
        <template v-else>
          <el-button @click="revealVisible = false">取消</el-button>
          <el-button type="primary" :loading="revealChecking" @click="submitReveal">验证并查看</el-button>
        </template>
      </template>
    </el-dialog>

    <!-- 新建 / 编辑弹窗 -->
    <UpstreamFormDialog
      ref="formDialogRef"
      :proxies="proxies"
      :presets="presets"
      @saved="loadUpstreams"
    />
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { ArrowDown, Connection, CopyDocument, MagicStick, Plus, QuestionFilled, Refresh, Search } from '@element-plus/icons-vue'
import { useRouter } from 'vue-router'
import { authApi, presetApi, proxyApi, upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import { copyText } from '@/utils/clipboard'
import { confirmDanger } from '@/utils/confirm'
import { PROTOCOL_LABEL } from '@/utils/consts'
import { fmtTime } from '@/utils/format'
import type { Preset, ProxyOut, UpstreamOut } from '@/api/types'
import UpstreamFormDialog from '@/components/upstreams/UpstreamFormDialog.vue'
import ProvisionWizard from '@/components/upstreams/ProvisionWizard.vue'
import QuotaDialog from '@/components/upstreams/QuotaDialog.vue'
import DevinReauthDialog from '@/components/upstreams/DevinReauthDialog.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import ErrorState from '@/components/common/ErrorState.vue'
import RowActions from '@/components/common/RowActions.vue'
import StatusTag from '@/components/common/StatusTag.vue'
import { useBreakpoint } from '@/composables/useBreakpoint'

const DISABLED_BY_LABELS: Record<string, string> = {
  manual: '手动禁用',
  breaker: '熔断',
  cooldown: '冷却中',
}

const presets = ref<Preset[]>([])
const upstreams = ref<UpstreamOut[]>([])
const proxies = ref<ProxyOut[]>([])
const pageLoading = ref(false)
/** 列表加载失败信息（非空时展示失败态 + 重试，而不是空表格） */
const listError = ref('')
const router = useRouter()
/** 接入向导可见性 + 预选供应商（null = 从「选择供应商」开始） */
const wizardVisible = ref(false)
const wizardPreset = ref<Preset | null>(null)
const togglingSet = reactive(new Set<string>())
const deletingSet = reactive(new Set<string>())
const testingId = ref('')
const helpVisible = ref(false)
/** 列表筛选：搜索 + 状态 */
const search = ref('')
type StatusFilter = 'all' | 'enabled' | 'disabled' | 'error' | 'auth'
const statusFilter = ref<StatusFilter>('all')
/** 断点：桌面 / 平板（隐藏代理列）/ 窄屏（操作收下拉） */
const { isMobile: isNarrow, isTablet } = useBreakpoint()
const formDialogRef = ref<InstanceType<typeof UpstreamFormDialog>>()

async function loadUpstreams() {
  pageLoading.value = true
  try {
    upstreams.value = await upstreamApi.list()
    listError.value = ''
  } catch (e) {
    listError.value = errMsg(e)
    ElMessage.error(errMsg(e))
  } finally {
    pageLoading.value = false
  }
}

async function loadProxies() {
  try {
    proxies.value = await proxyApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

async function loadPresets() {
  try {
    presets.value = await presetApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

onMounted(() => {
  loadUpstreams()
  loadProxies()
  loadPresets()
})

/** 窄屏操作列下拉菜单分发（桌面端仍是行内按钮） */
function onRowAction(command: string, row: UpstreamOut) {
  if (command === 'edit') openEdit(row)
  else if (command === 'reveal') openReveal(row)
  else if (command === 'test') testUpstream(row)
  else if (command === 'quota') openQuota(row)
  else if (command === 'reauth') openReauth(row)
  else if (command === 'delete') removeUpstream(row)
}

// —— 余额/额度快照（codex/devin + 通用 balance 适配器）——
const quotaVisible = ref(false)
const quotaRow = ref<UpstreamOut | null>(null)
const reauthVisible = ref(false)
const reauthRow = ref<UpstreamOut | null>(null)

/** Devin 渠道（支持 PKCE 重新授权） */
function isDevin(row: UpstreamOut): boolean {
  return row.kind === 'devin'
}

/** 凭证失效悬停详情：时间 + 错误码 + 上游原文 + 处置建议 */
function authTooltip(row: UpstreamOut): string {
  const st = row.auth_state
  if (!st) return ''
  const lines = [
    `最近一次凭证失效：${fmtTime(st.at)}`,
    `错误码：${st.code}（HTTP ${st.status}）`,
    `上游原文：${st.message}`,
  ]
  lines.push(
    row.auth_blocked
      ? '网关已临时跳过该上游（5 分钟后自动重试），请重新授权或更换凭证。'
      : '冷却已过（网关会重新尝试该上游）。',
  )
  return lines.join('\n')
}

function openReauth(row: UpstreamOut) {
  reauthRow.value = row
  reauthVisible.value = true
}

function quotaSupported(row: UpstreamOut): boolean {
  return row.balance_supported === true || row.kind === 'codex' || row.kind === 'devin'
}

function pctText(v?: number | null): string {
  if (v == null) return '—'
  return `${Number(v).toFixed(v >= 10 ? 0 : 1)}%`
}

/** 列表摘要：金额优先，多币种并列；无金额时窗口百分比兜底。 */
function quotaSummary(row: UpstreamOut): string {
  const q = row.extra?.quota
  if (!q) return ''
  if (q.kind === 'balance') {
    const balances = (q.balances ?? [])
      .filter((b) => b.available != null && b.available !== '')
      .slice(0, 2)
      .map((b) => `${b.currency} ${b.available}`)
    if (balances.length) return balances.join(' · ')
    const w = (q.windows ?? [])[0]
    if (w) return `${w.label || w.name} ${pctText(w.used_percent)}`
    return ''
  }
  if (row.kind === 'codex') {
    const p = q.primary?.used_percent
    if (p != null) return `主窗口 ${pctText(p)}`
  }
  if (row.kind === 'devin') {
    if (q.percent_value != null && q.percent_total != null) {
      return `余量 ${q.percent_value}/${q.percent_total}`
    }
    if (q.daily_quota != null) return `日额度 ${q.daily_quota}`
  }
  return ''
}

function quotaStale(row: UpstreamOut): boolean {
  const ts = row.extra?.quota?.captured_at
  if (!ts) return false
  return Date.now() / 1000 - ts > 24 * 3600
}

function quotaSummaryTitle(row: UpstreamOut): string {
  const q = row.extra?.quota
  if (!q) return ''
  const ts = q.captured_at ? new Date(q.captured_at * 1000).toLocaleString() : '时间未知'
  return `${q.source === 'probe' ? '主动探测' : '上游快照'} · ${ts}${quotaStale(row) ? '（超过 24h，可能过期）' : ''}`
}

function openQuota(row: UpstreamOut) {
  quotaRow.value = row
  quotaVisible.value = true
}

/** 能力矩阵限制标签（M10.3）：显式「不支持」的能力 + 最大上下文。 */
function capsTags(row: UpstreamOut): string[] {
  const caps = row.extra?.capabilities
  if (!caps) return []
  const tags: string[] = []
  if (caps.stream === false) tags.push('禁流式')
  if (caps.tools === false) tags.push('禁工具')
  if (caps.vision === false) tags.push('禁视觉')
  if (caps.max_context) tags.push(`上下文 ${Math.round(caps.max_context / 1024)}k`)
  return tags
}

function proxyName(proxyId: string | null): string {
  if (!proxyId) return '跟随默认'
  const p = proxies.value.find((x) => x.id === proxyId)
  return p ? p.name : proxyId
}

function statusMeta(row: UpstreamOut): { type: 'success' | 'warning' | 'danger'; text: string } {
  if (!row.enabled || row.disabled_by || row.cooldown_until) {
    let reason: string
    if (row.disabled_by) reason = DISABLED_BY_LABELS[row.disabled_by] ?? row.disabled_by
    else if (row.cooldown_until) reason = '冷却中'
    else reason = '已禁用'
    let text = reason
    if (row.cooldown_until) text += ` · 冷却至 ${fmtTime(row.cooldown_until)}`
    return { type: 'danger', text }
  }
  if (row.consecutive_failures > 0) {
    return { type: 'warning', text: `连续失败 ${row.consecutive_failures}` }
  }
  return { type: 'success', text: '启用' }
}


async function toggleEnabled(row: UpstreamOut, val: boolean) {
  togglingSet.add(row.id)
  try {
    // 只提交最小 patch，避免整行回写覆盖服务端并发修改（后端 PUT 支持字段可选）
    const updated = await upstreamApi.update(row.id, { enabled: val })
    Object.assign(row, updated)
    ElMessage.success(val ? '已启用' : '已禁用')
  } catch (e) {
    row.enabled = !val
    ElMessage.error(errMsg(e))
  } finally {
    togglingSet.delete(row.id)
  }
}

async function testUpstream(row: UpstreamOut) {
  testingId.value = row.id
  try {
    const r = await upstreamApi.test(row.id)
    if (r.ok) {
      ElMessage.success(`连通正常 HTTP ${r.status} · ${r.latency_ms}ms`)
    } else {
      ElMessage.error(r.error || '连通测试失败')
    }
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    testingId.value = ''
  }
}

async function removeUpstream(row: UpstreamOut) {
  if (deletingSet.has(row.id)) return
  const ok = await confirmDanger({
    title: '删除上游',
    message: `将删除上游「${row.name}」及其凭证配置。指向该上游的模型路由会失去可用上游（请求失败），该操作不可恢复。`,
  })
  if (!ok) return
  deletingSet.add(row.id)
  try {
    await upstreamApi.remove(row.id)
    ElMessage.success('已删除')
    loadUpstreams()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    deletingSet.delete(row.id)
  }
}

function openCreate() {
  formDialogRef.value?.open()
}

function openEdit(row: UpstreamOut) {
  formDialogRef.value?.open(row)
}

/** 空态引导 / 工具栏主操作：打开接入向导（可预选供应商） */
function openWizard(p: Preset | null) {
  wizardPreset.value = p
  wizardVisible.value = true
}

/** 向导「下一步：配置模型路由」：跳转模型页 */
function goConfigureRoutes() {
  router.push({ name: 'routes' })
}

function onAddCommand(command: string) {
  if (command === 'preset') openWizard(null)
  else if (command === 'manual') openCreate()
}

const KIND_LABEL: Record<string, string> = {
  openai: 'OpenAI',
  codex: 'Codex OAuth',
  anthropic: 'Anthropic',
  gemini: 'Gemini',
  devin: 'Devin',
  openai_compatible: 'OpenAI 兼容',
}

function kindLabel(kind: string): string {
  return KIND_LABEL[kind] ?? (kind || '未知类型')
}

function protocolSummary(row: UpstreamOut): string {
  if (!row.protocols.length) return '未配置协议'
  const labels = row.protocols.map((p) => PROTOCOL_LABEL(p))
  return labels.length > 2 ? `${labels.slice(0, 2).join(' · ')} +${labels.length - 2}` : labels.join(' · ')
}

function hostOf(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url
  }
}

function credentialLabel(row: UpstreamOut): string {
  if (row.has_oauth) return 'OAuth'
  if (row.has_api_key) return '已设置'
  return '未设置'
}

function isHealthy(row: UpstreamOut): boolean {
  return !!row.enabled && !row.disabled_by && !row.cooldown_until && !row.auth_blocked
}

const filteredUpstreams = computed(() => {
  const q = search.value.trim().toLowerCase()
  return upstreams.value.filter((row) => {
    if (q) {
      const hay = [row.name, row.base_url, row.kind].join(' ').toLowerCase()
      if (!hay.includes(q)) return false
    }
    if (statusFilter.value === 'enabled') return isHealthy(row)
    if (statusFilter.value === 'disabled') return !row.enabled || !!row.disabled_by || !!row.cooldown_until
    if (statusFilter.value === 'error') {
      return row.disabled_by === 'breaker' || row.consecutive_failures > 0 || row.auth_blocked
    }
    if (statusFilter.value === 'auth') return !!row.auth_state
    return true
  })
})

const hasFilter = computed(() => search.value.trim() !== '' || statusFilter.value !== 'all')

function clearFilters() {
  search.value = ''
  statusFilter.value = 'all'
}

function showAuthDetail(row: UpstreamOut) {
  if (!row.auth_state) return
  ElMessageBox.alert(authTooltip(row), `凭证失效详情 · ${row.name}`, {
    confirmButtonText: '知道了',
  })
}

// —— 查看明文 API Key（安全验证） ——
const revealVisible = ref(false)
const revealRow = ref<UpstreamOut | null>(null)
const revealPassword = ref('')
const revealTotp = ref('')
/** 账号是否已启用 TOTP（打开弹窗时预取，决定是否展示动态码输入框） */
const revealNeedTotp = ref(false)
const revealChecking = ref(false)
const revealApiKey = ref('')

const revealTitle = computed(() =>
  revealApiKey.value ? '完整 API Key' : '安全验证 · 查看 API Key',
)

async function openReveal(row: UpstreamOut) {
  revealRow.value = row
  revealPassword.value = ''
  revealTotp.value = ''
  revealApiKey.value = ''
  revealChecking.value = false
  revealNeedTotp.value = false
  revealVisible.value = true
  try {
    revealNeedTotp.value = (await authApi.totpStatus()).enabled
  } catch {
    // 状态预取失败不阻断：若后端要求动态码会返回 400 提示，届时补填重试
    revealNeedTotp.value = false
  }
}

function resetReveal() {
  revealRow.value = null
  revealPassword.value = ''
  revealTotp.value = ''
  revealApiKey.value = ''
  revealChecking.value = false
  revealNeedTotp.value = false
}

async function submitReveal() {
  if (!revealRow.value || revealChecking.value) return
  if (!revealPassword.value.trim()) {
    ElMessage.warning('请输入管理员密码')
    return
  }
  if (revealNeedTotp.value && !revealTotp.value.trim()) {
    ElMessage.warning('请输入二次验证码')
    return
  }
  revealChecking.value = true
  try {
    const resp = await upstreamApi.revealKey(revealRow.value.id, {
      password: revealPassword.value,
      totp_code: revealNeedTotp.value ? revealTotp.value.trim() : undefined,
    })
    revealApiKey.value = resp.api_key
  } catch (e) {
    const msg = errMsg(e)
    // 后端提示需要 TOTP（本地预取过期/状态变化）时补出动态码输入框
    if (msg.includes('TOTP') || msg.includes('验证码')) revealNeedTotp.value = true
    ElMessage.error(msg)
  } finally {
    revealChecking.value = false
  }
}

function copyRevealedKey() {
  copyText(revealApiKey.value, '已复制 API Key')
}

function closeRevealSecret() {
  revealVisible.value = false
}

</script>

<style scoped>
.table-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
}
.table-title {
  display: flex;
  align-items: baseline;
  gap: 8px;
  color: var(--nx-text);
  font-weight: 600;
}
.table-tools {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.status-filter {
  width: 140px;
}
.up-name {
  color: var(--nx-text);
  font-weight: 500;
  line-height: 1.5;
}
.up-meta {
  margin-top: 2px;
}
.up-credential {
  color: var(--nx-success);
}
.up-credential.is-oauth {
  color: var(--nx-accent);
}
.up-credential.is-missing {
  color: var(--nx-text-3);
}
.mobile-status {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  margin-top: 6px;
}
.status-cell {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}
.status-button {
  padding: 0;
  border: 0;
  background: none;
  cursor: pointer;
  line-height: 1;
}
.credential {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  color: var(--nx-success);
  font-size: var(--nx-font-caption);
  white-space: nowrap;
}
.credential::before {
  content: '';
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
}
.credential.is-oauth {
  color: var(--nx-accent);
}
.credential.is-missing {
  color: var(--nx-text-3);
}
.help-body p {
  margin: 0 0 12px;
  color: var(--nx-text-2);
  font-size: var(--nx-font-small);
  line-height: 1.7;
}
.help-body p:last-child {
  margin-bottom: 0;
}

@media (max-width: 800px) {
  .table-head {
    align-items: stretch;
  }
  .table-tools {
    width: 100%;
  }
  .status-filter {
    width: 100%;
  }
  .table-tools .el-button {
    width: 100%;
  }
}
.quota-summary {
  display: inline-block;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}
.quota-summary.stale {
  color: var(--el-text-color-placeholder);
}
.quota-empty {
  color: var(--el-text-color-placeholder);
}
</style>
