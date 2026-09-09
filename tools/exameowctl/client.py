"""HTTP client for the Exameow study API (/api/study) - stdlib urllib only.

Bearer-token auth. The token is only ever placed in the Authorization header;
it is never included in error messages, printed, or logged. System proxies are
bypassed on purpose: the study server is a private, self-hosted endpoint that
should always be reached directly.
"""

from __future__ import annotations

import json
import os
import re
import urllib.error
import urllib.parse
import urllib.request

DEFAULT_TIMEOUT = 30.0

EXIT_OK = 0
EXIT_GENERAL = 1
EXIT_USAGE = 2  # reserved for argparse
EXIT_CONFIG = 3
EXIT_NETWORK = 4
EXIT_AUTH = 5
EXIT_CLIENT = 6
EXIT_SERVER = 7
EXIT_VALIDATION = 8
EXIT_FILE = 9

_API_PREFIX = ("api", "study")
_TAG_RE = re.compile(r"<[^>]+>")

# Bypass HTTP(S)_PROXY etc.: direct connection to the private server.
_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


class ApiError(Exception):
    """Fatal CLI error carrying a process exit code; message is safe to print."""

    def __init__(self, exit_code, message):
        super().__init__(message)
        self.exit_code = exit_code
        self.message = message

    def __str__(self):
        return self.message


class Config(object):
    __slots__ = ("base_url", "token")

    def __init__(self, base_url, token):
        self.base_url = base_url
        self.token = token


def load_config():
    """Read EXAMEOW_BASE_URL / EXAMEOW_TOKEN from the environment.

    Raises ApiError(EXIT_CONFIG, ...) with a concise, secret-free message.
    """
    base_url = (os.environ.get("EXAMEOW_BASE_URL") or "").strip()
    token = (os.environ.get("EXAMEOW_TOKEN") or "").strip()
    if not base_url:
        raise ApiError(
            EXIT_CONFIG,
            "EXAMEOW_BASE_URL is not set (expected the server root, e.g. "
            "http://192.168.1.10:3000); on Windows persist it with: "
            'setx EXAMEOW_BASE_URL "http://..."')
    if not base_url.startswith(("http://", "https://")):
        raise ApiError(EXIT_CONFIG, "EXAMEOW_BASE_URL must start with http:// or https://")
    if any(ch.isspace() for ch in base_url):
        raise ApiError(EXIT_CONFIG, "EXAMEOW_BASE_URL must not contain whitespace")
    if not token:
        raise ApiError(
            EXIT_CONFIG,
            "EXAMEOW_TOKEN is not set; on Windows persist it with: "
            'setx EXAMEOW_TOKEN "<token>" (see tools/exameowctl/README.md)')
    return Config(base_url.rstrip("/"), token)


def quote_path_segment(segment):
    """Percent-encode one path segment; nothing is left unescaped."""
    return urllib.parse.quote(str(segment), safe="")


def build_url(base_url, path_segments, query=None):
    """Build an /api/study URL with URL-encoded path and query values.

    Query entries with None/"" values are skipped.
    """
    parts = list(_API_PREFIX) + [quote_path_segment(segment) for segment in path_segments]
    url = (base_url or "").rstrip("/") + "/" + "/".join(parts)
    if query:
        pairs = [(key, str(value)) for key, value in query.items() if value not in (None, "")]
        if pairs:
            url += "?" + urllib.parse.urlencode(pairs)
    return url


def exit_code_for_status(status):
    if status in (401, 403):
        return EXIT_AUTH
    if 400 <= status < 500:
        return EXIT_CLIENT
    return EXIT_SERVER


def _clean_text(text):
    text = _TAG_RE.sub(" ", text)
    return " ".join(text.split())


def extract_error_message(raw, limit=300):
    """Best-effort, secret-free message from an HTTP error body.

    Prefers JSON {"error"|"message"|"detail": "..."} fields; falls back to
    tag-stripped plain text. Always truncated.
    """
    if not raw:
        return ""
    text = raw.decode("utf-8", "replace").strip()
    if not text:
        return ""
    try:
        parsed = json.loads(text)
    except ValueError:
        cleaned = _clean_text(text)
        if not cleaned:
            return ""
        return cleaned[:limit] + ("..." if len(cleaned) > limit else "")
    if isinstance(parsed, dict):
        for key in ("error", "message", "detail"):
            value = parsed.get(key)
            if isinstance(value, str) and value.strip():
                value = value.strip()
                return value[:limit] + ("..." if len(value) > limit else "")
        if parsed:
            return json.dumps(parsed, ensure_ascii=False)[:limit]
        return ""
    return str(parsed)[:limit]


def _reason_text(reason):
    if reason is None:
        return "unknown network error"
    strerror = getattr(reason, "strerror", None)
    text = _clean_text(strerror or str(reason)) or str(reason)
    return text[:200]


def parse_body(raw):
    """Parse a response body as JSON; non-JSON bodies are wrapped as {"raw": ...}."""
    if not raw:
        return None
    text = raw.decode("utf-8", "replace")
    try:
        return json.loads(text)
    except ValueError:
        return {"raw": text.strip()[:4000]}


def request_json(config, method, path_segments, query=None, body=None,
                 timeout=DEFAULT_TIMEOUT):
    """Perform one API call; returns (status, payload) and raises ApiError on failure.

    `body` must be JSON-serializable and is sent as UTF-8 JSON.
    """
    url = build_url(config.base_url, path_segments, query)
    data = None
    headers = {
        "Accept": "application/json",
        "Authorization": "Bearer " + config.token,
    }
    if body is not None:
        data = json.dumps(body, ensure_ascii=False).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with _OPENER.open(request, timeout=timeout) as response:
            status = response.status
            raw = response.read()
    except urllib.error.HTTPError as exc:
        status = exc.code
        try:
            raw = exc.read()
        except OSError:
            raw = b""
        detail = extract_error_message(raw)
        message = "HTTP %d %s" % (status, exc.reason)
        if detail:
            message += ": %s" % detail
        raise ApiError(exit_code_for_status(status), message)
    except urllib.error.URLError as exc:
        raise ApiError(EXIT_NETWORK,
                       "cannot reach %s: %s" % (config.base_url, _reason_text(exc.reason)))
    except OSError as exc:
        # e.g. connection reset mid-response, bad status line, read timeout
        raise ApiError(EXIT_NETWORK,
                       "connection to %s failed: %s" % (config.base_url, _reason_text(exc)))
    return status, parse_body(raw)
