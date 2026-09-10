<template>
  <div class="page" v-loading="pageLoading">
    <div class="toolbar">
      <el-button type="primary" :icon="Plus" @click="openCreate">新建别名</el-button>
      <el-button :icon="Refresh" @click="load">刷新</el-button>
      <el-input
        v-model="keyword"
        placeholder="搜索别名 / 实际模型"
        clearable
        :prefix-icon="Search"
        style="width: 240px"
      />
      <div class="spacer" />
      <div class="hint" style="max-width: 620px">
        客户端可用别名请求模型，网关按实际模型路由、计费。白名单按实际模型判定；日志保留入口别名与实际模型。
      </div>
    </div>

    <el-card shadow="never">
      <div v-if="listError" class="error-state">
        <p class="error-msg">别名列表加载失败：{{ listError }}</p>
        <el-button type="primary" :icon="Refresh" @click="load">重新加载</el-button>
      </div>
      <template v-else>
      <el-table :data="pagedItems" :row-key="(row: ModelAliasRow) => row.id" border>
        <template #empty>
          <el-empty :description="emptyText" />
        </template>

        <el-table-column label="别名" min-width="220">
          <template #default="{ row }">
            <CopyText :text="row.alias" :truncate="24" />
          </template>
        </el-table-column>
        <el-table-column label="实际模型" min-width="220">
          <template #default="{ row }">
            <CopyText :text="row.model" :truncate="24" />
          </template>
        </el-table-column>
        <el-table-column label="启用" width="90" align="center">
          <template #default="{ row }">
            <el-switch
              v-model="row.enabled"
              :loading="togglingSet.has(row.id)"
              @change="(val: boolean) => toggleEnabled(row, val)"
            />
          </template>
        </el-table-column>
        <el-table-column label="创建时间" width="170">
          <template #default="{ row }">{{ fmtTime(row.created_at) }}</template>
        </el-table-column>
        <el-table-column label="操作" width="150" fixed="right">
          <template #default="{ row }">
            <el-button link type="primary" @click="openEdit(row)">编辑</el-button>
            <el-button
              link
              type="danger"
              :loading="deletingSet.has(row.id)"
              :disabled="deletingSet.has(row.id)"
              @click="remove(row)"
            >
              删除
            </el-button>
          </template>
        </el-table-column>
      </el-table>
      <div class="pagination">
        <el-pagination
          v-model:current-page="currentPage"
          v-model:page-size="pageSize"
          :total="filteredItems.length"
          :page-sizes="[10, 20, 50]"
          layout="total, sizes, prev, pager, next"
        />
      </div>
      </template>
    </el-card>

    <el-dialog
      v-model="dlg.visible"
      :title="dlg.isEdit ? '编辑别名' : '新建别名'"
      class="dlg"
      :close-on-click-modal="false"
      destroy-on-close
      :before-close="beforeClose"
    >
      <el-form
        ref="formRef"
        :model="dlg.form"
        :rules="rules"
        label-width="90px"
        @submit.prevent
        @keyup.enter="onFormKeyEnter"
      >
        <el-form-item label="别名" prop="alias">
          <el-input v-model="dlg.form.alias" placeholder="客户端请求使用的模型名，如 fast-gpt" />
        </el-form-item>
        <el-form-item label="实际模型" prop="model">
          <el-input v-model="dlg.form.model" placeholder="路由匹配使用的真实模型 ID，如 gpt-4o-mini" />
        </el-form-item>
        <el-form-item label="启用">
          <el-switch v-model="dlg.form.enabled" />
        </el-form-item>
        <div class="hint">
          别名不能与非通配路由的模型名同名，也不能指向另一个别名（不支持链式）。
        </div>
      </el-form>
      <template #footer>
        <el-button @click="onCancel">取消</el-button>
        <el-button type="primary" :loading="saving" @click="save">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onMounted, reactive, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { ElMessage, type FormInstance, type FormRules } from 'element-plus'
import { Plus, Refresh, Search } from '@element-plus/icons-vue'
import { aliasApi } from '@/api'
import { errMsg } from '@/api/http'
import { confirmDanger } from '@/utils/confirm'
import CopyText from '@/components/common/CopyText.vue'
import { useDirtyGuard } from '@/composables/useDirtyGuard'
import type { AliasIn, ModelAliasRow } from '@/api/types'
import { fmtTime } from '@/utils/format'

const items = ref<ModelAliasRow[]>([])
const pageLoading = ref(false)
/** 列表加载失败信息（非空时展示失败态 + 重试，而不是空表格） */
const listError = ref('')
const route = useRoute()
const saving = ref(false)
const togglingSet = reactive(new Set<string>())
const deletingSet = reactive(new Set<string>())

const formRef = ref<FormInstance>()
const dlg = reactive({
  visible: false,
  isEdit: false,
  id: '',
  form: { alias: '', model: '', enabled: true } as Required<AliasIn>,
})

const rules: FormRules = {
  alias: [{ required: true, message: '请输入别名', trigger: 'blur' }],
  model: [{ required: true, message: '请输入实际模型', trigger: 'blur' }],
}

