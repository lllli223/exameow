# exameowctl - Exameow study bridge CLI

`exameowctl` is a small, dependency-light (Python 3.8+ stdlib only) command line bridge between ChatGPT - or any local agent - and a **private, self-hosted Exameow study server**. It talks to the server's `/api/study` endpoints over HTTP with Bearer auth, validates study-bank files offline against **schemaVersion 1**, and prints machine-friendly JSON for scripted use.

This folder is self-contained: no frontend, Rust, or Worker code is required or modified. The CLI connects nowhere except `EXAMEOW_BASE_URL`, and it never prints, echoes, or logs your token.

## Files

| File | Purpose |
|---|---|
| `exameowctl.ps1` | Windows launcher: resolves env vars from User/Machine scopes, then runs the Python CLI |`n| `exameowctl.cmd` | CMD/PATH-friendly wrapper around `exameowctl.ps1` |
| `exameowctl.py` | CLI entry point (stdlib only) |
| `client.py` | HTTP client for `/api/study` (urllib, Bearer auth, exit-code mapping) |
| `consumer.py` | Consumer-key derivation + filter normalization (feed/ack cursor namespaces) |
| `schema.py` | Offline study-bank validation (schemaVersion 1) |
| `example_bank.json` | Example bank exercising every schema rule |`n| `../../schemas/stategrid-bank.schema.json` | Canonical schemaVersion 1 JSON Schema reference |
| `tests/` | `unittest` suite: schema, consumer derivation, client helpers |

## Server-side requirement

The self-hosted server must be started with a non-empty `STUDY_SYNC_TOKEN`. The Docker Compose files in this branch pass that variable into the server and persist `/app/data` so study history survives container recreation. Generate a long random token and provide it through the deployment environment; do not commit it to Git. The Windows `EXAMEOW_TOKEN` value must match the server `STUDY_SYNC_TOKEN`.

## Setup (Windows)

1. Install Python 3.8+ (`python --version` or `py -3 --version` must work).
2. Persist the two variables once, per user:
   ```powershell
   setx EXAMEOW_BASE_URL "http://192.168.1.10:3000"
   setx EXAMEOW_TOKEN "your-secret-token"
   ```
   `setx` writes the **User** scope. Admins may instead set machine-wide variables (**Machine** scope); see the resolution order below. Open a fresh shell afterwards.
3. If scripts are blocked, allow them for your user:
   ```powershell
   Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
   ```
4. Run from anywhere (pass the full path, or add this folder to PATH):
   ```powershell
   C:\path\to\exameowctl\exameowctl.ps1 status
   ```

### Environment resolution in `exameowctl.ps1`

For each of `EXAMEOW_BASE_URL` and `EXAMEOW_TOKEN`:

1. Read the **User** scope (`HKCU\Environment`).
2. If that value is empty/whitespace, read the **Machine** scope.
3. If a value was found, inject it into the current process (`$env:`), overwriting whatever the parent process had.
4. If neither scope has a value, the process environment is left untouched (so a manually exported variable still works on non-Windows shells).
5. The resolved values are handed to Python only - never echoed, printed, or logged.

The launcher prefers the `py -3` launcher, falls back to `python` on PATH, forces UTF-8 output, and propagates the CLI's exit code. Use **double-dash flags** (`--json`, not `-json`); PowerShell forwards those verbatim.

Direct (non-Windows, or no wrapper): export `EXAMEOW_BASE_URL` and `EXAMEOW_TOKEN` yourself and run `python exameowctl.py <command>`.

## Commands

| Command | What it does |
|---|---|
| `status` | `GET /api/study/health`; verifies reachability + credentials |
| `feed [--consumer chatgpt] [--subject S] [--chapter C] [--bank KEY] [--wrong-only] [--after CURSOR] [--limit N] [--json]` | list unseen wrong/flagged review events |
| `ack <cursor> [--consumer chatgpt] [--subject S] [--chapter C] [--bank KEY] [--wrong-only] [--json]` | acknowledge the exact matching feed namespace up to a cursor |
| `session latest [--json]` | latest practice session |
| `question history <questionKey> [--limit N] [--json]` | mistake history for one question |
| `bank validate <file> [--json]` | offline schema check (needs no env vars) |
| `bank import <file> [--json]` | validate locally first, then upload |
| `bank list [--json]` | list banks on the server |
| `bank show <bankKey> [--json]` / `bank get <bankKey> [--json]` | fetch one bank |

