export const MAX_CHARS_PER_CHUNK = 12000
export const MAX_Q_PER_CHUNK = 10
const TABLE_SEP_RE = /^\|(\s*:?-{3,}:?\s*\|)+$/

export interface TypeEntry { type: string; count: number }
export interface BatchSpec { text: string; typeCounts: Record<string, number> }

function splitByFileSections(text: string): { text: string; label: string }[] {
  const parts = text.split(/\n\n---\n\n(?=## )/)
  if (parts.length <= 1) return [{ text, label: '' }]
  return parts.map((p, i) => {
    const nl = p.indexOf('\n')
    const label = nl > 0 ? p.substring(3, nl).trim() : `File ${i + 1}`
    const content = nl > 0 ? p.substring(nl + 1) : p
    return { text: content, label }
  })
}

function chunkTextBySize(text: string, chunkCount: number): string[] {
  let paragraphs = text.split(/\n\n+/).filter((p) => p.trim().length > 10)
  let lineMode = false
  if (paragraphs.some((p) => p.length > MAX_CHARS_PER_CHUNK) || paragraphs.length < chunkCount) {
    paragraphs = text.split(/\n+/).filter((p) => {
      const t = p.trim()
      return t.length > 10 || (t.startsWith('|') && t.length > 2)
    })
    lineMode = true
  }
  if (paragraphs.length === 0 || chunkCount <= 1) return [text]

  let heading = ''
  let tableHeader: string | null = null
  const contexts: (string | null)[] = paragraphs.map((p, i) => {
    const t = p.trim()
    if (t.startsWith('#')) {
      heading = t
      tableHeader = null
      return null
    }
    if (t.startsWith('|')) {
      if (TABLE_SEP_RE.test(t)) return null
      if (tableHeader === null) {
        const next = paragraphs[i + 1]?.trim() ?? ''
        if (TABLE_SEP_RE.test(next)) {
          tableHeader = (heading ? heading + '\n\n' : '') + t + '\n' + next
        }
        return null
      }
      return tableHeader
    }
    tableHeader = null
    return null
  })

  const targetSize = Math.ceil(text.length / chunkCount)
  const chunks: string[] = []
  let current = ''
  const sep = lineMode ? '\n' : '\n\n'

  for (let i = 0; i < paragraphs.length; i++) {
    const para = paragraphs[i]!
    if (current.length + para.length > targetSize && current.length > 0 && chunks.length < chunkCount - 1) {
      chunks.push(current.trim())
      current = contexts[i] ? contexts[i] + '\n' + para : para
    } else {
      current += (current ? sep : '') + para
    }
  }
  if (current.trim()) chunks.push(current.trim())
  if (chunks.length === 0) return [text]
  return chunks
}

export function splitTextChunk(chunk: string): [string, string] {
  let header = ''
  let body = chunk
  if (chunk.startsWith('## ')) {
    const nl = chunk.indexOf('\n')
    if (nl > 0) {
      header = chunk.substring(0, nl + 1)
      body = chunk.substring(nl + 1)
    }
  }
  let tableHeader = ''
  const lines = body.split('\n')
  if (lines.length > 2 && lines[0]!.trim().startsWith('|') && TABLE_SEP_RE.test(lines[1]!.trim() ?? '')) {
    tableHeader = lines[0]! + '\n' + lines[1]! + '\n'
    body = lines.slice(2).join('\n')
  }
  const mid = Math.floor(body.length / 2)
  const nl = body.indexOf('\n', mid)
  const split = nl > 0 && nl < body.length - 1 ? nl + 1 : mid
  return [
    header + tableHeader + body.substring(0, split).trim(),
    header + tableHeader + body.substring(split).trim(),
  ]
}

function chunkByFileProportion(sections: { text: string; label: string }[], chunkCount: number, fallbackText: string): string[] {
  const sectionLengths = sections.map((s) => s.text.length)
  const totalLen = sectionLengths.reduce((s, l) => s + l, 0)

  const allocated: number[] = sectionLengths.map((len) => Math.max(1, Math.round((chunkCount * len) / totalLen)))
  let sum = allocated.reduce((s, n) => s + n, 0)
  while (sum > chunkCount) {
    const maxIdx = allocated.indexOf(Math.max(...allocated))
    allocated[maxIdx]!--
    sum--
  }
  while (sum < chunkCount) {
    const minIdx = allocated.indexOf(Math.min(...allocated))
    allocated[minIdx]!++
    sum++
  }

  const result: string[] = []
  for (let i = 0; i < sections.length; i++) {
    const sectionChunks = chunkTextBySize(sections[i]!.text, Math.max(1, allocated[i]!))
    for (const chunk of sectionChunks) {
      const prefix = sections[i]!.label ? `## ${sections[i]!.label}\n` : ''
      result.push(prefix + chunk)
    }
  }

  while (result.length < chunkCount) {
    let maxIdx = 0
    for (let i = 1; i < result.length; i++) {
      if (result[i]!.length > result[maxIdx]!.length) maxIdx = i
    }
    const [a, b] = splitTextChunk(result[maxIdx]!)
    result.splice(maxIdx, 1, a, b)
  }
  while (result.length > chunkCount) {
    let minIdx = 0
    let minLen = result[0]!.length + (result[1]?.length ?? Infinity)
    for (let i = 1; i < result.length - 1; i++) {
      const combined = result[i]!.length + result[i + 1]!.length
      if (combined < minLen) { minLen = combined; minIdx = i }
    }
    result.splice(minIdx, 2, result[minIdx]! + '\n\n' + result[minIdx + 1]!)
  }
  if (result.length === 0) return [fallbackText]
  return result
}

// Split oversized chunks (pathological single paragraphs) so the hard size invariant holds.
function enforceBudget(chunks: string[]): string[] {
  const out = [...chunks]
  for (let i = 0; i < out.length; i++) {
    while (out[i]!.length > MAX_CHARS_PER_CHUNK) {
      const [a, b] = splitTextChunk(out[i]!)
      out.splice(i, 1, a, b)
    }
  }
  return out
}

function allocateWithCap(sizes: number[], total: number, cap: number): number[] {
  const totalLen = sizes.reduce((s, n) => s + n, 0) || 1
  const alloc = sizes.map((s) => Math.min(Math.floor((s * total) / totalLen), cap))
  let rem = total - alloc.reduce((s, n) => s + n, 0)
  while (rem > 0) {
    let best = -1
    let bestCap = -1
    let bestSize = -1
    for (let i = 0; i < alloc.length; i++) {
      const room = cap - alloc[i]!
      if (room <= 0) continue
      if (room > bestCap || (room === bestCap && sizes[i]! > bestSize)) {
        best = i; bestCap = room; bestSize = sizes[i]!
      }
    }
    if (best < 0) break
    alloc[best]!++
    rem--
  }
  return alloc
}

export function computeBatchSpecs(text: string, typeEntries: TypeEntry[]): BatchSpec[] {
  const active = typeEntries.filter((e) => e.count > 0)
  const totalQ = active.reduce((s, e) => s + e.count, 0)
  if (totalQ <= 0 || !text.trim() || active.length === 0) return []

  const byBudget = Math.ceil(text.length / MAX_CHARS_PER_CHUNK)
  const byQ = Math.ceil(totalQ / MAX_Q_PER_CHUNK)
  const n = Math.max(1, byBudget, byQ)

  const sections = splitByFileSections(text)
  let chunks = chunkByFileProportion(sections, n, text)
  chunks = enforceBudget(chunks)

  const alloc = allocateWithCap(chunks.map((c) => c.length), totalQ, MAX_Q_PER_CHUNK)

  const remaining: Record<string, number> = {}
  for (const e of active) remaining[e.type] = e.count

  const specs: BatchSpec[] = []
  for (let i = 0; i < chunks.length; i++) {
    const count = alloc[i]!
    if (count <= 0) continue
    const typeCounts: Record<string, number> = {}
    let ti = 0
    for (let k = 0; k < count; k++) {
      const avail = active.filter((e) => (remaining[e.type] ?? 0) > 0)
      if (avail.length === 0) break
      const { type } = avail[ti % avail.length]!
      typeCounts[type] = (typeCounts[type] || 0) + 1
      remaining[type]!--
      ti++
    }
    specs.push({ text: chunks[i]!, typeCounts })
  }
  return specs
}
