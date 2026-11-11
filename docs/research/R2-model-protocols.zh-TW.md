# R2 模型協定調查：串流、工具呼叫、錯誤與狀態

- 工作包：R2（研究）。供 C3「兩個真實協定適配器」使用
- 查核日期：2026-11-11（以下所有連結均於此日以 WebFetch 讀取官方文件；本環境無法直接 curl 供應商網域，因此是經過摘要工具讀頁，長頁面分段讀取）
- 未呼叫任何付費 API，未使用任何真實金鑰
- 標記：**[驗證]** 官方文件明載；**[推論]** 由文件結構或 SDK 原始碼推得，非文件明文；**[未驗證]** 查不到或文件語意不清

---

## 0 結論摘要

**建議第一版實作：Anthropic Messages ＋ OpenAI Chat Completions（相容協定）。**

- Chat Completions 是覆蓋面最廣的線路格式：OpenAI 本身、Gemini 的 OpenAI 相容端點（beta）、Ollama、LM Studio、vLLM、OpenRouter、DeepSeek 等都提供（見 §3.9）。一個適配器就能連到雲端與本機模型。
- Anthropic Messages 與 Chat Completions 設計明顯不同：具名 SSE 事件、型別化內容區塊、以 index 區分區塊、system 在頂層、tool_result 放在 user 訊息、max_tokens 必填、usage 分兩次送達，而且有**必須原樣回傳的 opaque 思考區塊**。用這兩種協定能真正驗證抽象，不會只做出「OpenAI 形狀的介面」。附帶好處是 Ollama、vLLM、DeepSeek、LM Studio 也提供 Anthropic 相容端點。
- OpenAI Responses API 放第三順位（v1.1）。它對 OpenAI 自家推理模型最完整（加密推理項目、`phase`），但第三方覆蓋較少，事件種類也最多。手冊 §7.2 舉例的「OpenAI 用 Responses」可延後。
- Gemini 原生 `generateContent` **現已被官方標為 legacy**（仍完整支援），官方建議新專案使用 2026 年 6 月 GA 的 **Interactions API**（新的有狀態協定）。第一版不建議實作 Gemini 原生協定；要接 Gemini 可先走 Chat Completions 相容端點，但要處理 `thought_signature`（§4.7）。

### 0.1 比較表

| 項目 | A. Anthropic Messages | B. OpenAI Responses | C. OpenAI Chat Completions | D. Gemini generateContent（legacy） |
| --- | --- | --- | --- | --- |
| 端點 | `POST https://api.anthropic.com/v1/messages` | `POST https://api.openai.com/v1/responses` | `POST {base}/v1/chat/completions` | `POST .../v1beta/models/{m}:streamGenerateContent?alt=sse` |
| 驗證 | `Authorization: Bearer` 或舊式 `x-api-key` | `Authorization: Bearer` | `Authorization: Bearer`（本機常忽略） | `x-goog-api-key`（範例亦見 `?key=`） |
| 版本標頭 | `anthropic-version` 必填（最新 `2026-10-21`） | 無 | 無 | 無（路徑 `v1beta`） |
| system | 頂層 `system` | 頂層 `instructions` 或 `developer`/`system` 訊息 | `developer`/`system` 訊息 | 頂層 `systemInstruction` |
| 助理角色 | `assistant` | `assistant`（輸出項目） | `assistant` | `model` |
| 工具結果 | user 訊息中的 `tool_result` 區塊，`tool_use_id` | `function_call_output` 項目，`call_id` | `role:"tool"` 訊息，`tool_call_id` | user 內容中的 `functionResponse` 部件，`id` |
| 輸出上限 | `max_tokens`（**必填**，含思考） | `max_output_tokens` | `max_completion_tokens`（`max_tokens` 已棄用） | `generationConfig.maxOutputTokens` |
| 結構化輸出 | `output_config.format` = `json_schema` | `text.format` = `json_schema` | `response_format` = `json_schema` | `generationConfig.responseFormat.text.schema`（REST） |
| SSE 形式 | 具名事件 `event:`＋`data:`，無 `[DONE]` | 語意事件，各帶 `sequence_number` | 只有 `data:`，以 `data: [DONE]` 結束 | 只有 `data:`，每塊是完整 `GenerateContentResponse` |
| 工具參數串流 | `input_json_delta.partial_json`，`content_block_stop` 表示完成 | `function_call_arguments.delta` → `.done` | `delta.tool_calls[i].function.arguments` 片段，**無**逐呼叫完成訊號 | 文件未說明；`functionCall.args` 以物件出現 |
| 呼叫 ID | `toolu_…`（伺服器指派） | `call_id`（另有項目 `id`） | `call_…`（本機伺服器格式不一） | `functionCall.id`（需原樣回傳） |
| 結束原因位置 | `message_delta.delta.stop_reason` | `response.completed` / `.incomplete` / `.failed` | `choices[].finish_reason` | `candidates[].finishReason` |
| 用量 | `message_start` 與 `message_delta`（**累計值**） | `response.completed` 的 `response.usage` | 需 `stream_options.include_usage`，最後一塊 `choices: []` | `usageMetadata` |
| 串流中錯誤 | `event: error` | `error` 事件／`response.failed` | 未文件化（SDK 偵測 `data` 中的 `error`） | 未文件化 |
| Opaque 狀態 | `thinking.signature`、`redacted_thinking.data` 必須原樣回傳 | 推理項目 `encrypted_content`、`phase`、`previous_response_id` | 無（OpenAI）；Gemini 相容端點有 `extra_content.google.thought_signature` | `thoughtSignature`（Gemini 3 以上缺漏回 400） |
| 列模型 | `GET /v1/models`，**含** `capabilities`、`max_input_tokens`、`max_tokens` | `GET /v1/models`，無能力資訊 | 同左（第三方多半支援 `/v1/models`） | `GET /v1beta/models`，含 `inputTokenLimit`、`outputTokenLimit`、`supportedGenerationMethods`、`thinking` |

---

## 1 A. Anthropic Messages API

主要來源（2026-11-11 查核）：
- 串流：https://platform.claude.com/docs/en/build-with-claude/streaming
- 建立訊息參考：https://platform.claude.com/docs/en/api/messages/create
- API 概觀（標頭）：https://platform.claude.com/docs/en/api/overview
- 版本：https://platform.claude.com/docs/en/api/versioning
- 錯誤：https://platform.claude.com/docs/en/api/errors
- 限流：https://platform.claude.com/docs/en/api/rate-limits
- 結束原因：https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons
- 工具：https://platform.claude.com/docs/en/agents-and-tools/tool-use/implement-tool-use 、 …/handle-tool-calls 、 …/parallel-tool-use
- 思考：https://platform.claude.com/docs/en/build-with-claude/thinking 、 …/thinking-tool-workflows
- 圖片：https://platform.claude.com/docs/en/build-with-claude/vision
- 列模型：https://platform.claude.com/docs/en/api/models/list

### 1.1 端點、標頭、請求本體