Every network command reads `EXAMEOW_BASE_URL`/`EXAMEOW_TOKEN` from the environment. `bank validate` is fully offline.

Banks uploaded with `bank import` are centrally stored. The Web/mobile practice page discovers them through the study API and pulls only new or changed banks into its local-first bank list, using the server content hash to skip unchanged payloads.

## Consumer keys and ACK safety

The server tracks each feed's read cursor **per exact consumer key**. `feed` and `ack` derive one exact key from the same flags:

```
key = <base consumer>                                  (default: chatgpt, override with --consumer)
key += "/subject=<normalized subject>"                  (if --subject given)
key += "/chapter=<normalized chapter>"                  (if --chapter given)
key += "/bank=<bank key>"                                 (if --bank given)
key += "/flagged=0"                                       (if --wrong-only)
```

Normalization trims, collapses all whitespace (including full-width spaces) to single spaces, and casefolds, so `" 电工基础 "` and `"电工基础"` map to the same key.

| Flags | Consumer key |
|---|---|
| *(none)* | `chatgpt` |
| `--subject 电工基础` | `chatgpt/subject=电工基础` |
| `--chapter 1` | `chatgpt/chapter=1` |
| `--subject 电工基础 --chapter 1` | `chatgpt/subject=电工基础/chapter=1` |
| `--bank sgcc-iot` | `chatgpt/bank=sgcc-iot` |
| `--wrong-only` | `chatgpt/flagged=0` |
| `--consumer chatgpt/subject=电工基础` | namespace preserved; encoded filters are replayed |

If `--consumer` already contains `subject=`, `chapter=`, `bank=`, or `flagged=` segments it counts as **fully qualified**. The namespace stays verbatim and its encoded filters are replayed to the server.

**ACK rule.** Always run `ack <cursor>` with the *same* `--consumer/--subject/--chapter/--bank/--wrong-only` flags as the `feed` call that produced the cursor (or with the fully-qualified consumer shown by that feed). Consequences:

- Filtered feeds use their own keys, so a filtered ack can never accidentally consume the unfiltered cursor, and vice versa.
- But an ack run with *different* filters derives a different key, lands in a different namespace, and will not advance the feed you actually read - unseen mistakes there stay unseen.
- Human-mode output prints the derived key and a ready-to-paste `ack` command line so mismatches are visible before they bite.

## Study-bank schema (schemaVersion 1)

Bank object:

| Field | Required | Rules |
|---|---|---|
| `schemaVersion` | yes | must be exactly `1` |
| `key` | yes | 3-128 chars; portable ASCII `[A-Za-z0-9._:-]` only |
| `name` | yes | non-empty string |
| `version` | yes | integer `>= 1`; increment when the logical bank content version changes |
| `metadata` | yes | object with non-empty `exam`, `outlineVersion`, and `subject` strings |
| `questions` | yes | list of question objects (may be empty) |
| `subject`, `chapter` | no | non-empty strings |
| `tags` | no | list of non-empty, unique strings |
| `sourceMeta` | no | free-form object |

Question object:

| Field | Required | Rules |
|---|---|---|
| `id` or `stableKey` | one of the two, at least | each must use the same 3-128 character portable-ASCII rules as `key`; each field must be unique within the bank |
| `type` | yes | `single_choice`, `multi_choice`, `true_false`, `fill_blank` (SGCC workflow; `short_answer` is rejected) |
| `stem` | yes | non-empty string |
| `options` | choice types only | required for `single_choice`/`multi_choice`, forbidden otherwise; exactly 2-5 non-empty strings |
| `answer` | yes | see below |
| `analysis` | yes | non-empty string |
| `subject`, `chapter`, `knowledgePoint` | no | non-empty strings |
| `difficulty` | no | `easy`, `medium`, `hard` |
| `tags` | no | list of non-empty, unique strings |
| `sourceMeta` | no | free-form object |

