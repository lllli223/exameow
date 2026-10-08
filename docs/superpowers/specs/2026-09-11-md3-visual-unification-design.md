# MD3 视觉统一：高级设置下拉框、折叠头与 Switch 开关

日期：2026-09-11
状态：待评审（brainstorming 设计已口头通过）

## 背景

近期合并的 PR 引入了若干原生控件，未沿用仓库既有的 Material You 设计语言：

- `ConfigView.vue` 的「AI 高级设置」使用原生 `<details>/<summary>`，展开图标是浏览器默认三角。
- 同区两个字段使用原生 `<select>`（token 参数、思考强度），外观与 `ParamForm.vue` 的
  `BaseSelect`（出题难度下拉框）不一致。
- 「不发送 temperature」与「AI 自动分章」使用原生 `<input type="checkbox">`，缺少 MD3 Switch 形态。

全仓库其余控件已统一为 `BaseSelect.vue` / `BaseCombobox.vue` / `.input-outlined` / `card-filled`
等 MD3 组件（`frontend/src/assets/main.css`）。本设计补齐缺失的 Switch 组件，并把上述原生控件
替换为一致的 MD3 呈现。

## 目标

1. 新增可复用的 MD3 `BaseSwitch.vue` 组件（含 `role="switch"`、键盘无障碍、禁用态）。
2. 「AI 高级设置」折叠头改为 MD3 展开面板样式：左侧引导图标 + 标题，右侧旋转 chevron。
3. token 参数、思考强度改用 `BaseSelect`，与出题难度下拉框观感一致。
4. 「不发送 temperature」「AI 自动分章」改用 `BaseSwitch`。
5. 高级设置内部按语义分组并加小标题，提升信息层级。

## 非目标

- 不改任何配置字段语义、存储结构或四端参数传递逻辑。
- 不改 `packages/`、`src-tauri/`、`workers/` 任何后端代码。
- 不新增非必要的 Cloudflare Worker 请求（纯前端改动）。
- 不重构与本次无关的组件。
- 不改版本号（纯前端视觉改动，无需 `minShell` 提升）。

## 设计

### A. 新增 `frontend/src/components/common/BaseSwitch.vue`

props：`modelValue: boolean`、`disabled?: boolean`。
emits：`update:modelValue`。

MD3 Switch 规格：

- 轨道 52×32，圆角全圆；关闭态 `1.5px` 边框 `--md-outline`，背景 `--md-surface-container-highest`；
  开启态背景 `--md-primary`、无边框。
- 滑块 24px 圆；关闭态 `--md-outline`，开启态 `--md-on-primary`，并显示 16px 对勾图标。
- 位移动画 `translate-x` 使用现有弹簧曲线 `cubic-bezier(0.34,1.56,0.64,1)`。
- 根元素 `role="switch"`、`:aria-checked`、`:aria-disabled`；`tabindex="0"`；Space/Enter 切换；
  鼠标与键盘均有 hover/focus 状态层（`--md-state-hover-alpha`）。
- 禁用态 `opacity-40 pointer-events-none`，与 `btn-*:disabled` 一致。

### B. `ConfigView.vue` 高级设置面板

**折叠头**（替换 `<details>/<summary>`）：

- 外层卡片维持 `card-filled p-5 sm:p-6 mb-4 shadow-sm border`。
- 用受控 `ref` + `<button>` 实现 disclosure：行内左侧 `AdjustmentsHorizontalIcon`（w-5），
  中间标题 `configAdvancedAI`，右侧 `ChevronDownIcon`（w-5）在展开时旋转 180°，
  过渡曲线与 `BaseSelect` 一致。整行不加 hover 背景（细长色带观感差），仅保留 `cursor-pointer` 与 chevron 旋转。
- `aria-expanded` 正确反映状态；展开/收起用现有 `scale` 过渡包裹内容区。

**分组**（用现有 `.divider` 分隔，每组一个 `text-label-md` 小标题）：

| 分组标题（i18n key） | 字段 |
|----------------------|------|
| `configGroupOutput` | max tokens（数字输入，沿用 `input-outlined`） + token 参数（`BaseSelect`） |
| `configGroupSampling` | temperature 数字输入（留空即不发送） + 思考强度（`BaseSelect`） |
| `configGroupReliability` | retries + timeout（数字输入） |
| `configGroupPrompt` | 补充 Prompt textarea + 重试说明 |

- token 参数选项：`max_tokens` / `max_completion_tokens`。
- 思考强度选项：默认（空值）+ `minimal|low|medium|high|xhigh|max|none`，沿用现有选项集合。
- 不使用单独的「不发送 temperature」开关：temperature 输入留空即映射为 `omit_temperature: true`，输入数字则正常发送。`configOmitTemperature` 文案改作该输入框的 placeholder。
  - `stores/config.ts` 的 `temperature` 改为 `number | null`；`buildConfig()` 中 `temperature: temperature.value ?? undefined`、`omit_temperature: temperature.value === null`；移除独立的 `omitTemperature` ref。
  - 各语言 `configOmitTemperature` 与 `configAdvancedAIHint` 文案同步改为“留空即不发送”。

### C. `ParamForm.vue` 自动分章

- 用整行可点击布局替换原生 checkbox：左侧标题 `genAutoChapter` + 说明 `genAutoChapterHint`（纵向），
  右侧 `BaseSwitch` 绑定 `store.autoChapter`，`:disabled="store.generating"`。
- 点击标题文字区域也能切换（`cursor-pointer` + label 语义）。

### D. i18n

`frontend/src/i18n/locales.ts`：`Messages` 接口新增 4 个 key，并在全部 10 种语言
（zh、zh-TW、en、ja、ko、es、fr、de、ru、ar）补真实译文：

- `configGroupOutput`
- `configGroupSampling`
- `configGroupReliability`
- `configGroupPrompt`

## 测试

- `cd frontend && pnpm run type-check`（主要验证）。
- 手动检查明暗主题与 5 种强调色（blue/green/coral/purple/amber）下 Switch 与下拉框对比度。
- 手动检查键盘 Tab/Space 可操作 Switch、折叠头可回车展开。

## 风险

- `BaseSelect` 目前以 `class="[&>button]:..."` 覆写内边距，接入时需确认图标不被遮挡。
- 新增 i18n key 若漏某语言，`Messages` 接口会令 type-check 失败，属预期保护。
- iOS 原生控件外观不受影响（本项目未用原生 `<select>` 渲染，全部自定义 DOM）。