[驗證]
- `POST https://api.anthropic.com/v1/messages`
- 標頭：`Authorization: Bearer <key>`（「Yes, unless `x-api-key` is set」）或 `x-api-key`（文件稱「Legacy fallback… still supported」）；`anthropic-version` **必填**，目前最新版為 `2026-10-21`；`content-type: application/json`。Beta 功能用 `anthropic-beta`。
- **`2026-10-21` 版的破壞性變更**：送 `temperature`、`top_p`、`top_k` 任一個都回 400（所有模型）。適配器不可轉送取樣參數；若沿用 `2023-06-01` 則行為不同。建議設定檔固定版本字串，並在能力測試中驗證。
- 本體欄位：
  - `model`（必填）、`max_tokens`（**必填**；思考 token 也算在內）
  - `system`：字串或 `[{type:"text", text, cache_control?}]`
  - `messages`：`role` 為 `user`/`assistant` 交替；`content` 為字串或區塊陣列。最後一則若是 `assistant` 會被當作 prefill，在 Claude 4.6 以後的模型會回 400。
  - 圖片：`{"type":"image","source":{"type":"base64","media_type":"image/png","data":"…"}}`，另有 `source.type` 為 `url` 或 `file`（`file_id`）。支援 jpeg、png、gif、webp；單張 base64 上限 10 MB；最大 8000×8000 px。
  - 工具定義：`{name, description, input_schema, input_examples?, strict?}`；`name` 須符合 `^[a-zA-Z0-9_-]{1,128}$`。
  - `tool_choice`：`{"type":"auto"|"any"|"tool"|"none", "disable_parallel_tool_use"?: bool}`。**Opus 5.5、Sonnet 5.5、Fable／Mythos 5.x 不支援 `any`/`tool`（回 400）**，文件建議改用 `auto`＋`strict`。
  - 工具結果：在下一則 user 訊息放 `{"type":"tool_result","tool_use_id":"toolu_…","content": 字串或[text/image/document/search_result 區塊],"is_error"?:true}`。規則：
    - 結果必須緊接在對應的 tool_use 之後，中間不得有其他訊息
    - 同一則 user 訊息中 `tool_result` 必須**排在最前面**，文字放在後面
    - 平行呼叫的所有結果放在**同一則** user 訊息
  - 結構化輸出：`output_config: {"format": {"type":"json_schema","schema":{…}}, "effort"?: "low"|"medium"|"high"|"xhigh"|"max"}`。不是頂層 `output_format`。
  - `thinking`：`{"type":"adaptive"}` 或 `{"type":"enabled","budget_tokens":N}`。**Claude 5 世代預設開啟思考**，且多數 5.x 模型拒絕 `type:"disabled"` 與 `budget_tokens`（400）。顯示模式預設為 `"omitted"`：思考文字為空字串，只有 `signature`。
  - 其他：`stop_sequences`、`metadata.user_id`、`cache_control`、`service_tier`、`stream`。

### 1.2 串流

[驗證] 傳輸是 SSE。每個事件都有 `event:` 名稱行，`data:` 是 JSON，不送 `[DONE]`（`2023-06-01` 版起移除）。

事件順序：
1. `message_start`：`message` 物件，`content` 為空，含初始 `usage`
2. 每個內容區塊依序送出：`content_block_start`（含 `index`）→ 一或多個 `content_block_delta` → `content_block_stop`。`index` 等於該區塊在最終 `content` 陣列中的位置。
3. 一或多個 `message_delta`：`delta.stop_reason`、`delta.stop_sequence`、`usage`
4. `message_stop`

`ping` 可能出現在任何位置。文件要求「優雅處理未知事件類型」。

delta 類型：`text_delta.text`、`input_json_delta.partial_json`、`thinking_delta.thinking`、`signature_delta.signature`。（串流頁未列 citations 的 delta 類型。）

三種情境：
- (a) 純文字：`message_start` → text 區塊 → `message_delta(stop_reason=end_turn)` → `message_stop`。見 fixture `01-text.sse`（官方範例原文）。
- (b) 單一工具：可能先有 text 區塊，接著是 `content_block_start`，其中 `content_block` 為 `{"type":"tool_use","id":"toolu_…","name":…,"input":{}}`；然後是多個 `input_json_delta`，第一片可能是空字串；最後 `content_block_stop`，再接 `message_delta(stop_reason=tool_use)`。
- (c) 平行：同一回合有多個 `tool_use` 區塊，各有不同 `index`。[未驗證] 文件未說明不同區塊的 delta 會不會交錯；適配器一律以 `index` 為鍵累積。

參數片段 [驗證]：
- 片段是**部分 JSON 字串**，最終 `tool_use.input` 是物件。
- 應累積到 `content_block_stop` 後再解析。
- 目前模型一次吐出一組完整的 key/value，所以事件之間可能出現停頓。

完成判定：該 `index` 的 `content_block_stop`。

ID：由伺服器在 `content_block_start` 指派（`toolu_…`），回傳時放在 `tool_result.tool_use_id`。

[驗證] 若 `stop_reason` 是 `max_tokens`，且最後一個區塊是不完整的 tool_use，應提高上限重試，**不得執行**該呼叫。

### 1.3 結束原因與對應

| `stop_reason` | 文件意義 | 建議 ModelEvent |
| --- | --- | --- |
| `end_turn` | 自然結束（可能是空回應） | Completed(EndTurn) |
| `tool_use` | 呼叫工具 | Completed(ToolUse)；之前必須已送出全部 ToolCallReady |
| `max_tokens` | 達到上限 | Completed(MaxTokens)，標記截斷；不完整的工具呼叫丟棄 |
| `stop_sequence` | 命中停止序列 | Completed(StopSequence) |
| `pause_turn` | 伺服器工具迴圈達上限，需原樣送回後繼續 | Completed(Paused)（我們不用伺服器工具，理論上不會出現） |
| `refusal` | 拒答，細節在 `stop_details` | Completed(Refusal) 或 Failed(Refused)，**不可視為成功** |
| `model_context_window_exceeded` | 填滿脈絡視窗 | Completed(ContextWindowExceeded)，視為截斷 |

[驗證] `stop_reason` 只會在 `message_delta` 出現；`message_start` 中為 null。

### 1.4 用量

[驗證]
- 欄位：`input_tokens`、`output_tokens`、`cache_creation_input_tokens`、`cache_read_input_tokens`、`cache_creation.{ephemeral_5m_input_tokens, ephemeral_1h_input_tokens}`、`output_tokens_details.thinking_tokens`、`server_tool_use.*`、`service_tier`。
- 出現位置：`message_start.message.usage`（初始值），以及 `message_delta.usage`。文件說 `message_delta` 的 token 數是**累計值**。
- 輸入總量 = `cache_read_input_tokens + cache_creation_input_tokens + input_tokens`。`input_tokens` 只算最後一個快取斷點之後的部分。

[推論] 串流在中途被錯誤或斷線截斷時，不會有最終 `message_delta`，用量應記為「未知／部分」，不能記為 0。

### 1.5 錯誤

[驗證] 錯誤本體：`{"type":"error","error":{"type":…,"message":…},"request_id":"req_…"}`。回應標頭 `request-id`。