Answer rules:

- `single_choice`: exactly one uppercase option letter (e.g. `"A"`), which must exist among the options.
- `multi_choice`: string of distinct uppercase option letters (e.g. `"ABD"`); every letter must exist among the options. Alphabetical order is not required.
- `true_false`: exactly `"true"` or `"false"`.
- `fill_blank`: non-empty string.

Question IDs and stable keys must each be unique within one bank (duplicates are rejected). They must be deterministic - derive them from content, not position, e.g. `sha256(stem + "|" + answer + "|" + analysis)` truncated to 12-16 hex chars, or a manually assigned ID that never changes if the question text is edited. `bank validate` cannot verify determinism, but the example bank and this rule keep namespaces stable across imports.

Unknown fields are rejected at both bank and question level to catch typos early. See `example_bank.json` for a complete bank (Chinese SGCC-style content, all four question types, one `id`-only question).

## HTTP contract

All requests: `Authorization: Bearer EXAMEOW_TOKEN`, `Accept: application/json`, 30 s timeout, direct connection (system proxies are bypassed - the server is private). Path segments and query values are URL-encoded.

| Command | Request |
|---|---|
| `status` | `GET /api/study/health` |
| `feed` | `GET /api/study/feed?consumer=<key>&subject=<normalized>&chapter=<normalized>&after=<cursor>&limit=<n>` (filters/`after`/`limit` only when given) |
| `ack` | `POST /api/study/feed/ack` with body `{"consumer": "<key>", "cursor": <integer>, "subject": ..., "chapter": ...}` (subject/chapter only when derived from flags) |
| `session latest` | `GET /api/study/sessions/latest` |
| `question history` | `GET /api/study/questions/<questionKey>/history?limit=<n>` |
| `bank import` | `POST /api/study/banks/import` with the validated bank JSON as body |
| `bank list` | `GET /api/study/banks` |
| `bank show` / `bank get` | `GET /api/study/banks/<bankKey>` |

Server responses are parsed as JSON and passed through verbatim. A non-JSON body is wrapped as `{"raw": "..."}`. Human-mode output recognizes the server's `attempts` list and numeric `nextCursor` (plus a few compatibility field names) and otherwise pretty-prints the whole payload.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | unexpected internal error |
| 2 | usage error (bad arguments) |
| 3 | configuration error (missing/invalid `EXAMEOW_BASE_URL` / `EXAMEOW_TOKEN`) |
| 4 | network error (unreachable, timeout, reset) |
| 5 | authentication failed (HTTP 401/403) |
| 6 | other HTTP 4xx (bad request, not found) |
| 7 | HTTP 5xx |
| 8 | bank validation failure (schema errors; for `bank import` this means nothing was uploaded) |
| 9 | file error (missing file, unreadable, invalid JSON) |
| 130 | interrupted (Ctrl+C) |

## JSON output

`--json` prints exactly one compact JSON document on stdout (UTF-8, non-ASCII preserved) so scripts and agents can parse the result directly. `bank validate`/`bank import` emit `{"valid": bool, "bankKey": ..., "questionCount": ..., "errors": [{"path", "message"}]}` (plus `"imported": false` for a failed import). Errors and diagnostics always go to stderr, never mixed into the JSON on stdout.

## Tests

```powershell
cd tools/exameowctl
python -m unittest discover -s tests -t . -v
python -m py_compile exameowctl.py client.py consumer.py schema.py
```

No network access, no environment variables required.

## Assumptions and limits

- The server side of `/api/study` is the user's private deployment; this CLI only implements the client, and passes response payloads through rather than assuming a fixed shape.
- Only Bearer-token auth is supported; TLS must be valid (or use plain HTTP on a trusted LAN).
- Feed cursors are non-negative, monotonic server event-sequence integers. Treat them as server-owned positions: only ACK a cursor returned by the matching consumer/filter feed.
