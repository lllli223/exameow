import type { AIConfig, AIRequestOptions, ReasoningEffort, TokenParameter } from './types'

export const REASONING_EFFORTS: ReasoningEffort[] = ['minimal', 'low', 'medium', 'high', 'xhigh', 'max']

export const DEFAULT_AI_TEMPERATURE = 0.7
export const DEFAULT_TOKEN_PARAMETER: TokenParameter = 'max_tokens'
export const MAX_PROMPT_CHARS = 20000

/**
 * Turn the persisted per-device AIConfig into a normalized, cross-backend request
 * options object. Invalid or out-of-range values are clamped to safe defaults so
 * every backend (Tauri / Axum / Workers / browser-direct) sees identical semantics.
 */
export function resolveAIOptions(config: AIConfig): AIRequestOptions {
  const rawMax = config.max_tokens
  const max_tokens =
    typeof rawMax === 'number' && Number.isFinite(rawMax)
      ? Math.min(1_000_000, Math.max(1, Math.floor(rawMax)))
      : undefined

  const token_parameter: TokenParameter =
    config.token_parameter === 'max_completion_tokens' ? 'max_completion_tokens' : DEFAULT_TOKEN_PARAMETER

  const omit_temperature = config.omit_temperature === true
  let temperature: number | undefined
  if (!omit_temperature) {
    const rawTemp = config.temperature
    temperature =
      typeof rawTemp === 'number' && Number.isFinite(rawTemp)
        ? Math.min(2, Math.max(0, rawTemp))
        : DEFAULT_AI_TEMPERATURE
  }

  const reasoning_effort =
    typeof config.reasoning_effort === 'string' &&
    (REASONING_EFFORTS as string[]).concat('none').includes(config.reasoning_effort)
      ? config.reasoning_effort
      : undefined

  const retries =
    typeof config.retries === 'number' && Number.isFinite(config.retries)
      ? Math.min(5, Math.max(0, Math.floor(config.retries)))
      : 0

  const timeout_seconds =
    typeof config.timeout_seconds === 'number' && Number.isFinite(config.timeout_seconds)
      ? Math.min(3600, Math.max(1, Math.floor(config.timeout_seconds)))
      : undefined

  const extra = typeof config.extra_prompt === 'string' ? config.extra_prompt.trim() : ''
  const extra_prompt = extra ? extra.slice(0, MAX_PROMPT_CHARS) : undefined

  return {
    max_tokens,
    token_parameter,
    temperature,
    omit_temperature,
    reasoning_effort,
    extra_prompt,
    retries,
    timeout_seconds,
  }
}

/**
 * Append user-provided extra instructions to a system prompt while keeping the
 * task's required JSON output contract authoritative.
 */
export function applyExtraPrompt(system: string, extra?: string): string {
  const trimmed = extra?.trim()
  if (!trimmed) return system
  return `${system}\n\n## Additional Instructions (user-provided, highest priority)\n${trimmed}\n\n## Follow the required output format above.`
}