| HTTP | `error.type` | 重試 |
| --- | --- | --- |
| 400 | `invalid_request_error`（達到組織／工作區支出上限也用 400） | 否 |
| 401 | `authentication_error` | 否 |
| 402 | `billing_error` | 否 |
| 403 | `permission_error` | 否 |
| 404 | `not_found_error` | 否 |
| 409 | `conflict_error` | 視情況 |
| 413 | `request_too_large`（Messages 上限 128 MB） | 否 |
| 429 | `rate_limit_error` | 有 `retry-after` 才重試；`error.details.error_code = enforced_spend_limit_reached` 表示支出上限，**沒有 retry-after，不可重試** |
| 500 | `api_error` | 是，指數退避 |
| 504 | `timeout_error`（也可能以 500 出現） | 是 |
| 529 | `overloaded_error` | 是 |

- 限流標頭：`retry-after`（秒），以及 `anthropic-ratelimit-{requests,tokens,input-tokens,output-tokens}-{limit,remaining,reset}`；`reset` 為 RFC 3339 時間。
- 串流中錯誤 [驗證]：HTTP 200 之後可能收到 `event: error`，`data` 為 `{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}`。
- 脈絡過長 [未驗證]：輸入超過脈絡視窗時的錯誤訊息文字查無官方明文，應屬 400 `invalid_request_error`。適配器不應依賴訊息文字比對；可在請求前用 `max_input_tokens` 預估。輸出撞到視窗則是 `model_context_window_exceeded` 結束原因。
- 長請求 [驗證]：文件建議超過 10 分鐘的請求使用串流，並提醒閒置連線可能被網路設備中斷（SDK 會設 TCP keep-alive）。

### 1.6 取消

[未驗證] 文件沒有明說「關閉連線即取消、停止計費」。官方 SDK 以中止 HTTP 請求取消串流。[推論] 第一版直接關閉連線即可；已產生的 token 可能仍計費。取消後不會收到用量，應記為未知。

中斷復原 [驗證]：Claude 4.6 以後的模型要把部分輸出放進新的 user 訊息請模型接續，不能 prefill；**tool_use 與 thinking 區塊無法部分復原**。

### 1.7 Opaque 狀態

[驗證]
- `thinking` 區塊 `{type, thinking, signature}` 與 `redacted_thinking` 區塊 `{type, data}`：在工具迴圈中**必須完整、未修改地**隨 assistant 訊息回傳。「rebuilding the message or filtering out `redacted_thinking` blocks triggers a 400 error」；修改後的思考區塊也回 400。
- 規則摘要：工具回合內**必須**回傳；跨回合**建議**全部回傳；非工具情境**可以**省略之前回合的思考區塊。
- 換模型：思考區塊只有產生它的模型（與部分其他模型）能讀；API 會**忽略或丟棄**目標模型讀不懂的區塊，不報錯。
- `signature` 是加密的完整思考內容，不可解析。

對本專案的影響：
- 同供應商的工具迴圈中，適配器必須保存整個 assistant content（思考區塊＋tool_use 的順序與原文）。
- 跨供應商重建脈絡時，把思考區塊丟掉即可。從別家搬來的歷史本來就沒有思考區塊，只要它們不在「當前工具回合」就不會出錯。
- [推論] 若在 Anthropic 工具回合進行中切換到別家，再切回來，原工具回合已無法續接。手冊「只在步驟邊界切換」的規則應定義為「工具結果送回並取得模型回覆之後」。

### 1.8 列模型與能力

[驗證] `GET /v1/models`，以 `after_id`/`before_id`/`limit` 分頁，`lifecycle` 可篩選 active/deprecated/retired。

每個模型含：
- `max_input_tokens`、`max_tokens`
- `capabilities`：鍵固定包含 `batch`、`citations`、`code_execution`、`context_management`、`effort`、`image_input`、`pdf_input`、`server_tools`、`structured_outputs`、`thinking`，各有 `supported` 布林

這是四家中唯一能直接用來填 `ModelCapabilities` 的端點。注意 `capabilities` 中**沒有** `tool_calling` 或 `tool_choice` 能力鍵，仍需實測。

---

## 2 B. OpenAI Responses API

主要來源（2026-11-11 查核）：
- 串流事件參考：https://developers.openai.com/api/reference/resources/responses/streaming-events
- 建立：https://developers.openai.com/api/reference/resources/responses/methods/create
- 取得（Response 物件）：https://developers.openai.com/api/reference/resources/responses/methods/retrieve
- 函式呼叫：https://developers.openai.com/api/docs/guides/function-calling
- 串流指南：https://developers.openai.com/api/docs/guides/streaming-responses
- 推理：https://developers.openai.com/api/docs/guides/reasoning
- 對話狀態：https://developers.openai.com/api/docs/guides/conversation-state
- 錯誤碼：https://developers.openai.com/api/docs/guides/error-codes
- 限流：https://developers.openai.com/api/docs/guides/rate-limits
- 列模型：https://developers.openai.com/api/reference/resources/models/methods/list

### 2.1 請求

[驗證]
- `POST /v1/responses`，`Authorization: Bearer`
- `instructions`：system/developer 訊息；**不會**透過 `previous_response_id` 帶到下一輪
- `input`：字串，或項目陣列：
  - `{type:"message", role:"user"|"assistant"|"system"|"developer", content:[…]}`，內容部件有 `input_text`、`input_image{image_url|file_id, detail}`、`input_file`
  - `function_call{call_id, name, arguments}`
  - `function_call_output{call_id, output}`
  - 推理項目
- 工具：`{"type":"function","name","description","parameters","strict"}`，屬性在頂層，不像 Chat Completions 包在 `function` 裡
- `tool_choice`：`none` / `auto` / `required` / 指定函式 / allowed tools；`parallel_tool_calls`（預設 true）
- `max_output_tokens`
- `text.format`：`{type:"json_schema", name, schema, strict?}` 或 `json_object`
- `reasoning.effort`：`none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max`
- `reasoning.summary`：`auto`/`concise`/`detailed`
- `store`：預設保存 30 天，`false` 停用
- `previous_response_id`、`conversation`（兩者不能並用）、`include`
- `truncation` 已標記棄用

### 2.2 串流

[驗證] 每個事件 JSON 都有 `type` 與遞增的 `sequence_number`。

- 純文字：`response.created` → `response.in_progress` → `response.output_item.added`（message）→ `response.content_part.added` → 多個 `response.output_text.delta{item_id, output_index, content_index, delta}` → `response.output_text.done` → `response.content_part.done` → `response.output_item.done` → `response.completed`
  - 中間這段順序出自社群整理（https://community.openai.com/t/responses-api-streaming-the-simple-guide-to-events/1363122），非官方明文
- 工具：`response.output_item.added`，`item.type` 為 `function_call`，含 `name`、`call_id`、空的 `arguments` → `response.function_call_arguments.delta{item_id, output_index, delta}` → `response.function_call_arguments.done{arguments}`（完整字串）→ `response.output_item.done`
- 平行：每個呼叫是一個獨立輸出項目，有不同的 `output_index`。官方範例以 `output_index` 累積。
- 其他事件：`response.reasoning_summary_text.delta`、`response.refusal.delta`、`response.queued`、`response.output_text.annotation.added` 等。適配器應忽略未知類型。

