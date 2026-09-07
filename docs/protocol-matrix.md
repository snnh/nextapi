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
| `ext.thinking` / `ext.web_search` / `ext.cache_control` / `ext.top_k` / `ext.service_tier` | Chat 无对应字段 |
| `usage.cache_write_tokens` | Chat 无缓存写字段 |
| 未知 role / content part 类型 / 非 function 工具 | 结构不可映射 |

### 2.2 转入 Anthropic（出口）

| 字段 | 原因 |
|------|------|
| `max_tokens` | 入口未提供时填充默认 65536（非能力缺失，但记录以便追踪） |
| `response_format` / `seed` / `n` | Anthropic 无对应字段 |
| `ext.web_search` / `ext.service_tier` | 同上 |
| `input_audio` / `file` part | Anthropic 不支持音频/文件 part |

### 2.3 转入 OpenAI Responses（出口）

| 字段 | 原因 |
|------|------|
| `n` | Responses 不支持多候选 |
| `content.input_audio` | Responses 不支持音频输入 |
| `usage.cache_write_tokens` | Responses 无缓存写字段 |
| `ext.*`（thinking/web_search/cache_control/top_k/service_tier） | Responses 无对应输入字段 |

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
| `text.verbosity` | Responses 专属 |

## 3. 流式转换要点

- 各协议流式事件经 `StreamState` 状态机转为 IR chunk（`chunk_to_ir`），再编码为目标协议事件（`chunk_from_ir`）；
- 工具调用增量按 index 对齐累积（`ir::ToolCallAggregator`）；Gemini 的 `functionCall` 需完整 `args`，聚合到合法 JSON 才发送，`finish_reason` 到达时强制冲刷；
- Anthropic 事件序列保证 `message_start → content_block_start/delta*/stop → message_delta → message_stop`；
- OpenAI 流终止载荷 `[DONE]`；Responses 补发 `response.completed`；Gemini 无终止事件；
- 转换失败可回退透传（`convert_mode=passthrough_fallback`）。

## 4. 错误体翻译

`protocol::errors`：上游错误 JSON → `IrError{status,message,type,code}` → 入口协议错误结构。
message 尽量保留上游原文；`usage_logs.error` 仅存 ≤2000 字符摘要。
