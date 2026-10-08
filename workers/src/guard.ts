export interface GuardEnv {
  ALLOWED_ORIGINS?: string
}

const BUILTIN_ORIGINS = [
  'https://exam.superagentparty.com',
  'https://superagentparty.com',
  'https://www.superagentparty.com',
  'tauri://localhost',
  'http://tauri.localhost',
  'https://tauri.localhost',
]

const LOCAL_ORIGIN = /^https?:\/\/(localhost|127\.0\.0\.1|\[::1\])(:\d+)?$/

export function clientIp(req: Request): string {
  return req.headers.get('CF-Connecting-IP') || 'unknown'
}

export function originAllowed(origin: string | null, env: GuardEnv): boolean {
  if (!origin) return true
  if (LOCAL_ORIGIN.test(origin)) return true
  const extra = (env.ALLOWED_ORIGINS || '')
    .split(',')
    .map(value => value.trim())
    .filter(Boolean)
  return BUILTIN_ORIGINS.includes(origin) || extra.includes(origin)
}

/** Best-effort per-IP counter in the free Cache API: no D1 writes, no KV quota. */
export async function allowRequest(
  req: Request,
  bucket: string,
  limit: number,
  windowSeconds: number,
): Promise<boolean> {
  const slot = Math.floor(Date.now() / (windowSeconds * 1000))
  const key = new Request(
    `https://exameow-ratelimit.internal/${bucket}/${encodeURIComponent(clientIp(req))}/${slot}`,
  )
  try {
    const cache = caches.default
    const cached = await cache.match(key)
    const used = cached ? Number(await cached.text()) || 0 : 0
    if (used >= limit) return false
    await cache.put(
      key,
      new Response(String(used + 1), { headers: { 'Cache-Control': `max-age=${windowSeconds * 2}` } }),
    )
  } catch {
    // A cache failure must never take requests down; the other guards still apply.
  }
  return true
}

export function bodyTooLarge(req: Request, maxBytes: number): boolean {
  const header = req.headers.get('content-length')
  if (!header) return false
  const length = Number(header)
  return Number.isFinite(length) && length > maxBytes
}

export async function readJsonCapped(
  req: Request,
  maxBytes: number,
): Promise<{ ok: true; value: unknown } | { ok: false; status: 400 | 413; error: string }> {
  if (bodyTooLarge(req, maxBytes)) return { ok: false, status: 413, error: 'Payload too large' }
  let text: string
  try {
    text = await req.text()
  } catch {
    return { ok: false, status: 400, error: 'Invalid request body' }
  }
  if (text.length > maxBytes) return { ok: false, status: 413, error: 'Payload too large' }
  try {
    return { ok: true, value: JSON.parse(text) }
  } catch {
    return { ok: false, status: 400, error: 'Invalid JSON body' }
  }
}

export function clampText(value: unknown, maxLength: number): string {
  return typeof value === 'string' ? value.slice(0, maxLength) : ''
}

export function clampNumber(value: unknown, min: number, max: number, fallback: number): number {
  const parsed = typeof value === 'number' && Number.isFinite(value) ? Math.floor(value) : fallback
  return Math.min(Math.max(parsed, min), max)
}

const CF_MODEL = /^@cf\/[a-z0-9._-]+\/[a-z0-9._-]+$/i

export function sanitizeModel(model: unknown): string | undefined {
  return typeof model === 'string' && model.length <= 100 && CF_MODEL.test(model) ? model : undefined
}