完成判定：`function_call_arguments.done` 或 `output_item.done` 是**明確的逐呼叫完成訊號**，比 Chat Completions 好處理。

ID：
- `call_id` 用來配對 `function_call_output`
- 項目 `id`（例如 `fc_…`）是另一個識別碼，不要混淆

[未驗證] 線路上有沒有 `event:` 名稱行，以及有沒有 `[DONE]`，官方頁面沒有說明。社群文章說每個事件都有 `event:` 與 `data:` 兩行。建議解析時以 JSON 的 `type` 判斷事件，不依賴 `event:` 行。

### 2.3 結束狀態

[驗證]
- `Response.status`：`completed`、`failed`、`in_progress`、`cancelled`、`queued`、`incomplete`
- 串流終止事件：
  - `response.completed`
  - `response.incomplete`：`incomplete_details.reason` 為 `max_output_tokens`、`max_messages`、`content_filter`、`steered`（WebSocket）
  - `response.failed`：`response.error{code, message}`。`code` 例如 `server_error`、`rate_limit_exceeded`、`invalid_prompt`、`invalid_image*`、`image_too_large`、`bio_policy` 等
- Responses 沒有獨立的 `tool_calls` 結束原因：狀態為 completed 且輸出中有 `function_call` 項目，就表示要執行工具
- [驗證] 推理模型可能在任何可見輸出之前就 `incomplete(max_output_tokens)`，此時仍要計費

### 2.4 用量

[驗證]
- 位置：`response.usage`
- 欄位：`input_tokens`、`input_tokens_details.cached_tokens`、`input_tokens_details.cache_write_tokens`、`output_tokens`、`output_tokens_details.reasoning_tokens`、`total_tokens`
- [推論] 在 `response.completed`（或 incomplete/failed）中送達；中途 delta 不帶用量

### 2.5 錯誤

[驗證]
- 錯誤碼頁的 401、403、429、500、503 與 §3.5 共用
- 串流 `error` 事件：`{"type":"error","code":…,"message":…,"param":…,"sequence_number":…}`
- 限流標頭同 §3.5

### 2.6 取消

[未驗證] 前景串流的取消方式未查到明文。`background: true` 的回應另有取消端點（不在第一版範圍）。[推論] 關閉連線即可。

### 2.7 Opaque 狀態

[驗證]
- 「any reasoning items returned in model responses with tool calls must also be passed back with tool call outputs」：工具迴圈中推理項目必須連同 `function_call` 項目一起放回 `input`
- `store=false`（或 ZDR）是無狀態模式，推理項目預設帶 `encrypted_content`。舊的 `include: ["reasoning.encrypted_content"]` 仍被接受，但已不必要
- 「changes to the history preceding a reasoning item can cause the API to drop that reasoning」：修改前文會讓推理被丟棄
- assistant 訊息有 `phase`：`commentary` 或 `final_answer`，文件建議無狀態模式保留全部輸出項目以延續 `phase`
- `previous_response_id`／`conversation` 是伺服器端狀態

[推論] 推理項目被丟掉時，文件只說會「drop reasoning」，沒說會報錯，主要影響推理品質與快取。依手冊規定，不可只保存 `previous_response_id`。

### 2.8 列模型

[驗證] `GET /v1/models` 回傳 `{object:"list", data:[{id, object, created, owned_by, shutdown_date}]}`，**沒有能力資訊**。能力必須靠設定與實測。

---

## 3 C. OpenAI Chat Completions（相容協定）

主要來源（2026-11-11 查核）：
- 建立：https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create
- 串流 chunk：https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events
- 函式呼叫：https://developers.openai.com/api/docs/guides/function-calling
- 錯誤碼：https://developers.openai.com/api/docs/guides/error-codes
- 限流：https://developers.openai.com/api/docs/guides/rate-limits
- SDK 串流解析：https://raw.githubusercontent.com/openai/openai-python/main/src/openai/_streaming.py
- 第三方：見 §3.9

### 3.1 請求

[驗證]
- `POST {base}/chat/completions`，OpenAI 的 base 是 `https://api.openai.com/v1`
- `messages`：
  - `role` 為 `developer`、`system`、`user`、`assistant`、`tool`
  - 內容部件：`{type:"text",text}`、`{type:"image_url",image_url:{url, detail}}`（`url` 可以是 data URL）、`input_audio`、`file`
  - assistant 的 `tool_calls[]`：`{id, type:"function", function:{name, arguments(JSON 字串)}}`
  - 工具結果：`{role:"tool", tool_call_id, content}`，內容只能是文字
- `tools`：`[{type:"function", function:{name, description, parameters, strict}}]`
- `tool_choice`：`none`/`auto`/`required`/`{type:"function",function:{name}}`
- `parallel_tool_calls`：預設 true
- `max_completion_tokens`：含推理 token；`max_tokens` **已棄用**，且不相容 o 系列模型
- `response_format`：`{type:"json_schema", json_schema:{name, schema, strict}}` 或 `{type:"json_object"}`
- `stream`
- `stream_options`：`{include_usage, include_obfuscation}`
- `reasoning_effort`

### 3.2 串流

[驗證]
- 每塊都是 `chat.completion.chunk`：`{id（每塊相同）, object, created, model, service_tier?, system_fingerprint?(棄用), choices:[{index, delta:{role?, content?, refusal?, tool_calls?}, logprobs, finish_reason}], usage?, obfuscation?}`
- 以 `data: [DONE]` 結束
- [推論] 沒有 `event:` 行；SDK 遇到 `[DONE]` 即停止
- `obfuscation`：預設存在的填充欄位

三種情境：
- (a) 文字：第一塊 `delta.role="assistant"` → 多塊 `delta.content` → 一塊 `finish_reason:"stop"` 且 delta 為空 → （有 include_usage 時）`choices:[]`＋`usage` → `[DONE]`
- (b) 單一工具：第一片帶 `index`、`id`、`type`、`function.name`；之後的片段中 `id`、`name` 為 **null**，只帶 `function.arguments`；以 `index` 為鍵串接。文件明說「may be invalid JSON, so validate them before calling a function」
- (c) 平行：`tool_calls[].index` 不同。[未驗證] OpenAI 是否會在同一塊中交錯多個呼叫沒有說明（schema 允許）

**完成判定**：協定**沒有逐呼叫的完成訊號**。只能在 `finish_reason` 出現時（或串流結束時）一次確定所有呼叫。[推論] 不要用「出現更大的 index 就代表前一個完成」來提早執行，ToolCallReady 統一在 finish 後送出。

ID：
- 由伺服器指派，回傳時放在 `tool_call_id`，並且要把原 assistant 訊息連同 `tool_calls` 一起放回歷史
- [推論] 本機伺服器的 ID 格式不一，可能重複或缺漏；缺漏時適配器應自行產生，並在同一回合內保持一致

### 3.3 結束原因

[驗證] `finish_reason`：`stop`、`length`、`tool_calls`、`content_filter`、`function_call`（棄用）。

對應：
- `stop` → Completed(EndTurn)
- `tool_calls` → Completed(ToolUse)
- `length` → Completed(MaxTokens)
- `content_filter` → Completed(ContentFilter)

