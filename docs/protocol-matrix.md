# 协议转换矩阵与不可映射能力清单

> 实现见 `src/protocol/`（IR 枢轴 + 四适配器 + SSE + 错误翻译）。
> 双向转换一律走 IR（N→1→N），降级项在转换时收集进 `ConvCtx`，由网关写入
> warning 日志与 `X-NextAPI-Degraded` 响应头。

## 1. 字段映射矩阵

| 能力 | OpenAI Chat | OpenAI Responses | Anthropic Messages | Gemini |
|------|-------------|------------------|--------------------|--------|
| 系统提示 | `messages[role=system]` | `instructions` | 顶层 `system`（字符串/block 数组） | `systemInstruction.parts[].text` |
| 用户文本 | `content: string/parts` | `input[message]` | content block `text` | `parts[text]` |
| 图片（URL） | `image_url` | `input_image` | `image(source.type=url)` | `file_data{file_uri}`（降级：Gemini 建议内联） |
| 图片（内联） | `image_url` data: URL | `input_image` base64 | `image(source.base64)` | `inline_data{mime_type,data}` |
| 工具定义 | `tools[type=function]` | `tools`（平铺 function） | `tools[{name,input_schema}]` | `tools[functionDeclarations]` |
| 工具调用 | `tool_calls[{id,function}]` | `function_call{call_id,...}` | content block `tool_use` | `parts[functionCall]` |
| 工具结果 | `messages[role=tool]` | `function_call_output` | content block `tool_result` | `parts[functionResponse]` |
| 思考/推理 | `reasoning_content` | `reasoning` item | `thinking` block（signature 原样携带） | `parts[thought:true]` |
| 思考强度 | `reasoning_effort` | `reasoning.effort` | `output_config.effort` | `generationConfig.thinkingConfig.thinkingLevel` |
| 输出详略 | `verbosity` | `text.verbosity` | —（降级） | —（降级） |
| 思考开关 | `thinking{type}` | 同上（`reasoning` 块） | `thinking{type: enabled/disabled/adaptive}` | `generationConfig.thinkingConfig` |
| 工具调用签名 | `thought_signature`（扩展字段） | `thought_signature`（扩展字段） | —（降级） | `parts[].thoughtSignature` |
| max_tokens | `max_tokens`/`max_completion_tokens` | `max_output_tokens` | `max_tokens`（必填，缺省填 65536） | `generationConfig.maxOutputTokens` |
| 停止序列 | `stop` | —（降级） | `stop_sequences` | `generationConfig.stopSequences` |
| top_k | —（降级） | —（降级） | `top_k` | `generationConfig.topK` |
| usage | `prompt/completion_tokens`（+details.cached） | `input/output_tokens`（+details） | `input/output/cache_read_input/cache_creation_input` | `usageMetadata` |
| 流式 | SSE `chat.completion.chunk` + `[DONE]` | SSE `response.*` 事件 | SSE `message_*`/`content_block_*` 事件 | SSE `data:` 增量块（同响应结构） |
| 停止原因 | `finish_reason` | `status: completed/incomplete` | `stop_reason` | `finishReason` |

finish_reason 归一化为 OpenAI 词汇（`stop/length/tool_calls/content_filter`），原始值保留在
`IrResponse.extra`（如 `anthropic_stop_reason`、`gemini_finish_reason`）。

## 2. 不可映射能力清单（降级项）

转换时按字段逐一判定，三级策略：直接映射 → 降级 + 记录 → 扩展透传。
以下为当前实现会记录降级的字段（`ConvCtx.degrade`）：

### 2.1 转入 OpenAI Chat（出口）

| 字段 | 原因 |
|------|------|
| `ext.thinking` | 仅 `thinking{type: enabled/disabled}` 形状下发；其它形状（Anthropic `adaptive` 等）降级 |
| `ext.web_search` / `ext.cache_control` / `ext.top_k` / `ext.service_tier` | Chat 无对应字段 |
| `usage.cache_write_tokens` | Chat 无缓存写字段 |
| 未知 role / content part 类型 / 非 function 工具 | 结构不可映射 |

### 2.2 转入 Anthropic（出口）

| 字段 | 原因 |
|------|------|
| `max_tokens` | 入口未提供时填充默认 65536（非能力缺失，但记录以便追踪） |
| `response_format` / `seed` / `n` | Anthropic 无对应字段 |
| `ext.web_search` / `ext.service_tier` / `ext.verbosity` | 同上 |
| `ext.thinking`（非 Anthropic 形状） | Gemini `thinkingConfig` 等其它协议形状不下发 |
| `input_audio` / `file` part | Anthropic 不支持音频/文件 part |

### 2.3 转入 OpenAI Responses（出口）

| 字段 | 原因 |
|------|------|
| `n` | Responses 不支持多候选 |
| `content.input_audio` | Responses 不支持音频输入 |
| `usage.cache_write_tokens` | Responses 无缓存写字段 |
| `ext.*`（thinking/web_search/cache_control/top_k/service_tier） | Responses 无对应输入字段 |
| `ext.thinking` 非 Anthropic 形状 | Anthropic 出站会校验 `type` 取值，其它协议的 thinking 配置不发 |

### 2.4 转入 Gemini（出口）

