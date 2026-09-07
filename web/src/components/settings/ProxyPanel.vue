<template>
  <div class="proxy-panel">
    <el-alert type="info" :closable="false" show-icon class="info-banner">
      <template #title>
        <div class="info-text">
          系统默认代理在「运行参数 → 代理（全局）」的 <span class="mono">proxy.default_proxy_id</span> 配置（当前
          <span class="mono">{{ defaultProxyLabel }}</span>）。外联矩阵：供应商请求 / GitHub 更新检查 / FX 拉取 /
          价格 URL 导入 / 媒体下载 的各场景 <span class="mono">use_proxy</span> 与
          <span class="mono">proxy_id</span> 在「运行参数」对应分组内配置；代理自身
          <span class="mono">no_proxy</span> 与全局 <span class="mono">proxy.no_proxy</span> 取并集，命中即直连。
        </div>
      </template>
    </el-alert>

    <div class="toolbar">
      <el-button type="primary" :icon="Plus" @click="openCreate">新建代理</el-button>
      <div class="spacer" />
      <el-button :icon="Refresh" :loading="loading" @click="load">刷新</el-button>
    </div>

    <el-card shadow="never">
      <el-table :data="proxies" empty-text="暂无代理" v-loading="loading">
        <el-table-column prop="name" label="名称" min-width="160" show-overflow-tooltip>
          <template #default="{ row }">{{ row.name }}</template>
        </el-table-column>
        <el-table-column label="类型" width="100" align="center">
          <template #default="{ row }">
            <el-tag :type="kindType(row.kind)" size="small" effect="plain">{{ row.kind }}</el-tag>
          </template>
        </el-table-column>
        <el-table-column label="地址" min-width="170">
          <template #default="{ row }">
            <span class="mono">{{ row.host }}:{{ row.port }}</span>
          </template>
        </el-table-column>
        <el-table-column label="用户名" min-width="120">
          <template #default="{ row }">
            <span v-if="row.username" class="mono">{{ row.username }}</span>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>
        <el-table-column label="密码" width="110" align="center">
          <template #default="{ row }">
            <el-tag v-if="row.has_password" type="success" size="small" effect="light">已设密码</el-tag>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>
        <el-table-column label="直连名单" min-width="180">
          <template #default="{ row }">
            <template v-if="row.no_proxy.length">
              <el-tag v-for="(t, i) in showNoProxy(row)" :key="i" size="small" style="margin-right: 4px">{{ t }}</el-tag>
              <el-tooltip v-if="row.no_proxy.length > 3" placement="top">
                <template #content>
                  <div v-for="(t, i) in row.no_proxy" :key="i" class="tip-item">{{ t }}</div>
                </template>
                <el-tag size="small" type="info" effect="plain">+{{ row.no_proxy.length - 3 }}</el-tag>
              </el-tooltip>
            </template>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>
        <el-table-column label="启用" width="80" align="center">
          <template #default="{ row }">
            <el-switch
              :model-value="row.enabled"
              :loading="toggling.has(row.id)"
              @update:model-value="(v: boolean) => toggleEnabled(row, v)"
            />
          </template>
        </el-table-column>
        <el-table-column label="操作" width="200" fixed="right">
          <template #default="{ row }">
            <el-tooltip content="探测地址为运行参数 proxy.probe_url（系统设置-运行参数-代理）" placement="top">
              <el-button size="small" :loading="testing.has(row.id)" @click="test(row)">测试</el-button>
            </el-tooltip>
            <el-button size="small" @click="openEdit(row)">编辑</el-button>
            <el-button size="small" type="danger" @click="remove(row)">删除</el-button>
          </template>
        </el-table-column>
      </el-table>
    </el-card>

    <el-dialog
      v-model="dlg.visible"
      :title="dlg.isEdit ? '编辑代理' : '新建代理'"
      width="620px"
      :close-on-click-modal="false"
      destroy-on-close
    >
      <el-form ref="formRef" :model="dlg.form" :rules="rules" label-width="90px" @submit.prevent>
        <el-form-item label="名称" prop="name">
          <el-input v-model="dlg.form.name" placeholder="代理名称（唯一）" clearable />
        </el-form-item>
        <el-form-item label="类型" prop="kind">
          <el-select v-model="dlg.form.kind" style="width: 100%">
            <el-option v-for="k in PROXY_KINDS" :key="k.value" :value="k.value" :label="k.label" />
          </el-select>
        </el-form-item>
        <el-form-item label="主机" prop="host">
          <el-input v-model="dlg.form.host" placeholder="如 proxy.example.com" clearable />
        </el-form-item>
        <el-form-item label="端口" prop="port">
          <el-input-number v-model="dlg.form.port" :min="1" :max="65535" :controls-position="'right'" style="width: 200px" />
        </el-form-item>
        <el-form-item label="用户名">
          <el-input v-model="dlg.form.username" placeholder="（可选）" clearable />
        </el-form-item>
        <el-form-item label="密码">
          <div v-if="dlg.isEdit" class="pwd-block">
            <el-input
              v-model="dlg.form.password"
              type="password"
              show-password
              :placeholder="dlg.form.hasPassword ? '留空保持不变' : '（可选）'"
              clearable
            />
            <el-checkbox v-model="dlg.form.clearPassword">清除密码（提交空串）</el-checkbox>
          </div>
          <el-input
            v-else
            v-model="dlg.form.password"
            type="password"
            show-password
            placeholder="（可选）"
            clearable
          />
        </el-form-item>
        <el-form-item label="直连名单">
          <div class="tags-editor">
            <el-tag v-for="(t, i) in dlg.form.no_proxy" :key="i" closable size="small" @close="removeTag(i)">{{ t }}</el-tag>
            <el-input v-model="noProxyInput" placeholder="域名/CIDR/*后缀 回车添加" size="small" class="tag-input" @keyup.enter="addTag" />
          </div>
          <div class="hint">与全局 proxy.no_proxy 取并集命中即直连。</div>
        </el-form-item>
        <el-form-item label="启用">
          <el-switch v-model="dlg.form.enabled" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dlg.visible = false">取消</el-button>
        <el-button type="primary" :loading="saving" @click="submit">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox, type FormInstance, type FormRules } from 'element-plus'
