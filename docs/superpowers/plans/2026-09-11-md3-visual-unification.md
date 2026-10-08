# MD3 视觉统一 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把近期 PR 引入的原生控件（两个 `<select>`、原生 `<details>` 三角、两个 checkbox）替换为仓库既有的 Material You / MD3 呈现，并新增可复用 `BaseSwitch`。

**Architecture:** 纯前端改动。新增 `BaseSwitch.vue` 共享组件；`ConfigView.vue` 高级设置改为受控折叠面板 + 分组布局，下拉框复用 `BaseSelect.vue`；`ParamForm.vue` 自动分章改用 `BaseSwitch`。i18n 新增 4 个分组标题键并补齐 10 种语言。

**Tech Stack:** Vue 3 `<script setup>` + TypeScript、Tailwind CSS 3.4、`@heroicons/vue`、Pinia、`vue-tsc`。

## Global Constraints

- **纯前端**：不得修改 `packages/`、`src-tauri/`、`workers/`、任何后端代码或存储结构。
- **不改版本号**：无新增原生命令，无需 bump，也无需改 `ota.json.minShell`。
- **不新增 Cloudflare Worker 请求**。
- **i18n 全覆盖**：`frontend/src/i18n/locales.ts` 的 `LocaleMessages` 接口与全部 10 种语言（zh, zh-TW, en, ja, ko, es, fr, de, ru, ar）必须同步；否则 `pnpm run type-check` 失败。
- **验证命令（无自动化测试套件）**：`cd frontend && pnpm run type-check`。仓库没有单测框架，AGENTS.md 指定 type-check 为验证手段。
- **颜色/圆角/动效**：复用 `frontend/src/assets/main.css` 中的 `--md-*` 令牌与 `ease-[cubic-bezier(0.34,1.56,0.64,1)]` 弹簧曲线，不引入新库。

---

### Task 1: 新增 MD3 `BaseSwitch` 组件

**Files:**
- Create: `frontend/src/components/common/BaseSwitch.vue`

**Interfaces:**
- Consumes: 无（仅 `@heroicons/vue` 的 `CheckIcon`）。
- Produces: 组件 `BaseSwitch`，props `{ modelValue: boolean; disabled?: boolean }`，emit `update:modelValue(v: boolean)`。支持 `v-model`，也可直接传 `:disabled`。

- [ ] **Step 1: 创建组件文件**

创建 `frontend/src/components/common/BaseSwitch.vue`，内容：

```vue
<script setup lang="ts">
import { CheckIcon } from '@heroicons/vue/24/outline'

const props = defineProps<{
  modelValue: boolean
  disabled?: boolean
}>()

const emit = defineEmits<{ (e: 'update:modelValue', v: boolean): void }>()

function toggle() {
  if (props.disabled) return
  emit('update:modelValue', !props.modelValue)
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === ' ' || e.key === 'Enter') {
    e.preventDefault()
    toggle()
  }
}
</script>

<template>
  <button
    type="button"
    role="switch"
    :aria-checked="modelValue"
    :aria-disabled="disabled || undefined"
    :disabled="disabled"
    class="group relative inline-flex shrink-0 items-center w-[52px] h-8 rounded-full outline-none focus-visible:ring-2 focus-visible:ring-offset-2 focus-visible:ring-[rgb(var(--md-primary))] transition-colors duration-200 cursor-pointer disabled:opacity-40 disabled:pointer-events-none"
    :style="{
      backgroundColor: modelValue ? 'rgb(var(--md-primary))' : 'rgb(var(--md-surface-container-highest))',
      border: modelValue ? '2px solid rgb(var(--md-primary))' : '2px solid rgb(var(--md-outline))',
    }"
    @click="toggle"
    @keydown="onKeydown"
  >
    <span
      class="pointer-events-none absolute inset-0 rounded-full opacity-0 transition-opacity duration-200 group-hover:opacity-[0.08] group-active:opacity-[0.12]"
      :style="{ backgroundColor: modelValue ? 'rgb(var(--md-on-primary))' : 'rgb(var(--md-on-surface))' }"
    />
    <span
      class="pointer-events-none absolute top-1/2 -translate-y-1/2 flex items-center justify-center rounded-full shadow-sm transition-all duration-300 ease-[cubic-bezier(0.34,1.56,0.64,1)]"
      :style="{
        width: modelValue ? '24px' : '16px',
        height: modelValue ? '24px' : '16px',
        left: modelValue ? '24px' : '8px',
        backgroundColor: modelValue ? 'rgb(var(--md-on-primary))' : 'transparent',
        border: modelValue ? 'none' : '2px solid rgb(var(--md-outline))',
        color: 'rgb(var(--md-primary))',
      }"
    >
      <CheckIcon v-if="modelValue" class="w-4 h-4" />
    </span>
  </button>
</template>
```

