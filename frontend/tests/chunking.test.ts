import { test } from 'node:test'
import assert from 'node:assert/strict'
import { computeBatchSpecs, splitTextChunk, MAX_CHARS_PER_CHUNK, MAX_Q_PER_CHUNK } from '../src/utils/chunking.ts'
import type { TypeEntry } from '../src/utils/chunking.ts'

function paragraphs(n: number, paraChars = 200): string {
  const para = '甲'.repeat(paraChars)
  return Array.from({ length: n }, (_, i) => `${i + 1}. ${para}`).join('\n\n')
}

test('small doc stays a single batch with exact per-type counts', () => {
  const text = paragraphs(5)
  const specs = computeBatchSpecs(text, [
    { type: 'single_choice', count: 3 },
    { type: 'true_false', count: 3 },
  ])
  assert.equal(specs.length, 1)
  assert.equal(specs[0].typeCounts.single_choice, 3)
  assert.equal(specs[0].typeCounts.true_false, 3)
})

test('large doc: every batch within char & question caps, totals preserved', () => {
  const text = paragraphs(300, 210) // ~63k chars
  const types: TypeEntry[] = [
    { type: 'single_choice', count: 20 },
    { type: 'multi_choice', count: 12 },
    { type: 'short_answer', count: 8 },
  ]
  const specs = computeBatchSpecs(text, types)
  assert.ok(specs.length >= 2)
  for (const s of specs) {
    assert.ok(s.text.length <= MAX_CHARS_PER_CHUNK, `chunk ${s.text.length} > ${MAX_CHARS_PER_CHUNK}`)
    const q = Object.values(s.typeCounts).reduce((a, b) => a + b, 0)
    assert.ok(q >= 1 && q <= MAX_Q_PER_CHUNK, `chunk questions ${q}`)
  }
  const sums: Record<string, number> = {}
  for (const s of specs) for (const [t, c] of Object.entries(s.typeCounts)) sums[t] = (sums[t] || 0) + c
  assert.equal(sums.single_choice, 20)
  assert.equal(sums.multi_choice, 12)
  assert.equal(sums.short_answer, 8)
})

test('sparse questions over huge doc yields one batch per allocated question', () => {
  const text = paragraphs(1200, 210) // ~250k chars
  const specs = computeBatchSpecs(text, [{ type: 'single_choice', count: 3 }])
  assert.equal(specs.length, 3)
  const total = specs.reduce((s, x) => s + Object.values(x.typeCounts).reduce((a, b) => a + b, 0), 0)
  assert.equal(total, 3)
  const texts = new Set(specs.map((s) => s.text))
  assert.equal(texts.size, 3)
})

test('splitTextChunk keeps both halves non-empty', () => {
  const body = paragraphs(20)
  const [a, b] = splitTextChunk(body)
  assert.ok(a.length > 0 && b.length > 0)
})