第三方差異：OpenRouter 將各家的值正規化成 `tool_calls`、`stop`、`length`、`content_filter`、`error`，原值放在 `native_finish_reason`。`error` 應對應 Failed。[推論] 部分本機模型回傳 `tool_calls` 時，`finish_reason` 可能是 `stop`；應以「實際有沒有累積到 tool_calls」為準。

### 3.4 用量

[驗證]
- 欄位：`prompt_tokens`、`completion_tokens`、`total_tokens`、`prompt_tokens_details.{cached_tokens, audio_tokens}`、`completion_tokens_details.{reasoning_tokens, audio_tokens, accepted_prediction_tokens, rejected_prediction_tokens}`
- 串流中**只有**設定 `stream_options.include_usage: true` 才會送：放在 `[DONE]` 前的額外一塊，`choices: []`；其他塊的 `usage` 為 null
- 「If the stream is interrupted, you may not receive this final chunk」
- OpenRouter 的最後用量塊 `choices` 不是空陣列
- 不支援 `stream_options` 的伺服器不會回報用量，只能記為未知

### 3.5 錯誤與限流

[驗證]
- 401 金鑰無效／錯誤／非組織成員／IP 未授權；403 地區不支援
- 429 有多種：
  - 請求速率（可重試，遵守 `Retry-After`）
  - `slow_down`（type `rate_limit_error`；降速）
  - **`credit_balance_exhausted`、`organization_spend_limit_exceeded`、`project_spend_limit_exceeded`、`organization_usage_limit_exceeded`：不可重試**；`error.type` 可能是 `insufficient_quota`，**要看 `error.code` 判斷**
- 500 伺服器錯誤；503 `server_is_overloaded`（type `service_unavailable_error`，遵守 `Retry-After`）
- 標頭：`x-ratelimit-{limit,remaining}-{requests,tokens}`；`x-ratelimit-reset-{requests,tokens}` 是**時間長度字串**（例如 `1s`、`6m0s`），與 Anthropic 的 RFC 3339 時間不同；`Retry-After` 為秒
- 文件明說：「Don't auto-replay streams: after output has been consumed」，即已開始輸出的串流不要自動重送

[未驗證]
- 完整的 429 本體 JSON，以及脈絡過長的錯誤碼（常見說法是 `context_length_exceeded`）在官方頁面都查不到
- 串流中錯誤：官方參考沒有文件化。[推論] 依 SDK 原始碼，`data:` JSON 中帶頂層 `error` 物件即為錯誤

### 3.6 取消

[未驗證] 未見明文。[推論] 關閉連線。Gemini 錯誤表把 499 `cancelled` 描述為「usually a disconnect」，可作為「斷線即取消」的旁證。

### 3.7 Opaque 狀態

- [驗證] OpenAI 的 Chat Completions 沒有可回傳的推理狀態欄位。[推論] 因此在 Chat Completions 上使用 OpenAI 推理模型時，工具迴圈之間的推理無法延續（可能影響品質，不會報錯）。這是將來為 OpenAI 增加 Responses 適配器的主要理由。
- [驗證] **Gemini 的相容端點例外**：tool call 上會有 `extra_content.google.thought_signature`，必須以相同方式回傳（見 §4.7）。所以適配器要能保存並回傳 tool call 上的不明擴充欄位。

### 3.8 列模型

`GET {base}/models`（見 §2.8）。Ollama 與 LM Studio 都支援 `/v1/models`。內容只有 ID，沒有能力資訊。

### 3.9 相容實作現況（2026-11-11）

| 實作 | Chat Completions | 其他相容端點 | 已知差異（文件明載） | 來源 |
| --- | --- | --- | --- | --- |
| Gemini | `https://generativelanguage.googleapis.com/v1beta/openai/`，Bearer | — | 標示 beta；`reasoning_effort` 對應思考設定（Gemini 2.5 Pro／3 無法關閉思考）；`extra_body` 可設 Gemini 專屬參數；支援串流、函式呼叫、圖片、結構化輸出、列模型；最後更新 2026-10-28 | https://ai.google.dev/gemini-api/docs/openai |
| Ollama | `http://localhost:11434/v1/`（雲端 `https://ollama.com/v1`） | `/v1/responses`（只支援無狀態）、`/v1/messages`（Anthropic 子集） | 不支援 `tool_choice`、`n`、logprobs；本機忽略金鑰但用戶端仍須帶值；其 Anthropic 端點在串流錯誤時「not sent as `error` events; failures return an HTTP status」 | https://docs.ollama.com/api/openai-compatibility 、 https://docs.ollama.com/api/anthropic-compatibility |
| vLLM | 有 | Responses、`/v1/messages`（Anthropic） | 自動工具呼叫需 `--enable-auto-tool-choice`＋`--tool-call-parser`；`hf` parser 串流時「each tool call is emitted whole」；Llama 3 不支援平行呼叫 | https://docs.vllm.ai/en/latest/serving/online_serving/ 、 https://docs.vllm.ai/en/latest/features/tool_calling/ |
| LM Studio | `http://localhost:1234/v1`（範例埠） | `/v1/responses`、Anthropic 相容頁面 | 工具與結構化輸出細節在子頁，未讀取 | https://lmstudio.ai/docs/developer/openai-compat |
| OpenRouter | `/api/v1/chat/completions` | `/api/v1/responses` | SSE 中有註解行需忽略；finish_reason 正規化；用量塊格式不同 | https://openrouter.ai/docs/api/reference/overview |
| DeepSeek | 有（頁面僅連結） | `https://api.deepseek.com/anthropic` | Anthropic 端點：`any` 當作 `auto`、忽略 `budget_tokens`、不支援 `redacted_thinking` | https://api-docs.deepseek.com/guides/anthropic_api |

---

## 4 D. Google Gemini API

主要來源（2026-11-11 查核）：
- Interactions：https://ai.google.dev/gemini-api/docs/interactions （最後更新 2026-10-15）
- Interactions 串流：https://ai.google.dev/gemini-api/docs/interactions/streaming （2026-09-23）
- generateContent 參考：https://ai.google.dev/api/generate-content
- 文字生成：https://ai.google.dev/gemini-api/docs/generate-content/text-generation （2026-10-19）
- 函式呼叫：https://ai.google.dev/gemini-api/docs/generate-content/function-calling （2026-10-15）
- 思考簽章：https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures （2026-10-13）
- 結構化輸出：https://aistudio.google.com/docs/structured-output?generate-content=true
- 錯誤：https://aistudio.google.com/docs/api-errors 、 https://ai.google.dev/gemini-api/docs/troubleshooting （2026-10-27）
- 模型：https://ai.google.dev/api/models
- 相容端點：https://ai.google.dev/gemini-api/docs/openai （2026-10-28）

### 4.0 重要現況