- [ ] **Step 2: 类型检查**

Run: `cd frontend && pnpm run type-check`
Expected: 通过（无错误）。

- [ ] **Step 3: 提交**

```bash
git add frontend/src/components/common/BaseSwitch.vue
git commit -m "feat(web): add MD3 BaseSwitch component"
```

---

### Task 2: 新增高级设置分组 i18n 键（10 种语言）

**Files:**
- Modify: `frontend/src/i18n/locales.ts`（接口约第 9-20 行；10 个语言对象各一处）

**Interfaces:**
- Consumes: 无。
- Produces: `LocaleMessages` 新增 4 个字符串键：`configGroupOutput`、`configGroupSampling`、`configGroupReliability`、`configGroupPrompt`。

- [ ] **Step 1: 在接口中声明 4 个键**

在 `export interface LocaleMessages {` 内、`configAdvancedAIHint: string`（第 10 行）之后插入：

```ts
  configGroupOutput: string
  configGroupSampling: string
  configGroupReliability: string
  configGroupPrompt: string
```

- [ ] **Step 2: 为每种语言插入译文**

在**每个**语言对象中，紧随该语言的 `configAdvancedAIHint: "...",` 行之后插入下面 4 行（用对应语言的值替换 `...`）。每种语言的插入位置都以 `configAdvancedAIHint` 为锚点。

模板：

```ts
  configGroupOutput: "<OUTPUT>",
  configGroupSampling: "<SAMPLING>",
  configGroupReliability: "<RELIABILITY>",
  configGroupPrompt: "<PROMPT>",
```

各语言取值（逐字使用）：

| 语言对象 | OUTPUT | SAMPLING | RELIABILITY | PROMPT |
|----------|--------|----------|-------------|--------|
| `zh` | 输出上限 | 采样与思考 | 可靠性 | 补充 Prompt |
| `zh-TW` | 輸出上限 | 取樣與思考 | 可靠性 | 補充 Prompt |
| `en` | Output limit | Sampling & thinking | Reliability | Extra prompt |
| `ja` | 出力上限 | サンプリングと思考 | 信頼性 | 追加プロンプト |
| `ko` | 출력 한도 | 샘플링 및 사고 | 안정성 | 추가 프롬프트 |
| `es` | Límite de salida | Muestreo y razonamiento | Fiabilidad | Prompt adicional |
| `fr` | Limite de sortie | Échantillonnage et réflexion | Fiabilité | Prompt supplémentaire |
| `de` | Ausgabelimit | Sampling & Denken | Zuverlässigkeit | Zusätzlicher Prompt |
| `ru` | Лимит вывода | Сэмплирование и рассуждение | Надёжность | Дополнительный промпт |
| `ar` | حد الإخراج | أخذ العينات والتفكير | الموثوقية | مطالبة إضافية |

例如 `zh` 对象应变为：

```ts
  configAdvancedAI: "AI 高级设置",
  configAdvancedAIHint: "适用于当前设备上的出题、答题、批改和解析。思考使用 reasoning_effort，支持的档位取决于模型。OpenAI 推理模型请选择 max_completion_tokens，并按模型要求勾选不发送 temperature。",
  configGroupOutput: "输出上限",
  configGroupSampling: "采样与思考",
  configGroupReliability: "可靠性",
  configGroupPrompt: "补充 Prompt",
```

- [ ] **Step 3: 类型检查**

Run: `cd frontend && pnpm run type-check`
Expected: 通过。若某个语言对象漏加键，此处会报缺失属性错误。

- [ ] **Step 4: 提交**

