import type { WrongQuestionEntry, WrongSort } from '@exameow/shared'

/** A manually marked question is reviewable even if it has never been answered wrong. */
export function isReviewEntry(entry: WrongQuestionEntry): boolean {
  return entry.wrongCount > 0 || entry.flagged === true
}

/** Marking is independent of the wrong-answer count and its three-correct streak. */
export function setReviewFlag(
  entry: WrongQuestionEntry | undefined,
  questionId: string,
  flagged: boolean,
  now = Date.now(),
): WrongQuestionEntry | null {
  if (flagged) {
    if (entry?.flagged) return entry
    return entry
      ? { ...entry, flagged: true, flaggedAt: now }
      : {
        questionId,
        wrongCount: 0,
        consecutiveCorrect: 0,
        lastWrongAt: 0,
        addedAt: now,
        flagged: true,
        flaggedAt: now,
      }
  }
  if (!entry) return null
  if (entry.wrongCount <= 0) return null
  return { ...entry, flagged: false, flaggedAt: undefined }
}

/** Preserve an explicit review mark when a wrong question is mastered. */
export function recordReviewCorrect(entry: WrongQuestionEntry): {
  entry: WrongQuestionEntry | null
  removedFromReview: boolean
} {
  if (entry.wrongCount <= 0) return { entry, removedFromReview: false }
  const consecutiveCorrect = entry.consecutiveCorrect + 1
  if (consecutiveCorrect < 3) {
    return { entry: { ...entry, consecutiveCorrect }, removedFromReview: false }
  }
  if (entry.flagged) {
    return {
      entry: { ...entry, wrongCount: 0, consecutiveCorrect: 0 },
      removedFromReview: false,
    }
  }
  return { entry: null, removedFromReview: true }
}

/** For marked-only questions, use the marking time instead of the epoch. */
export function reviewActivityAt(entry: WrongQuestionEntry): number {
  return Math.max(entry.lastWrongAt, entry.flaggedAt ?? 0, entry.addedAt)
}

export function sortReviewEntries(entries: WrongQuestionEntry[], sort: WrongSort): WrongQuestionEntry[] {
  return [...entries].sort((a, b) => {
    switch (sort) {
      case 'count-desc': return b.wrongCount - a.wrongCount || reviewActivityAt(b) - reviewActivityAt(a)
      case 'count-asc': return a.wrongCount - b.wrongCount || reviewActivityAt(b) - reviewActivityAt(a)
      case 'time-desc': return reviewActivityAt(b) - reviewActivityAt(a)
      case 'time-asc': return reviewActivityAt(a) - reviewActivityAt(b)
    }
  })
}