[驗證] Interactions API（`POST /v1beta/interactions`）的現況：
- 文件稱「Generally Available and recommended for all new projects」（2026 年 6 月）
- 「While it is now considered legacy, the original generateContent API remains fully supported」。[未驗證] 這句中的 "it" 指誰略有歧義，依上下文判斷是指 generateContent
- Interactions 預設有狀態（`store=true`，可用 `previous_interaction_id`；付費層保存 55 天），`store=false` 為無狀態
- 串流事件：`interaction.created` → 每個步驟依序 `step.start` / `step.delta` / `step.stop`（步驟之間可能有 `interaction.status_update`）→ `interaction.completed`（內含 `usage`）→ `event: done` / `data: [DONE]`
- 函式參數以 `step.delta` 的 `arguments_delta` 字串片段送出；需要工具時，互動以 `requires_action` 狀態完成
- 錯誤事件 `event_type: "error"`

官方函式呼叫主頁已改用 Interactions 寫法：`function_call` 步驟用 `id`/`name`/`arguments`，結果以 `function_result` 回傳，含 `call_id`；模式為 `auto`/`any`/`none`/`validated`，設在 `generation_config.tool_choice`。

### 4.1 generateContent 請求

[驗證]
- 串流端點：`POST https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse`，標頭 `x-goog-api-key`（部分參考範例用 `?key=`）
- 本體：`contents[]`、`tools[]`、`toolConfig`、`safetySettings[]`、`systemInstruction`、`generationConfig`、`cachedContent`、`serviceTier`、`store`
- REST 範例同時出現 `system_instruction` 與 `systemInstruction`
- `contents[].role` 為 `user`/`model` 交替；`parts[]` 可以是：
  - `text`
  - `inline_data{mime_type, data}`
  - `functionCall{id, name, args}`
  - `functionResponse{id, name, response}`
- 工具：`tools:[{functionDeclarations:[{name, description, parameters}]}]`，parameters 是 OpenAPI 子集
- 呼叫模式：`toolConfig.functionCallingConfig.mode` 為 `AUTO`/`ANY`/`NONE`/`VALIDATED`，可加 `allowedFunctionNames`
- `generationConfig`：
  - `maxOutputTokens`、`stopSequences`（最多 5 個）
  - `thinkingConfig{thinkingBudget, thinkingLevel, includeThoughts}`
  - 結構化輸出：參考頁將 `responseSchema` 標為棄用並指向 `responseFormat`；結構化輸出指南的 REST 範例使用 `responseFormat.text.{mimeType:"APPLICATION_JSON", schema}`
- [未驗證] 參考頁中 `responseJsonSchema` 的說明文字自相矛盾，欄位命名正在變動

### 4.2 串流

[驗證] SSE 每塊 `data:` 是一個完整的 `GenerateContentResponse`，含 `candidates`、`usageMetadata`、`modelVersion`、`responseId`；各塊的 `responseId` 相同。文字以增量片段送出。思考簽章可能出現在**最後一塊的空文字部件**中，所以要讀到 `finishReason` 為止。

[未驗證]
- 有沒有 `event:` 行、有沒有 `[DONE]`：官方沒有線路範例
- `functionCall` 是否一次整個送出：文件未說明；舊版行為是整個物件一次送出，但無法確認
- `usageMetadata` 是否每塊都有

### 4.3 結束原因

[驗證] `finishReason` 有：`STOP`、`MAX_TOKENS`、`SAFETY`、`RECITATION`、`LANGUAGE`、`OTHER`、`BLOCKLIST`、`PROHIBITED_CONTENT`、`SPII`、`MALFORMED_FUNCTION_CALL`、`IMAGE_*`、`NO_IMAGE`、`UNEXPECTED_TOOL_CALL`、`TOO_MANY_TOOL_CALLS`、`MISSING_THOUGHT_SIGNATURE`、`MALFORMED_RESPONSE`、`ESCALATION`、`PUP_LIMITED_DISABLED`、`CONTINUATION`。

- **沒有 `TOOL_CALLS` 這個值**：有函式呼叫時 `finishReason` 是 `STOP`，要從 parts 判斷
- 提示被擋時看 `promptFeedback.blockReason`：`SAFETY`、`OTHER`、`BLOCKLIST`、`PROHIBITED_CONTENT`、`IMAGE_SAFETY`

### 4.4 用量

[驗證] `usageMetadata`：`promptTokenCount`、`cachedContentTokenCount`、`candidatesTokenCount`、`toolUsePromptTokenCount`、`thoughtsTokenCount`、`totalTokenCount`，以及各 `*Details`。總量 = 提示＋工具提示＋思考＋候選。

### 4.5 錯誤

[驗證]
- 新錯誤頁（AI Studio）列 snake_case 錯誤碼，本體為 `{"error":{"code":"invalid_request","message":…}}`：
  - `invalid_request` 400、`failed_precondition` 400、`authentication` 401、`payment_required` 402、`permission_denied` 403、`model_not_found` 404
  - `rate_limit_exceeded`/`quota_exceeded`/`too_many_requests` 429
  - `cancelled` 499、`api_error` 500、`service_unavailable` 503、`deadline_exceeded` 504 等
- 疑難排解頁：429/503 用指數退避加 jitter；400/402/403 不重試

[未驗證]
- 這個本體格式是否也適用 generateContent：傳統上 generateContent 使用 google.rpc 格式 `{"error":{"code":429,"message":…,"status":"RESOURCE_EXHAUSTED","details":[…]}}`，可能帶 `RetryInfo.retryDelay`，但本次沒有讀到官方明文
- 有沒有 `Retry-After` 標頭
- 脈絡過長的錯誤碼

### 4.6 取消

[驗證] 錯誤表的 499 `cancelled` 描述為「usually a disconnect」。[推論] 關閉連線即可。

### 4.7 Opaque 狀態（思考簽章）

[驗證]
- `thoughtSignature` 可能出現在 `functionCall`、`text` 部件，或沒有函式呼叫時的最後一個部件
- **平行呼叫只有第一個 `functionCall` 帶簽章**
- Gemini 3 與 4：當前回合的函式呼叫缺少簽章會回 **400**；文字部件不嚴格驗證
- 必須「pass it back exactly as received」，放在原本的部件裡
- 不能交錯排列成「FC1+簽章, FR1, FC2, FR2」，否則 400
- 注入別處來的函式呼叫（例如從其他模型搬來的歷史）可用假簽章跳過驗證：`"skip_thought_signature_validator"` 或 `"context_engineering_is_the_way_to_go"`
- 相容端點：放在 tool call 的 `extra_content.google.thought_signature`
- `finishReason` 有 `MISSING_THOUGHT_SIGNATURE`
- Interactions 無狀態模式下必須原樣回傳所有 thought 步驟（`signature` 必填）

對本專案：**跨供應商重建脈絡時，如果把含工具呼叫的歷史送到 Gemini 3 以上，而且這些呼叫落在「當前回合」，就必須填入假簽章，否則請求直接被拒。** 這是手冊目前沒有涵蓋的失敗模式。

### 4.8 列模型

[驗證] `GET /v1beta/models`（`pageSize` 最大 1000、`pageToken`）。模型欄位有：`name`、`baseModelId`、`version`、`displayName`、`description`、`inputTokenLimit`、`outputTokenLimit`、`supportedGenerationMethods`、`thinking`、`temperature`、`maxTemperature`、`topP`、`topK`。相容端點也有 `GET /models`。

---

## 5 建議：第一版兩個協定