```bash
git add frontend/src/i18n/locales.ts
git commit -m "feat(web): add advanced AI settings group i18n keys"
```

---

### Task 3: `ConfigView.vue` 高级设置改用 MD3 折叠头、`BaseSelect`、`BaseSwitch` 与分组

**Files:**
- Modify: `frontend/src/views/ConfigView.vue:1-11`（imports + 状态）
- Modify: `frontend/src/views/ConfigView.vue:181-262`（高级设置 `<details>` 块整体替换）

**Interfaces:**
- Consumes: Task 1 的 `BaseSwitch`（`v-model` + `:disabled`）；Task 2 的 4 个 i18n 键；现有 `BaseSelect`（props `modelValue: any`、`options: {value,label,hint?}[]`、emit `update:modelValue`）。
- Produces: 无跨任务接口。

- [ ] **Step 1: 更新 imports 与本地状态**

把 `frontend/src/views/ConfigView.vue` 第 8-9 行：

```ts
import BaseCombobox from '@/components/common/BaseCombobox.vue'
import { ServerIcon, KeyIcon, CloudArrowDownIcon, CpuChipIcon, CheckCircleIcon, EyeIcon, EyeSlashIcon, CheckIcon, ArrowRightIcon, ArrowLeftIcon, CloudIcon } from '@heroicons/vue/24/outline'
```

替换为：

```ts
import BaseCombobox from '@/components/common/BaseCombobox.vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import BaseSwitch from '@/components/common/BaseSwitch.vue'
import { ServerIcon, KeyIcon, CloudArrowDownIcon, CpuChipIcon, CheckCircleIcon, EyeIcon, EyeSlashIcon, CheckIcon, ArrowRightIcon, ArrowLeftIcon, CloudIcon, AdjustmentsHorizontalIcon, ChevronDownIcon } from '@heroicons/vue/24/outline'
```

- [ ] **Step 2: 新增折叠状态与选项**

在 `const showKey = ref(false)` 之前插入：

```ts
const advancedOpen = ref(false)

const tokenParameterOptions = [
  { value: 'max_tokens', label: 'max_tokens' },
  { value: 'max_completion_tokens', label: 'max_completion_tokens' },
]

const reasoningEffortOptions = computed(() => [
  { value: '', label: i18n.t('configReasoningDefault') },
  ...['minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'none'].map((e) => ({ value: e, label: e })),
])
```

`computed` 已在第 2 行导入，无需改动。

- [ ] **Step 3: 替换整段高级设置模板**

把第 181-262 行从 `<!-- Advanced AI settings (includes response max tokens) -->` 到对应的 `</details>`（含）整体替换为：

