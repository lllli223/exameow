import assert from 'node:assert/strict'
import { test } from 'node:test'
import {
  isReviewEntry, recordReviewCorrect, reviewActivityAt, setReviewFlag, sortReviewEntries,
} from '../src/utils/reviewEntries.ts'
import type { WrongQuestionEntry } from '@exameow/shared'

const wrong = (questionId: string, count = 1): WrongQuestionEntry => ({
  questionId, wrongCount: count, consecutiveCorrect: 0, lastWrongAt: 10, addedAt: 9,
})

test('a manually flagged correct-only question enters review without being marked wrong', () => {
  const marked = setReviewFlag(undefined, 'q1', true, 50)!
  assert.equal(marked.wrongCount, 0)
  assert.equal(marked.flagged, true)
  assert.equal(isReviewEntry(marked), true)
  assert.equal(reviewActivityAt(marked), 50)
  const correct = recordReviewCorrect(marked)
  assert.equal(correct.entry?.flagged, true)
  assert.equal(correct.entry?.wrongCount, 0)
  assert.equal(correct.removedFromReview, false)
  assert.equal(setReviewFlag(correct.entry!, 'q1', false), null)
})

test('wrong-only questions retain the existing three-correct automatic removal', () => {
  let current: WrongQuestionEntry | null = wrong('wrong')
  for (let i = 0; i < 2; i++) {
    const result = recordReviewCorrect(current!)
    assert.equal(result.removedFromReview, false)
    current = result.entry
    assert.equal(current?.consecutiveCorrect, i + 1)
  }
  const result = recordReviewCorrect(current!)
  assert.equal(result.removedFromReview, true)
  assert.equal(result.entry, null)
})

test('a wrong AND marked question stays in review after three correct answers', () => {
  let current = setReviewFlag(wrong('both', 4), 'both', true, 20)!
  for (let i = 0; i < 3; i++) {
    const result = recordReviewCorrect(current)
    assert.equal(result.removedFromReview, false)
    current = result.entry!
  }
  assert.equal(current.wrongCount, 0)
  assert.equal(current.flagged, true)
  assert.equal(current.consecutiveCorrect, 0)
  assert.equal(isReviewEntry(current), true)
  assert.equal(setReviewFlag(current, 'both', false), null)
})

test('unmarking a still-wrong question preserves the wrong record and counter', () => {
  const marked = setReviewFlag(wrong('both', 2), 'both', true, 22)!
  const unmarked = setReviewFlag(marked, 'both', false)!
  assert.equal(unmarked.wrongCount, 2)
  assert.equal(unmarked.flagged, false)
  assert.equal(isReviewEntry(unmarked), true)
  assert.equal(setReviewFlag(undefined, 'missing', false), null)
})

test('review sorting includes marked-only entries with meaningful timestamps', () => {
  const marked = setReviewFlag(undefined, 'flagged', true, 30)!
  const onlyWrong = wrong('wrong', 3)
  assert.deepEqual(sortReviewEntries([marked, onlyWrong], 'count-desc').map(e => e.questionId), ['wrong', 'flagged'])
  assert.deepEqual(sortReviewEntries([marked, onlyWrong], 'count-asc').map(e => e.questionId), ['flagged', 'wrong'])
  assert.deepEqual(sortReviewEntries([marked, onlyWrong], 'time-desc').map(e => e.questionId), ['flagged', 'wrong'])
  assert.deepEqual(sortReviewEntries([marked, onlyWrong], 'time-asc').map(e => e.questionId), ['wrong', 'flagged'])
})

test('legacy wrong-question records remain reviewable without new fields', () => {
  const legacy = wrong('legacy')
  assert.equal(isReviewEntry(legacy), true)
  assert.equal(setReviewFlag(legacy, 'legacy', false)?.wrongCount, 1)
})