**Anthropic Messages ＋ OpenAI Chat Completions。** 理由：

1. **覆蓋面**：Chat Completions 一個適配器能連到 OpenAI、Gemini（相容，beta）、Ollama、LM Studio、vLLM、OpenRouter、DeepSeek 等，符合「可選擇串接多家 API」與本機模型需求。Anthropic Messages 除了 Claude 本身，Ollama、vLLM、DeepSeek、LM Studio 也提供相容端點。
2. **真正不同的兩種設計**：
   - 具名事件＋型別化區塊＋明確的區塊結束 vs. 匿名 chunk＋按 index 合併＋只有全域結束
   - system 在頂層 vs. 放在訊息中
   - 工具結果放在 user 區塊（必須排最前） vs. 獨立的 `tool` 角色
   - `max_tokens` 必填 vs. 選填
   - 用量分兩次送、累計值 vs. 選擇性最後一塊
   - 必須原樣回傳的 opaque 思考區塊 vs. 完全沒有狀態
   
   手冊 §14 的「工具參數分片、畸形 JSON、重複事件、限流、用量缺漏、中途斷線」在這兩個協定上都能用 fixture 涵蓋，而且失敗形式不同。
3. **工作量**：兩者的事件模型最小。Responses 有 50 種以上的事件，Gemini 原生正在轉向 Interactions，現在投入 generateContent 會很快過時。
4. **風險說明**：
   - 用 Chat Completions 接 OpenAI 推理模型時，工具迴圈之間的推理無法延續（§3.7）
   - Gemini 相容端點仍是 beta，且需要處理 `extra_content` 簽章
   - 第二版建議依序加入 OpenAI Responses、Gemini Interactions

替代方案：若使用者主要用 OpenAI 推理模型，可改成 Anthropic ＋ Responses，但會失去本機與多數第三方的覆蓋（Ollama、vLLM、LM Studio 雖然有 Responses，但都是子集）。

---

## 6 統一契約必須吸收的差異

| # | 差異 | 適配器責任 |
| --- | --- | --- |
| 1 | system 位置：頂層 `system` / `instructions` / `developer` 訊息 / `systemInstruction` | `ModelRequest.instructions` 由適配器放到正確位置 |
| 2 | 角色名：`assistant` vs `model`；工具結果的角色：`user` 區塊 vs `tool` 訊息 vs `user` 部件 | 依協定轉換；Anthropic 與 Gemini 需合併連續的同角色訊息、保持交替 |
| 3 | 工具結果排列：Anthropic 要求同一則 user 訊息、`tool_result` 排最前；Chat 每個結果一則 `tool` 訊息；Gemini 不可交錯 | 由「一次 assistant 工具回合 → 一組結果」的結構產生，不逐筆附加 |
| 4 | 工具定義形狀：`input_schema` / 頂層 `parameters` / `function.parameters` / `functionDeclarations`（OpenAPI 子集） | 共用 JSON Schema，適配器包裝；Gemini 需降轉 schema 子集（第二版） |
| 5 | 參數片段完成訊號：`content_block_stop` / `arguments.done` / 只有全域 `finish_reason` / 可能一次整個送出 | ToolCallReady 統一在「該呼叫確定完成」時送出；Chat 一律在 finish 時送出 |
| 6 | 片段鍵：Anthropic `index`（區塊位置，含文字與思考）/ Chat `tool_calls[].index`（僅工具）/ Responses `output_index`＋`item_id` | 適配器內部對照到穩定的呼叫 ID |
| 7 | 結束原因：工具呼叫有專用值（`tool_use`、`tool_calls`）或沒有（Responses、Gemini 的 `STOP`） | 依「是否有完整工具呼叫」判斷 ToolUse |
| 8 | 截斷與拒答：`max_tokens`、`length`、`incomplete`、`MAX_TOKENS`；`refusal`、`content_filter`、`SAFETY`… | 契約需要正式列舉，不可混成 Completed 成功 |
| 9 | 用量語意：Anthropic 累計值且送兩次；Chat 選擇性只送一次；快取欄位名與語意不同（Anthropic 的 `input_tokens` **不含**快取讀取；OpenAI 的 `cached_tokens` 是 prompt 的**子集**） | 正規化為「總輸入、其中快取讀取、快取寫入、輸出、其中推理」，每欄可為未知 |
| 10 | 限流資訊：`retry-after` 秒；reset 時間格式為 RFC 3339 或時間長度字串；支出上限型 429 不可重試 | Failed 需帶 retry_after 與 retryable |
| 11 | 串流中錯誤：`event: error` / `error` 事件 / `data` 帶 `error` / OpenRouter 的 `finish_reason:"error"` / Ollama 的 Anthropic 端點直接用 HTTP 狀態 | 全部歸成 Failed，並標記是否已輸出部分內容 |
| 12 | Opaque 狀態：Anthropic 思考區塊（必須回傳）、Responses 推理項目（必須回傳）、Gemini 思考簽章（缺漏回 400，可用假值） | 見 §7 |
| 13 | 必填參數：Anthropic `max_tokens` 必填；`2026-10-21` 版禁止取樣參數 | `output_budget` 不可為空；不轉送 temperature |
| 14 | 圖片：base64 / URL / 檔案 ID；Ollama 的 Anthropic 端點不支援 URL；Chat 的工具結果只能是文字 | 圖片參照在適配器內轉成 base64；工具結果中的圖片需要降級策略 |
| 15 | 本機伺服器的工具呼叫 ID 可能缺漏或格式不同；`tool_choice` 可能不支援（Ollama） | 缺 ID 時由適配器產生；能力宣告 tool_choice |

---

## 7 手冊契約（§7.1、§7.2）不足之處

1. **ModelEvent 缺少的事件**
   - `ReasoningDelta`（或 `ThinkingDelta`）：用於顯示思考摘要（Anthropic `thinking_delta`、Responses `reasoning_summary_text.delta`、Gemini `thought` 部件）。不顯示也行，但必須能區分，不能混進 TextDelta。
   - `ProviderState` / `OpaqueContinuation`：適配器在串流中收到的 opaque 資料（`signature`、`redacted_thinking.data`、`encrypted_content`、`thoughtSignature`、`phase`），要能交給協調器**隨該 assistant 回合**保存。手冊說「隔離保存於該適配器」，但適配器是無狀態的串流函式，協調器必須知道它的存在與位置（排在哪個 tool_use 之前），否則下一次請求無法原樣重建。
   - `ResponseStarted{provider_response_id, model_actually_used, request_id}`：用於任務紀錄與客服追查（`request-id`、`x-request-id`、`responseId`）。
   - `Refusal`：可以是 Completed 的一個 stop reason，但需要能帶出拒答文字（Responses 的 `refusal.delta`）。
2. **Completed 需要帶 `stop_reason` 列舉**：EndTurn、ToolUse、MaxTokens、StopSequence、ContextWindowExceeded、Refusal、ContentFilter、Paused、Other(raw)。截斷不是失敗，但也不能當作正常完成。
3. **Failed 需要錯誤分類與屬性**：
   - kind：auth、permission、billing/quota（不重試）、rate_limited、overloaded、context_too_long、invalid_request、unsupported_capability、server、timeout、network_disconnect、cancelled、safety_blocked、malformed_stream
   - retryable、retry_after、http_status、provider_code、request_id
   - **partial_output_emitted**：OpenAI 文件明說已輸出的串流不要自動重送，這會影響 §7.4 的重試政策