```html
    <!-- Advanced AI settings -->
    <div class="card-filled p-5 sm:p-6 mb-4 shadow-sm border border-[rgb(var(--md-outline-variant)/0.3)]">
      <button
        type="button"
        class="w-full flex items-center gap-3 -mx-2 px-2 py-1 rounded-2xl text-left transition-colors hover:bg-[rgb(var(--md-on-surface-variant)/0.08)] cursor-pointer"
        :aria-expanded="advancedOpen"
        @click="advancedOpen = !advancedOpen"
      >
        <AdjustmentsHorizontalIcon class="w-5 h-5 shrink-0" style="color: rgb(var(--md-on-surface-variant))" />
        <span class="flex-1 text-label-md font-semibold" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('configAdvancedAI') }}</span>
        <ChevronDownIcon
          class="w-5 h-5 shrink-0 transition-transform duration-300 ease-[cubic-bezier(0.34,1.56,0.64,1)]"
          :class="{ 'rotate-180': advancedOpen }"
          style="color: rgb(var(--md-on-surface-variant))"
        />
      </button>

      <Transition name="scale">
        <div v-if="advancedOpen">
          <p class="text-body-sm mt-3 mb-4" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('configAdvancedAIHint') }}</p>

          <!-- 输出上限 -->
          <p class="text-label-md font-semibold mb-3" style="color: rgb(var(--md-on-surface))">{{ i18n.t('configGroupOutput') }}</p>
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
            <label class="text-label-md block">{{ i18n.t('configMaxTokens') }}
              <input
                :value="configStore.maxTokens ?? ''"
                type="number"
                min="1"
                step="1"
                class="input-outlined w-full mt-2 text-sm"
                :placeholder="i18n.t('configMaxTokensHint')"
                @input="(e: Event) => {
                  const v = (e.target as HTMLInputElement).value
                  configStore.maxTokens = v === '' ? null : Math.max(1, Math.floor(Number(v)) || 1)
                }"
              />
            </label>
            <div>
              <label class="text-label-md block">{{ i18n.t('configTokenParameter') }}</label>
              <BaseSelect
                class="mt-2 [&>button]:!rounded-xl"
                :model-value="configStore.tokenParameter"
                :options="tokenParameterOptions"
                @update:model-value="configStore.tokenParameter = $event"
              />
            </div>
          </div>

          <div class="divider my-5" />

          <!-- 采样与思考 -->
          <p class="text-label-md font-semibold mb-3" style="color: rgb(var(--md-on-surface))">{{ i18n.t('configGroupSampling') }}</p>
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
            <div>
              <label class="text-label-md block">{{ i18n.t('configTemperature') }}</label>
              <input
                :value="configStore.temperature"
                type="number"
                min="0"
                max="2"
                step="0.1"
                :disabled="configStore.omitTemperature"
                class="input-outlined w-full mt-2 text-sm disabled:opacity-50"
                @input="(e: Event) => {
                  const v = Number((e.target as HTMLInputElement).value)
                  configStore.temperature = Number.isFinite(v) ? Math.min(2, Math.max(0, v)) : 0.7
                }"
              />
              <div class="flex items-center justify-between gap-3 mt-2">
                <span class="text-body-sm" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('configOmitTemperature') }}</span>
                <BaseSwitch v-model="configStore.omitTemperature" />
              </div>
            </div>
            <div>
              <label class="text-label-md block">{{ i18n.t('configReasoningEffort') }}</label>
              <BaseSelect
                class="mt-2 [&>button]:!rounded-xl"
                :model-value="configStore.reasoningEffort"
                :options="reasoningEffortOptions"
                @update:model-value="configStore.reasoningEffort = $event"
              />
            </div>
          </div>

          <div class="divider my-5" />

          <!-- 可靠性 -->
          <p class="text-label-md font-semibold mb-3" style="color: rgb(var(--md-on-surface))">{{ i18n.t('configGroupReliability') }}</p>
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
            <label class="text-label-md block">{{ i18n.t('configRetries') }}
              <input v-model.number="configStore.retries" type="number" min="0" max="5" step="1" class="input-outlined w-full mt-2 text-sm" />
            </label>
            <label class="text-label-md block">{{ i18n.t('configTimeout') }}
              <input
                :value="configStore.timeoutSeconds ?? ''"
                type="number"
                min="1"
                max="3600"
                step="1"
                class="input-outlined w-full mt-2 text-sm"
                @input="(e: Event) => {
                  const v = (e.target as HTMLInputElement).value
                  configStore.timeoutSeconds = v === '' ? null : Math.min(3600, Math.max(1, Math.floor(Number(v)) || 1))
                }"
              />
            </label>
          </div>

          <div class="divider my-5" />

          <!-- 补充 Prompt -->
          <p class="text-label-md font-semibold mb-3" style="color: rgb(var(--md-on-surface))">{{ i18n.t('configGroupPrompt') }}</p>
          <label class="text-label-md block">{{ i18n.t('configExtraPrompt') }}
            <textarea
              v-model="configStore.extraPrompt"
              maxlength="20000"
              rows="4"
              class="input-outlined w-full mt-2 text-sm"
              :placeholder="i18n.t('configExtraPromptHint')"
            />
          </label>
          <p class="text-body-sm mt-3" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('configRetryHint') }}</p>
        </div>
      </Transition>
    </div>
```

- [ ] **Step 4: 类型检查**

Run: `cd frontend && pnpm run type-check`
Expected: 通过。注意 `configStore.reasoningEffort` 类型为 `ReasoningEffort | ''`，`BaseSelect` 的 `modelValue` 为 `any`，赋值 `$event` 兼容。

- [ ] **Step 5: 提交**