| 字段 | 原因 |
|------|------|
| `image_url` | Gemini 需内联数据；以 `file_data{file_uri}` 携带并降级（受控下载转内联由 `media_download` 开关控制） |
| `tool_response_name` | `functionResponse` 需要函数名，缺失时用 `tool_call_id` 代替 |
| `tool_choice` / `seed` / `n` / `user` | Gemini 无对应字段 |
| `usage.cache_write_tokens` | Gemini 无对应 |
| `input_audio` | Gemini 不支持（音频模型后续扩展） |

### 2.5 从 OpenAI Responses 转出（入口降级）

| 字段 | 原因 |
|------|------|
| `previous_response_id` / `store` / `include` / `background` | Responses 有状态能力，无法映射到无状态协议 |
| `input.reasoning` item | 推理 item 无法回放为消息 |

### 2.6 思考强度与输出详略

- 思考强度**不做档位换算**：入口写什么值，就按目标协议的字段名原样回写（`high` 不会变成 `medium`）；
- 输出详略只在 OpenAI 两协议之间互通（`verbosity` ↔ `text.verbosity`），转出到 Anthropic/Gemini 记降级；
- 各协议对取值集合的校验交给上游：网关不裁剪、不补默认值；
- 工具调用签名（Gemini `thoughtSignature`）在 OpenAI 两协议以扩展字段 `thought_signature` 携带，客户端原样回传即可续接 Gemini 工具循环；Anthropic 无槽位，记录降级。

### 2.7 入口专有字段的跨协议隔离

入口协议的未知顶层字段原先会原样铺到目标请求体，这对 Anthropic/Gemini 是**非法请求**（未知字段直接 400）。
现在只有目标协议与字段来源一致时才回写，跨协议一律丢弃并记降级（`X-NextAPI-Degraded`）。

## 3. 渠道参数整形（`upstreams.extra.params`）

同一协议下的不同渠道对参数形状的接受度不同，按官方文档逐渠道声明覆盖（未声明 = 不改写）：

| 声明 | 作用 | 典型渠道 |
|------|------|----------|
| `thinking_style: enable_thinking` | 用私有开关 `enable_thinking: bool` 替代 `thinking{type}` | 阿里云百炼 Qwen 系 |
| `thinking_style: effort_only` | 删除 `thinking` 块，只留思考强度 | Kimi K3 |
| `thinking_style: fixed` | 思考恒开，强制 `thinking{type: enabled}` | GLM-5.3 |
| `thinking_style: extended` + `thinking_budget` | Anthropic 旧形状 `thinking{type: enabled, budget_tokens}` | 旧版扩展思考 |
| `thinking_style: adaptive` | Anthropic 新形状 `thinking{type: adaptive}` | Claude Opus 5.x |
| `thinking_style: none` | 删除全部思考参数 | 不支持思考的渠道 |
| `effort_field: <点路径>` | 思考强度改投到渠道字段（默认按协议规范落点） | 私有 `reasoning_effort` 变体 |
| `strip: [<点路径>…]` | 剔除渠道不接受的字段 | Gemini 3.6+ 采样参数 |
| `max_tokens_field` | `max_tokens` ↔ `max_completion_tokens` | MiniMax 等新字段渠道 |
| `include_thoughts` | Gemini `thinkingConfig.includeThoughts` | 需要回传思考内容的渠道 |

整形在 `upstream::params::apply`，透传与协议转换两条路径都生效；执行顺序为
渠道 body 覆盖（`extra.overrides`）→ 参数整形。

### 3.1 Gemini 3.6+ 的模型级规则

- 禁止末轮 `model` 预填（400）：跨协议带入的 assistant 预填会被丢弃并记降级；
- `temperature` / `top_p` / `top_k` 已弃用（当前被忽略、后续版本报错）：新代模型不再下发；
- 思考档位不做换算，原样透传：Gemini 4 系（`gemini-4-*`）只认 `low/medium/high/xhigh`，
  客户端传 `minimal` 时由上游返回 400，网关不代改档位；
- Gemini 4 系的 `frequency_penalty` / `presence_penalty` / `candidate_count` 一律 400，
  IR 不承载这三个字段，转换时不会下发。

## 4. 流式转换要点

- 各协议流式事件经 `StreamState` 状态机转为 IR chunk（`chunk_to_ir`），再编码为目标协议事件（`chunk_from_ir`）；
- 工具调用增量按 index 对齐累积（各协议 `StreamState` 自行累积，`ir::ToolCallAggregator` 仅供单测校验）；Gemini 的 `functionCall` 需完整 `args`，聚合到合法 JSON 才发送，`finish_reason` 到达时强制冲刷；
- Anthropic 事件序列保证 `message_start → content_block_start/delta*/stop → message_delta → message_stop`；
- OpenAI 流终止载荷 `[DONE]`；Responses 补发 `response.completed`；Gemini 无终止事件；
- 转换失败不回退透传：直接转移到下一候选上游（`convert_mode` 取值见 `logging::LogEvent`：`none|passthrough|convert|media_adapt|media_task`）。

## 5. 错误体翻译

`protocol::errors`：上游错误 JSON → `IrError{status,message,type,code}` → 入口协议错误结构。
message 尽量保留上游原文；`usage_logs.error` 仅存 ≤2000 字符摘要。
