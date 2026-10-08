const TRUE_ANSWERS = new Set([
  'A', '√', '对', '正确', 'TRUE', 'T', '是', 'YES', 'Y', '1',
])

const FALSE_ANSWERS = new Set([
  'B', '×', '错', '错误', 'FALSE', 'F', '否', 'NO', 'N', '0',
])

/** Parse a true/false answer without substring matching. */
export function parseTrueFalseAnswer(answer: string | null | undefined): boolean | null {
  const normalized = (answer ?? '').trim().toUpperCase()
  if (!normalized) return null
  if (TRUE_ANSWERS.has(normalized)) return true
  if (FALSE_ANSWERS.has(normalized)) return false
  return null
}

/** Grade a true/false answer only when both sides are recognized values. */
export function gradeTrueFalseAnswer(
  userAnswer: string | null | undefined,
  correctAnswer: string | null | undefined,
): boolean {
  const user = parseTrueFalseAnswer(userAnswer)
  const correct = parseTrueFalseAnswer(correctAnswer)
  return user !== null && correct !== null && user === correct
}

export function trueFalseOption(answer: string | null | undefined): 'A' | 'B' | null {
  const parsed = parseTrueFalseAnswer(answer)
  return parsed === null ? null : parsed ? 'A' : 'B'
}