```bash
git add frontend/src/views/ConfigView.vue
git commit -m "refactor(web): MD3 disclosure, selects and switch in advanced AI settings"
```

---

### Task 4: `ParamForm.vue` 自动分章改用 `BaseSwitch`

**Files:**
- Modify: `frontend/src/components/generate/ParamForm.vue:6-7`（import）
- Modify: `frontend/src/components/generate/ParamForm.vue:142-148`（checkbox 块）

**Interfaces:**
- Consumes: Task 1 的 `BaseSwitch`。
- Produces: 无。

- [ ] **Step 1: 引入 `BaseSwitch`**

在第 6 行 `import BaseSelect from '@/components/common/BaseSelect.vue'` 之后插入：

```ts
import BaseSwitch from '@/components/common/BaseSwitch.vue'
```

- [ ] **Step 2: 替换 checkbox 块**

把第 142-148 行：

```html
    <label class="flex items-start gap-3 mt-5 cursor-pointer">
      <input v-model="store.autoChapter" type="checkbox" :disabled="store.generating" class="mt-1 h-4 w-4 accent-primary" />
      <span>
        <span class="text-label-md font-semibold">{{ i18n.t('genAutoChapter') }}</span>
        <span class="block text-body-sm mt-1" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('genAutoChapterHint') }}</span>
      </span>
    </label>
```

替换为：

```html
    <div class="flex items-start justify-between gap-4 mt-5">
      <div
        class="select-none"
        :class="store.generating ? 'opacity-40 pointer-events-none' : 'cursor-pointer'"
        @click="store.autoChapter = !store.autoChapter"
      >
        <span class="text-label-md font-semibold block">{{ i18n.t('genAutoChapter') }}</span>
        <span class="block text-body-sm mt-1" style="color: rgb(var(--md-on-surface-variant))">{{ i18n.t('genAutoChapterHint') }}</span>
      </div>
      <BaseSwitch v-model="store.autoChapter" :disabled="store.generating" />
    </div>
```

- [ ] **Step 3: 类型检查**

Run: `cd frontend && pnpm run type-check`
Expected: 通过。

- [ ] **Step 4: 手动核对（可选，但推荐）**

Run: `cd frontend && pnpm dev`，浏览器打开 `http://localhost:5273`
Expected: 出题页自动分章为 MD3 开关；点击文字与开关都能切换；出题中开关禁用。配置页高级设置展开/收起正常，两个下拉框与出题难度样式一致，「不发送 temperature」为开关。

- [ ] **Step 5: 提交**

```bash
git add frontend/src/components/generate/ParamForm.vue
git commit -m "refactor(web): MD3 switch for auto chapter tagging"
```

---

## Self-Review

- **Spec coverage：** A 组件 → Task 1；B 折叠头/下拉框/分组 → Task 3；C 自动分章 → Task 4；D i18n → Task 2。全部覆盖。
- **Placeholder scan：** 无 TBD/TODO；所有代码与译文均给出。
- **Type consistency：** `BaseSwitch` props 为 `modelValue: boolean`/`disabled?: boolean`，Task 3、4 用法一致；i18n 键名 `configGroupOutput/Sampling/Reliability/Prompt` 在接口与 10 语言中一致。
- **无测试套件说明：** 已按 AGENTS.md 用 `pnpm run type-check` 替代单测；这是对 TDD 步骤的必要适配。

---

## Revision (2026-09-11, post-Task-4)

用户反馈后调整，实施已完成：

1. **移除「不发送 temperature」`BaseSwitch`**：temperature 输入留空即不发送。`stores/config.ts` 的 `temperature` 改为 `number | null`，`omit_temperature` 由 `temperature === null` 派生，删除独立 `omitTemperature` ref；`configOmitTemperature` 文案改作 temperature 输入框 placeholder（10 语言同步为“留空即不发送”），并更新各语言 `configAdvancedAIHint` 中过时的“勾选”措辞。
2. **移除高级设置折叠头的 hover 背景**（细长色带观感差），仅保留 `cursor-pointer`、`aria-expanded` 与 chevron 旋转。

`BaseSwitch` 仍由 `ParamForm.vue`（AI 自动分章）使用，组件保留。
