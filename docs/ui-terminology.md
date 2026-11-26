# 前端术语统一（M21 / 批次 4）

> 状态：清单待确认（2026-11）。确认后按本表批量替换，只改**用户可见文案**。
> 背景：同一概念全站至少两套叫法（密钥 47 / API Key 20；上游 136 / 渠道 46），用户要在同一个产品里反复学同一个词。

## 1. 术语表（唯一叫法）

| 概念 | 统一叫法 | 不再使用 | 说明 |
|---|---|---|---|
| 网关签发给调用方的凭据 | **密钥** | API 密钥（作页名引用时） | `/keys` 页、调用示例、日志筛选 |
| 上游供应商下的一条接入配置 | **上游** | 渠道、供应商、Provider | 侧栏已是「上游供应商」 |
| 上游侧用于鉴权的凭据 | **API Key**（保留英文） | 上游密钥、凭证 | 表单 label 与提示语；该词通用易懂，不做翻译 |
| 对外模型名到上游的映射 | **路由** | 规则、转发规则（混用） | `/models?tab=routes` |
| 请求记录 | **请求日志** | 日志（单说时）、logs | `/usage` 第二段 |
| 用量与成本汇总 | **用量统计** | 统计、报表、Stats | `/usage` 第一段 |

保留不改：代码标识符（`api_key`、`upstream`）、后端 API 字段、代码注释里的技术说明、协议名（OpenAI / Anthropic / Codex / Devin）。

## 2. 替换清单

### 2.1 API Key（保留原文，不改）

用户拍板：`API Key` 是通用叫法，全站保持英文，不翻译成「API 密钥」「上游密钥」「凭证」。

唯一例外已执行：`components/models/ModelDetailDrawer.vue:165` 的「API 密钥」页 → 「密钥」页（引用的是侧栏页名，页名本身已是「密钥」）。

### 2.2 渠道 → 上游（25 处）

| 文件:行 | 现状 | 建议 |
|---|---|---|
| `components/settings/fields.ts:256` | 渠道自动禁用 | 上游自动禁用 |
| `components/settings/fields.ts:259` | 不自动禁用任何渠道 … 单个渠道可在渠道表单里 | 不自动禁用任何上游 … 单个上游可在上游表单里 |
| `components/settings/fields.ts:445` | model_sync=auto 的渠道定时拉取 | model_sync=auto 的上游定时拉取 |
| `components/settings/fields.ts:514` | 渠道模型同步 | 上游模型同步 |
| `components/settings/RuntimeParamsPanel.vue:562` | 解除全部历史「自动禁用」渠道 | 解除全部历史「自动禁用」上游 |
| `components/settings/RuntimeParamsPanel.vue:563` | 关闭渠道自动禁用？ | 关闭上游自动禁用？ |
| `components/upstreams/ProvisionWizard.vue:77` | token 将随渠道一起加密保存 … 先建渠道 | token 将随上游一起加密保存 … 先建上游 |
| `components/upstreams/ProvisionWizard.vue:184` | 点击该渠道的「Devin 授权」 | 点击该上游的「Devin 授权」 |
| `components/upstreams/ProvisionWizard.vue:191` | 未授权渠道不会参与路由 | 未授权上游不会参与路由 |
| `components/upstreams/ProvisionWizard.vue:215` | 该渠道为「自动同步」 | 该上游为「自动同步」 |
| `components/upstreams/ProvisionWizard.vue:228` | 该渠道为「手动同步」 | 该上游为「手动同步」 |
| `components/upstreams/QuotaDialog.vue:11` | 该渠道未配置余额/额度查询。可在渠道编辑中 | 该上游未配置余额/额度查询。可在上游编辑中 |
| `components/upstreams/QuotaDialog.vue:222` | 这两个渠道需要 Management Key | 这两个上游需要 Management Key |
| `components/upstreams/QuotaDialog.vue:224` | 失败只提示，不影响渠道 | 失败只提示，不影响上游 |
| `components/upstreams/QuotaDialog.vue:228` | 相对路径走渠道同源 | 相对路径走上游同源 |
| `components/upstreams/UpstreamFormDialog.vue:314` | 自动（按渠道类型识别） | 自动（按上游类型识别） |
| `components/upstreams/UpstreamFormDialog.vue:337` | 拼到渠道 origin | 拼到上游 origin |
| `components/upstreams/UpstreamFormDialog.vue:449` | 手动路由（含其他渠道）绝不受影响 | 手动路由（含其他上游）绝不受影响 |
| `components/upstreams/UpstreamFormDialog.vue:490` | 已路由（本渠道） | 已路由（本上游） |
| `components/upstreams/UpstreamFormDialog.vue:526` | 查看该渠道可用模型 | 查看该上游可用模型 |
| `components/upstreams/UpstreamFormDialog.vue:621-622` | 还需在系统设置里打开「渠道自动禁用」… 该渠道永不自动禁用 | 还需在系统设置里打开「上游自动禁用」… 该上游永不自动禁用 |
| `components/upstreams/UpstreamFormDialog.vue:785` | 他渠道占用 | 其他上游占用 |
| `components/upstreams/UpstreamFormDialog.vue:1039-1041` | 已有本渠道手动路由 / 其他渠道路由占用 | 已有本上游手动路由 / 其他上游路由占用 |
| `components/upstreams/DevinReauthDialog.vue:15` | 直接写回该渠道 | 直接写回该上游 |
| `views/ModelsView.vue:45` | 实际提供模型的渠道与额度 | 实际提供模型的上游与额度 |

### 2.3 上游类型描述里的「渠道」（保留）

用户拍板：`UpstreamFormDialog.vue:798-800` 的「OpenAI 兼容接口（多数国产 / 聚合渠道）」「Codex OAuth 渠道」「Devin OAuth 渠道」保留「渠道」——此处指**接入形态**（一个供应商可有多种形态），与「一条接入配置」的上游不是同一层次。

代码注释里的「渠道」（`api/types.ts`、组件内 `//` 注释）一律保留。

## 3. 执行方式

1. 按 2.2 执行替换（2.1 已定：不改），一次 commit 只做术语，便于回滚。
2. 只替换中文文案与 `label` / `placeholder` / `title` / `aria-label` / `ElMessage` 字符串，不动代码标识符与注释。
3. 替换后跑：`npx vue-tsc --noEmit`、`npm run build`；人工抽查 4 路由（用量与日志 / 模型接入 / 密钥 / 系统设置）。
4. 设置页 label 来自 `fields.ts` 的前端覆盖（后端键名不变），改 label 不影响 API 与 DB。

## 4. 影响面

- 纯文案，无逻辑变更；设置页搜索按 label/help 实时匹配（`SETTING_SCENARIOS` 只按 key 分组），替换后搜「渠道」不再命中，属预期结果。
- 若有外部文档/截图引用旧术语，需同步更新（`docs/`、`README`、`UPDATING` 等）。