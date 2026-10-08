// Focused tests for AI request options: pure mapping, prompt extras and transport retry.
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const Module = require('node:module')
const root = path.resolve(__dirname, '..')
const ts = require(path.join(root, 'frontend/node_modules/typescript'))
const resolve = Module._resolveFilename
Module._resolveFilename = function (id, parent, ...args) {
  if (id === '@exameow/shared') return resolve.call(this, path.join(root, 'packages/shared/src/index.ts'), parent, ...args)
  if (id.startsWith('@/')) return resolve.call(this, path.join(root, 'frontend/src', id.slice(2)), parent, ...args)
  return resolve.call(this, id, parent, ...args)
}
require.extensions['.ts'] = (module, filename) => {
  module._compile(ts.transpileModule(fs.readFileSync(filename, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText, filename)
}

const { resolveAIOptions, applyExtraPrompt } = require(path.join(root, 'packages/shared/src/aiOptions.ts'))
const { chatRequest, AIHttpError, isRetryableAIError } = require(path.join(root, 'frontend/src/utils/chatRequest.ts'))

// --- resolveAIOptions ---
const defaults = resolveAIOptions({ endpoint: 'e', api_key: 'k', model: 'm' })
assert.equal(defaults.token_parameter, 'max_tokens')
assert.equal(defaults.temperature, 0.7)
assert.equal(defaults.omit_temperature, false)
assert.equal(defaults.retries, 0)
assert.equal(defaults.max_tokens, undefined)
assert.equal(defaults.reasoning_effort, undefined)
assert.equal(defaults.timeout_seconds, undefined)

const clamped = resolveAIOptions({
  endpoint: 'e', api_key: 'k', model: 'm',
  max_tokens: 5_000_000, temperature: 9, retries: 9, timeout_seconds: 0,
})
assert.equal(clamped.max_tokens, 1_000_000)
assert.equal(clamped.temperature, 2)
assert.equal(clamped.retries, 5)
assert.equal(clamped.timeout_seconds, 1)

const omitted = resolveAIOptions({ endpoint: 'e', api_key: 'k', model: 'm', omit_temperature: true })
assert.equal(omitted.omit_temperature, true)
assert.equal(omitted.temperature, undefined)

assert.equal(resolveAIOptions({ endpoint: 'e', api_key: 'k', model: 'm', reasoning_effort: 'high' }).reasoning_effort, 'high')
assert.equal(resolveAIOptions({ endpoint: 'e', api_key: 'k', model: 'm', reasoning_effort: 'none' }).reasoning_effort, 'none')
assert.equal(resolveAIOptions({ endpoint: 'e', api_key: 'k', model: 'm', reasoning_effort: 'ultra' }).reasoning_effort, undefined)
assert.equal(resolveAIOptions({ endpoint: 'e', api_key: 'k', model: 'm', token_parameter: 'max_completion_tokens' }).token_parameter, 'max_completion_tokens')
assert.equal(resolveAIOptions({ endpoint: 'e', api_key: 'k', model: 'm', extra_prompt: '  hello  ' }).extra_prompt, 'hello')

// --- applyExtraPrompt ---
assert.equal(applyExtraPrompt('SYS', undefined), 'SYS')
assert.equal(applyExtraPrompt('SYS', '  '), 'SYS')
const withExtra = applyExtraPrompt('SYS', 'Be terse')
assert.ok(withExtra.startsWith('SYS'))
assert.ok(withExtra.includes('Be terse'))
assert.ok(withExtra.includes('Additional Instructions'))

// --- retry classification ---
assert.equal(isRetryableAIError(new AIHttpError(503, 'x')), true)
assert.equal(isRetryableAIError(new AIHttpError(429, 'x')), true)
assert.equal(isRetryableAIError(new AIHttpError(400, 'x')), false)
assert.equal(isRetryableAIError(new Error('empty response')), false)

// --- chatRequest transport ---
async function testTransport() {
  const originalFetch = global.fetch
  let calls = 0
  const bodies = []
  global.fetch = async (url, request) => {
    calls++
    bodies.push(JSON.parse(request.body))
    assert.match(String(url), /\/chat\/completions$/)
    if (calls === 1) return new Response('busy', { status: 503 })
    return Response.json({ choices: [{ message: { content: '{"answer":"A","analysis":"ok"}' } }] })
  }
  try {
    const config = {
      endpoint: 'https://example.invalid/v1', api_key: 'k', model: 'm',
      max_tokens: 2048, token_parameter: 'max_completion_tokens',
      reasoning_effort: 'high', omit_temperature: true,
      extra_prompt: 'Be terse', retries: 1,
    }
    const content = await chatRequest('SYS', 'USER', config, resolveAIOptions(config))
    assert.equal(calls, 2)
    assert.deepEqual(bodies[0], bodies[1])
    assert.equal(bodies[0].max_completion_tokens, 2048)
    assert.equal(bodies[0].reasoning_effort, 'high')
    assert.equal(bodies[0].temperature, undefined)
    assert.equal(bodies[0].max_tokens, undefined)
    assert.ok(bodies[0].messages[0].content.startsWith('SYS'))
    assert.ok(bodies[0].messages[0].content.includes('Be terse'))
    assert.equal(content, '{"answer":"A","analysis":"ok"}')
  } finally {
    global.fetch = originalFetch
  }

  // Client errors must not be retried.
  let badCalls = 0
  global.fetch = async () => { badCalls++; return new Response('nope', { status: 400 }) }
  try {
    const config = { endpoint: 'https://example.invalid/v1', api_key: 'k', model: 'm', retries: 3 }
    await assert.rejects(() => chatRequest('SYS', 'USER', config, resolveAIOptions(config)))
    assert.equal(badCalls, 1)
  } finally {
    global.fetch = originalFetch
  }

  // Fallback max tokens is used when no explicit value is set.
  let fallbackBody
  global.fetch = async (_url, request) => {
    fallbackBody = JSON.parse(request.body)
    return Response.json({ choices: [{ message: { content: 'ok' } }] })
  }
  try {
    const config = { endpoint: 'https://example.invalid/v1', api_key: 'k', model: 'm' }
    await chatRequest('SYS', 'USER', config, resolveAIOptions(config), undefined, 16384)
    assert.equal(fallbackBody.max_tokens, 16384)
  } finally {
    global.fetch = originalFetch
  }

  console.log('AI options mapping, prompt extras and transport retry tests passed')
}

testTransport().catch(error => { console.error(error); process.exitCode = 1 })