import { Plus, Refresh } from '@element-plus/icons-vue'
import { proxyApi } from '@/api'
import { errMsg } from '@/api/http'
import { PROXY_KINDS } from '@/utils/consts'
import type { ProxyIn, ProxyOut, SettingItem } from '@/api/types'

const props = defineProps<{ settings: SettingItem[] }>()

const proxies = ref<ProxyOut[]>([])
const loading = ref(false)
const saving = ref(false)
const toggling = reactive(new Set<string>())
const testing = reactive(new Set<string>())

const formRef = ref<FormInstance>()

function kindType(kind: string): 'primary' | 'success' | 'warning' | 'info' {
  if (kind === 'https') return 'success'
  if (kind === 'socks5') return 'warning'
  if (kind === 'http') return 'primary'
  return 'info'
}

const defaultProxyId = computed(() => {
  const it = props.settings.find((s) => s.key === 'proxy.default_proxy_id')
  return it ? String(it.value ?? '') : ''
})
const defaultProxyLabel = computed(() =>
  defaultProxyId.value ? defaultProxyId.value : '未设置（直连）',
)

async function load() {
  loading.value = true
  try {
    proxies.value = await proxyApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    loading.value = false
  }
}

function showNoProxy(row: ProxyOut): string[] {
  return row.no_proxy.slice(0, 3)
}

async function toggleEnabled(row: ProxyOut, val: boolean) {
  toggling.add(row.id)
  try {
    const body: ProxyIn = {
      name: row.name,
      host: row.host,
      kind: row.kind,
      port: row.port,
      username: row.username,
      no_proxy: [...row.no_proxy],
      enabled: val,
    }
    const updated = await proxyApi.update(row.id, body)
    Object.assign(row, updated)
    ElMessage.success(val ? '已启用' : '已禁用')
  } catch (e) {
    row.enabled = !val
    ElMessage.error(errMsg(e))
  } finally {
    toggling.delete(row.id)
  }
}

async function test(row: ProxyOut) {
  testing.add(row.id)
  try {
    const r = await proxyApi.test(row.id)
    if (r.ok) {
      const st = r.status != null ? `HTTP ${r.status}` : 'HTTP OK'
      const lat = r.latency_ms != null ? ` · ${r.latency_ms} ms` : ''
      ElMessage.success(`连通正常 ${st}${lat}`)
    } else {
      ElMessage.error(r.error || '测试失败')
    }
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    testing.delete(row.id)
  }
}

