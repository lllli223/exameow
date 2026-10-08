import { gradeTrueFalseAnswer, parseTrueFalseAnswer, trueFalseOption } from './answerGrading.ts'

function assertEqual<T>(actual: T, expected: T, message: string) {
  if (actual !== expected) {
    throw new Error(`${message}: expected ${String(expected)}, received ${String(actual)}`)
  }
}

for (const value of ['A', '√', '对', '正确', 'TRUE', 'T', '是', 'YES', 'Y', '1']) {
  assertEqual(parseTrueFalseAnswer(value), true, `${value} should parse as true`)
}
for (const value of ['B', '×', '错', '错误', 'FALSE', 'F', '否', 'NO', 'N', '0']) {
  assertEqual(parseTrueFalseAnswer(value), false, `${value} should parse as false`)
}

// Regression: the old substring matcher treated FALSE as true because it contains "A".
assertEqual(parseTrueFalseAnswer('FALSE'), false, 'FALSE must never match A')
assertEqual(gradeTrueFalseAnswer('B', 'false'), true, 'B must be correct for false')
assertEqual(gradeTrueFalseAnswer('A', 'false'), false, 'A must be wrong for false')
assertEqual(gradeTrueFalseAnswer('A', 'true'), true, 'A must be correct for true')
assertEqual(gradeTrueFalseAnswer('B', 'true'), false, 'B must be wrong for true')

// Unknown text must not silently become false.
for (const value of ['', 'banana', 'not sure', 'TRUE-ish', 'FALSE-ish']) {
  assertEqual(parseTrueFalseAnswer(value), null, `${value || '<empty>'} should be rejected`)
}
assertEqual(gradeTrueFalseAnswer('banana', 'false'), false, 'unknown user answer must not grade as false')
assertEqual(gradeTrueFalseAnswer('B', 'banana'), false, 'unknown correct answer must not grade as false')
assertEqual(trueFalseOption('true'), 'A', 'true should map to option A')
assertEqual(trueFalseOption('false'), 'B', 'false should map to option B')
assertEqual(trueFalseOption('banana'), null, 'unknown answer should not highlight an option')
