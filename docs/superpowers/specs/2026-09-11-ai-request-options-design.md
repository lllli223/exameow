# AI 请求高级选项 · 四端打通 · 原生取消

日期：2026-09-11
状态：已评审（brainstorming 设计通过；用户选择范围 C）

## 背景

上游 PR #3（`stors789:feat/advanced-ai-settings`）提供了推理模型兼容参数，但与 main 已独立落地的
“长文本出题 + `custom_prompt` + `AIConfig.max_tokens`”实现模型冲突，直接合并会产生两套
`max_tokens`/prompt 真相源、`ConfigView` 类型断裂、原生端持久化丢失等问题。本设计把 #3 中
**main 尚无的独有能力**用 main 的模型干净重写，并补齐原生取消。

参照 #3 的能力清单与本仓库现状，确认「范围 C」＝ 推理兼容包 + 重试/超时 + 原生取消。

## 目标

1. 支持推理模型所需的参数：
   - 思考强度 `reasoning_effort`（`minimal|low|medium|high|xhigh|max`，或 `none` 关闭）
   - 输出上限字段可选 `max_tokens` / `max_completion_tokens`
   - `temperature` 可配置，或选择“不发送”
2. 全局“补充 Prompt”应用到**出题、答题、批改、解析**四类任务。
3. HTTP 层自动重试（仅瞬时错误）与可配置超时。
4. Tauri 原生取消：前端 abort 时真正中止 Rust 端在途 AI 请求。
5. 保持向后兼容：旧配置 JSON 可读，旧行为默认不变。

## 非目标

- 不引入 #3 的整套 `AIOptions` 嵌套模型；不迁移现有 `AIConfig.max_tokens` / `ExamParams.custom_prompt`。
- 不改 Cloudflare Worker 的 `env.AI` 计费模型；不新增 Worker 请求。
- 不改变任务型 prompt 的 JSON 输出契约。

## 设计

### A. 配置模型（单一真相源，向后兼容）

`packages/shared/src/types.ts` 的 `AIConfig` 与 `packages/core/src/config/store.rs` 的
`AIConfigData` 同步新增可选字段（保留现有 `max_tokens`）：

| 字段 | 类型 | 默认 | 语义 |
|------|------|------|------|
| `max_tokens` | number \| null | 未设 | 输出上限值（沿用现有） |
| `token_parameter` | `max_tokens` \| `max_completion_tokens` | `max_tokens` | 输出上限写入哪个字段 |
| `temperature` | number \| null | 0.7 | `null` = 不发送该参数 |
| `reasoning_effort` | `minimal`…`max` \| `none` \| null | 未设 | `none` = 发送 `reasoning_effort:"none"`；未设 = 不发送 |
| `extra_prompt` | string | 空 | 追加到所有系统提示词 |
| `retries` | number | 0 | 额外尝试次数 0–5 |
| `timeout_seconds` | number | 未设 | 单次请求超时 1–3600；未设则回退 `AI_TIMEOUT_SECS`（默认 600） |

旧配置缺失新字段时为默认行为；原生加密 JSON 与 `localStorage` 均为加字段，兼容。

### B. 共享模块

`packages/shared/src/aiOptions.ts`（新增，`index.ts` 正式导出）：
- `AIRequestOptions` 接口
- `DEFAULT_AI_OPTIONS`
- `resolveAIOptions(config)`：校验/裁剪范围，产出可跨端传递的 options
- `applyExtraPrompt(system, extra)`：把补充 Prompt 追加到系统提示词

Rust 镜像 `packages/core/src/ai/options.rs`：`AIRequestOptions` `struct`（serde default）+
`validate()` + `apply(&mut body)`；`AIClient::with_options(Option<AIRequestOptions>)`。
`apply` 按 `role=="system"` 定位消息，而非假设 `messages[0]`。

### C. 四端打通

- 浏览器直连：新增 `frontend/src/utils/chatRequest.ts`，统一构建 body（temperature/token
  字段/reasoning/extra prompt）并做重试/超时；`aiClient.ts`、`answerClient.ts` 全部改走它。
- Tauri：`bridge.ts` 把 `options` 传给 `generate_exam` / `answer_question` / `judge_answer` /
  `explain_question`；`lib.rs` 反序列化 `options` 并 `AIClient::with_options`。
- Axum：`routes.rs` 各请求结构加 `options`，handler 应用。
- Workers：`cf.ts` 传 `options`；`index.ts`/`ai.ts` 仅把 CF AI 支持的字段映射进 `ai.run`
  options（不支持的安全忽略），不新增请求。
- Rust `post_chat` 统一应用 options + 重试；`timeout_seconds` 优先，未设用 `AI_TIMEOUT_SECS`。

重试规则（四端一致）：仅重试 瞬时网络错误 / 超时 / HTTP 408 / 429 / 5xx；参数错误、JSON 解析
错误、空响应不重试。退避 `(attempt+1)` 秒。

### D. 原生取消（Tauri）

- 新增 `src-tauri/src/ai_requests.rs`：`Mutex<HashMap<String, oneshot::Sender<()>>>` 注册表 +
  `cancel_ai_request(requestId)` 命令；命令完成/出错时清理自身注册。
- AI 命令签名新增 `requestId: String`，内部 `tokio::select!` 在 AI future 与取消信号间二选一，
  取消返回可识别的 `Cancelled` 错误。
- `bridge.ts` 增加 `invokeAI(command, args, signal)`：生成 `crypto.randomUUID()` 作为 requestId，
  abort 时 `invoke('cancel_ai_request', { requestId })`，并在 `finally` 移除监听。
- Axum/Workers 已有客户端断连取消语义，不新增。

### E. UI

- `ConfigView.vue`：新增可折叠“AI 高级设置”，含上方全部字段；现有独立
  “Response Max Tokens” 输入框**并入该区**，避免两个入口。
- i18n：`frontend/src/i18n/locales.ts` 10 种语言新增键。zh / zh-TW 真实翻译，其余 8 语言
  也提供译文（本设计按用户要求全翻译）。

### F. 版本与发布耦合

- 原生取消新增 `cancel_ai_request` 命令 → 按仓库规则 bump 版本并提高 minShell：
  `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`、`workers/package.json`
  同步 1.4.5 → 1.5.0；`Cargo.lock` 仅改 `name = "exameow"` 条目；`ota.json.minShell` → `1.5.0`。
  （初版曾误写为 1.6.0，跳过了 1.5.0；已按 semver 修正回 1.5.0。）
- 该功能未发版前不影响线上；发版流程照 AGENTS.md。

## 测试

- `packages/core/tests/ai_options.rs`：默认值、校验、字段映射、`messages` 定位、重试分类。
- `scripts/test-ai-options.cjs`：TS 侧参数映射、prompt 附加、重试/超时纯函数对齐 Rust。
- `cargo test -p exameow-core`、`pnpm --dir frontend run type-check`、`pnpm --dir frontend build`、
  `pnpm --dir workers typecheck`、`cargo build -p exameow-server`、`bash scripts/check-ota-minshell.sh`。

## 风险

- 四端参数语义需保持一致；以共享 TS + Rust 镜像测试锁定。
- 新增原生命令的 OTA 兼容性由 minShell 兜底。
- 旧壳不含 `cancel_ai_request`：若无热更新（前端已 OTA）但原生壳过旧，取消会报命令不存在；
  由 minShell 阻止旧壳接收新前端。