4. **Usage 語意未定義**：需要規定「快照或增量」（建議：每個 Usage 事件都是累計快照，最後一個為準）、每個欄位都是 `Option`（未知不能當 0；手冊 §7.3 已有原則，契約型別也要反映）、快取讀取／快取寫入／推理 token 各自分開。
5. **ModelRequest 缺少的欄位**
   - `tool_choice`（auto/none/required/指定）與 `parallel_tool_calls`
   - `stop_sequences`
   - `reasoning`（effort/思考開關/摘要顯示）：Claude 5 世代預設開啟思考，而且算在 `max_tokens` 內
   - `messages` 每個 assistant 回合需要可選的 `provider_state` 欄位，加上「來源供應商與模型」標記。適配器據此決定：同供應商就回傳，不同就丟棄；Gemini 需要時填假簽章
   - 工具結果需要 `is_error` 與內容型別（文字/圖片）
   - `metadata.user_id`（選用）
6. **ModelCapabilities 不足**：手冊只列 text、streaming、tool_calling、structured_output、image_input、context_limit。建議增加：
   - `max_output_tokens`
   - `parallel_tool_calls`
   - `tool_choice_modes`（例如 Opus 5.5 不支援 `any`/`tool`，Ollama 不支援 `tool_choice`）
   - `reasoning`、`requires_opaque_roundtrip`
   - `usage_in_stream`（是否可靠回報用量）
   - `image_input_sources`（base64/url）
   - `streams_tool_args`（逐片 vs 一次整個；只影響 UI）
7. **取消的語意**：未規定取消時是否送 `Failed(cancelled)` 或直接結束串流；建議一律送 `Failed(kind=cancelled)`，用量記為未知。
8. **deadline 未區分**：需要分成「整體期限」與「事件閒置逾時」。Anthropic 會送 `ping`，可據此設定閒置逾時；其他協定沒有心跳。
9. **步驟邊界的定義**：§7.4「只在明確步驟邊界切換」需要補充：Anthropic 與 Gemini 的工具回合在結果送回、模型回應之前不可中斷換家（opaque 狀態綁在該回合）。

---

## 8 Fixtures

位置：`docs/research/fixtures/`。每個檔案開頭都以註解標示「COPIED FROM DOCUMENTATION」或「CONSTRUCTED」，並附來源網址與查核日期。

- `.sse` 檔的標頭用 SSE 註解行（以 `:` 開頭），合規的 SSE 解析器會忽略，可以直接回放。
- `.http` 檔中，`# ---` 以上是說明，回放工具需要剝除；以下是原始 HTTP 回應。
- 行尾使用 `\n`。實際串流可能是 `\r\n`，測試應兩種都跑。

| 檔案 | 內容 | 來源性質 |
| --- | --- | --- |
| `anthropic-messages/01-text.sse` | 純文字 | 官方範例原文 |
| `anthropic-messages/02-tool-call-split.sse` | 單一工具，參數 7 片（含空片、字串中斷） | 前段原文，後段依 schema 補全 |
| `anthropic-messages/03-parallel-tools.sse` | 兩個 tool_use 區塊，含快取用量 | 依 schema 建構 |
| `anthropic-messages/04-midstream-error.sse` | 200 之後 `event: error`，工具參數未完成 | 建構；error 事件為原文 |
| `anthropic-messages/05-rate-limit-429.http` | 429＋`retry-after`＋`anthropic-ratelimit-*` | 依文件欄位建構 |
| `anthropic-messages/06-thinking-then-tool.sse` | （加做）思考區塊（omitted＋signature）後接工具 | 依 schema 建構，簽章為假值 |
| `openai-chat-completions/01-text.sse` | 純文字＋include_usage＋`[DONE]`＋`obfuscation` | 依 schema 建構 |
| `openai-chat-completions/02-tool-call-split.sse` | 單一工具，參數 5 片，後續片的 id/name 為 null | 依文件描述建構 |
| `openai-chat-completions/03-parallel-tools.sse` | 前置文字＋兩個呼叫，同一塊內含兩個 index | 建構（交錯情況 OpenAI 是否會發生未驗證） |
| `openai-chat-completions/04-midstream-error.sse` | `data` 中帶 `error`、無 `[DONE]` | 依 SDK 原始碼推論建構 |
| `openai-chat-completions/05-rate-limit-429.http` | 429＋`x-ratelimit-*`（時間長度格式）＋`retry-after` | 標頭依文件；本體的 type/code 為假設 |
| `openai-chat-completions/06-local-whole-call-no-usage.sse` | （加做）本機伺服器：工具呼叫一次整個送出、無用量 | 依 vLLM、Ollama 文件建構 |

已用腳本確認：所有 `data:` 都是合法 JSON；完整的 fixture 中，參數片段串接後都能解析成預期物件；兩個 04 檔的參數刻意停在半截。

**建議補做的負面案例**（可由現有檔案變形，C3 可自行產生）：
- 重複事件：同一 `data:` 行重送；Responses 可用 `sequence_number` 去重
- 畸形 JSON：最後一片少了 `}`
- 不帶 `[DONE]` 的 EOF
- `\r\n` 行尾
- 單一 `data:` 跨多行
- 未知事件類型

---

## 9 未能驗證或文件不明確的項目

1. OpenAI Responses 的 SSE 線路格式：有沒有 `event:` 行、有沒有 `[DONE]`。官方頁面只有語意事件；有 `event:` 行的說法只出自社群文章。
2. OpenAI Chat Completions 串流中錯誤的形狀：官方未文件化，fixture 依 SDK 原始碼推論。
3. OpenAI 請求速率型 429 的完整 JSON 本體，以及脈絡過長的錯誤碼：官方頁面未列出。
4. Anthropic 輸入超過脈絡視窗時的錯誤訊息；平行 tool_use 區塊的 delta 會不會交錯。
5. 三家都沒有明文說「關閉連線＝取消且停止計費」；只有 Gemini 的 499 描述可以旁證。
6. Gemini generateContent：
   - 串流的線路細節（`event:` 行、`[DONE]`、`functionCall` 是否一次整個送出、每塊是否有 `usageMetadata`）
   - 錯誤本體是 google.rpc 格式還是新的 snake_case 格式，以及是否有 `Retry-After`/`RetryInfo`
   - 結構化輸出欄位名稱正在變動（`responseSchema` 棄用 → `responseFormat`，`responseJsonSchema` 的說明自相矛盾）
   - 「legacy」一句中的 "it" 略有歧義
7. LM Studio 的工具與結構化輸出子頁，以及 vLLM 的 `--api-key`：未讀取。
8. 所有網頁都經過摘要工具讀取（本環境的代理禁止 curl 直接連到供應商網域）。引號內文字是工具回傳的原文摘錄，長頁面為分段讀取，可能漏讀未出現在摘錄中的段落。實作前若發現矛盾，以官方頁面為準。