async function remove(row: ProxyOut) {
  try {
    await ElMessageBox.confirm(`确定删除代理「${row.name}」？`, '删除代理', { type: 'warning' })
  } catch {
    return
  }
  try {
    await proxyApi.remove(row.id)
    ElMessage.success('已删除')
    load()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

// —— 弹窗 ——
interface ProxyForm {
  name: string
  kind: string
  host: string
  port: number
  username: string
  password: string
  hasPassword: boolean
  clearPassword: boolean
  no_proxy: string[]
  enabled: boolean
}

function defaultForm(): ProxyForm {
  return {
    name: '',
    kind: 'http',
    host: '',
    port: 8080,
    username: '',
    password: '',
    hasPassword: false,
    clearPassword: false,
    no_proxy: [],
    enabled: true,
  }
}

const dlg = reactive({ visible: false, isEdit: false, id: '', form: defaultForm() })
const noProxyInput = ref('')

const rules: FormRules = {
  name: [{ required: true, message: '请输入代理名称', trigger: 'blur' }],
  host: [{ required: true, message: '请输入主机地址', trigger: 'blur' }],
  port: [
    { required: true, message: '请输入端口', trigger: 'blur' },
    {
      validator: (_r, v: number, cb) => {
        if (v == null || v < 1 || v > 65535) return cb(new Error('端口需在 1-65535 之间'))
        cb()
      },
      trigger: 'blur',
    },
  ],
}

function openCreate() {
  dlg.isEdit = false
  dlg.id = ''
  dlg.form = defaultForm()
  noProxyInput.value = ''
  dlg.visible = true
}

function openEdit(row: ProxyOut) {
  dlg.isEdit = true
  dlg.id = row.id
  dlg.form = {
    name: row.name,
    kind: row.kind,
    host: row.host,
    port: row.port,
    username: row.username ?? '',
    password: '',
    hasPassword: row.has_password,
    clearPassword: false,
    no_proxy: [...row.no_proxy],
    enabled: row.enabled,
  }
  noProxyInput.value = ''
  dlg.visible = true
}

function addTag() {
  const v = noProxyInput.value.trim()
  if (!v) return
  if (!dlg.form.no_proxy.includes(v)) dlg.form.no_proxy.push(v)
  noProxyInput.value = ''
}

function removeTag(idx: number) {
  dlg.form.no_proxy.splice(idx, 1)
}

function buildBody(): ProxyIn {
  const body: ProxyIn = {
    name: dlg.form.name.trim(),
    kind: dlg.form.kind,
    host: dlg.form.host.trim(),
    port: dlg.form.port,
    username: dlg.form.username.trim() || null,
    no_proxy: [...dlg.form.no_proxy],
    enabled: dlg.form.enabled,
  }
  if (dlg.isEdit) {
    if (dlg.form.clearPassword) body.password = ''
    else if (dlg.form.password && dlg.form.password !== '***') body.password = dlg.form.password
  } else if (dlg.form.password) {
    body.password = dlg.form.password
  }
  return body
}

async function submit() {
  if (!formRef.value || saving.value) return
  try {
    await formRef.value.validate()
  } catch {
    return
  }
  saving.value = true
  try {
    const body = buildBody()
    if (dlg.isEdit) {
      await proxyApi.update(dlg.id, body)
      ElMessage.success('代理已更新')
    } else {
      await proxyApi.create(body)
      ElMessage.success('代理已创建')
    }
    dlg.visible = false
    dlg.form.password = '' // 敏感字段用完即清（发布审阅前端 M3）
    load()
  } catch (e) {
    ElMessage.error(errMsg(e))
    dlg.form.password = ''
  } finally {
    saving.value = false
  }
}

load()
</script>

<style scoped>
.info-banner {
  margin-bottom: 14px;
}
.info-text {
  line-height: 1.7;
}
.info-text .mono,
.proxy-panel .mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12px;
  background: #f5f7fa;
  border-radius: 4px;
  padding: 1px 6px;
}
.toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 14px;
}
.toolbar .spacer {
  flex: 1;
}
.muted {
  color: #c0c4cc;
}
.tip-item {
  font-size: 12px;
  line-height: 1.6;
  white-space: nowrap;
}
.pwd-block {
  width: 100%;
}
.pwd-block .el-checkbox {
  margin-top: 6px;
}
.tags-editor {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  width: 100%;
}
.tag-input {
  width: 220px;
}
.hint {
  font-size: 12px;
  color: #6b7280;
  margin-top: 6px;
}
</style>