// —— 搜索与客户端分页 ——
const keyword = ref('')
const currentPage = ref(1)
const pageSize = ref(10)

/** 按别名 / 实际模型模糊过滤（不区分大小写） */
const filteredItems = computed<ModelAliasRow[]>(() => {
  const kw = keyword.value.trim().toLowerCase()
  if (!kw) return items.value
  return items.value.filter(
    (r) => r.alias.toLowerCase().includes(kw) || r.model.toLowerCase().includes(kw),
  )
})

/** 当前页数据（客户端分页） */
const pagedItems = computed<ModelAliasRow[]>(() => {
  const start = (currentPage.value - 1) * pageSize.value
  return filteredItems.value.slice(start, start + pageSize.value)
})

const emptyText = computed(() =>
  keyword.value.trim()
    ? '未找到匹配的别名，请调整搜索关键词'
    : '暂无别名，点击左上角「新建别名」创建',
)

// 关键词变化时回到第一页
watch(keyword, () => {
  currentPage.value = 1
})

// 数据量 / 每页条数变化后，页码越界时收敛到末页
watch(
  () => [filteredItems.value.length, pageSize.value],
  () => {
    const max = Math.max(1, Math.ceil(filteredItems.value.length / pageSize.value))
    if (currentPage.value > max) currentPage.value = max
  },
)

// —— 脏表单守卫：表单值变化需二次确认才关闭 ——
const dirtyGuard = useDirtyGuard(() => dlg.form)

async function load() {
  pageLoading.value = true
  try {
    items.value = await aliasApi.list()
    listError.value = ''
  } catch (e) {
    listError.value = errMsg(e)
    ElMessage.error(errMsg(e))
  } finally {
    pageLoading.value = false
  }
}

function openCreate() {
  dlg.isEdit = false
  dlg.id = ''
  dlg.form = { alias: '', model: '', enabled: true }
  dlg.visible = true
  dirtyGuard.snapshot()
  nextTick(() => formRef.value?.clearValidate())
}

function openEdit(row: ModelAliasRow) {
  dlg.isEdit = true
  dlg.id = row.id
  dlg.form = { alias: row.alias, model: row.model, enabled: row.enabled }
  dlg.visible = true
  dirtyGuard.snapshot()
  nextTick(() => formRef.value?.clearValidate())
}

async function save() {
  if (saving.value || !formRef.value) return
  try {
    await formRef.value.validate()
  } catch {
    return
  }
  saving.value = true
  try {
    if (dlg.isEdit) {
      await aliasApi.update(dlg.id, dlg.form)
    } else {
      await aliasApi.create(dlg.form)
    }
    ElMessage.success('已保存')
    dirtyGuard.disarm()
    dlg.visible = false
    await load()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    saving.value = false
  }
}

/** 点 X / 按 Esc / 点遮罩时：脏则弹确认再关闭（遮罩已由 close-on-click-modal 关闭） */
function beforeClose(done: () => void) {
  dirtyGuard.confirmClose(done)
}

/** 取消按钮：脏则弹确认再关闭 */
function onCancel() {
  dirtyGuard.confirmThen(() => {
    dlg.visible = false
  })
}

/** 表单内按 Enter 保存；排除 el-select 选择与 textarea 换行等输入场景 */
function onFormKeyEnter(e: KeyboardEvent) {
  const target = e.target as HTMLElement | null
  if (!target) return
  if (target.closest('.el-select')) return
  if (target.tagName === 'TEXTAREA') return
  save()
}

async function toggleEnabled(row: ModelAliasRow, val: boolean) {
  togglingSet.add(row.id)
  try {
    await aliasApi.update(row.id, { alias: row.alias, model: row.model, enabled: val })
    ElMessage.success(val ? '已启用' : '已停用')
  } catch (e) {
    row.enabled = !val
    ElMessage.error(errMsg(e))
  } finally {
    togglingSet.delete(row.id)
  }
}

async function remove(row: ModelAliasRow) {
  // 行级 loading 防重入：确认与请求期间该行删除按钮均禁用
  deletingSet.add(row.id)
  try {
    const ok = await confirmDanger({
      title: '删除别名',
      message: `将删除别名「${row.alias}」→ 实际模型「${row.model}」的映射。使用该别名的客户端请求会立即失败（可改用实际模型名），该操作不可恢复。`,
    })
    if (!ok) return
    await aliasApi.remove(row.id)
    ElMessage.success('已删除')
    await load()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    deletingSet.delete(row.id)
  }
}

onMounted(() => {
  // 支持从模型详情跳转带入模型名（/aliases?model=xxx），自动过滤该模型的别名
  const qModel = typeof route.query.model === 'string' ? route.query.model.trim() : ''
  if (qModel) keyword.value = qModel
  load()
})
</script>

<style scoped>
.pagination {
  margin-top: 14px;
  display: flex;
  justify-content: flex-end;
}
.error-state {
  padding: 30px 16px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 14px;
}
.error-msg {
  margin: 0;
  color: #f56c6c;
  font-size: 13px;
  text-align: center;
  word-break: break-all;
}
</style>
