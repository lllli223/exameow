#!/usr/bin/env python3
"""exameowctl - local CLI bridge to a private, self-hosted Exameow study server.

Dependency-light: Python 3.8+ standard library only. This CLI is the only
bridge ChatGPT (or any local agent) uses to reach the user's private server;
it never connects anywhere except EXAMEOW_BASE_URL and never prints the token.

On Windows, prefer the exameowctl.ps1 launcher, which resolves
EXAMEOW_BASE_URL / EXAMEOW_TOKEN from the persistent User (then Machine)
environment scopes. See README.md in this folder.
"""

from __future__ import annotations

import argparse
import json
import os
import sys

# Sibling modules (stdlib-only). When run as a script, Python puts this file's
# directory first on sys.path, so these imports are safe.
import client
import consumer
import schema
from client import (
    ApiError,
    EXIT_FILE,
    EXIT_OK,
    EXIT_VALIDATION,
    load_config,
    request_json,
)

PROG = "exameowctl"


# ---------------------------------------------------------------------------
# small output helpers


def _force_utf8_streams():
    """Keep CJK content readable when stdout is piped on Windows."""
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError, OSError):
            pass


def compact(payload):
    """Single-line, machine-friendly JSON (non-ASCII preserved)."""
    return json.dumps(payload, ensure_ascii=False, separators=(",", ":"))


def emit_json(payload):
    print(compact(payload))


def pretty(payload):
    print(json.dumps(payload, ensure_ascii=False, indent=2))


def _find_list(payload, names):
    if isinstance(payload, list):
        return payload
    if not isinstance(payload, dict):
        return None
    for name in names:
        value = payload.get(name)
        if isinstance(value, list):
            return value
    return None


def _find_str(payload, names):
    if not isinstance(payload, dict):
        return None
    for name in names:
        value = payload.get(name)
        if isinstance(value, str) and value.strip():
            return value
    return None


def _find_cursor(payload, names):
    if not isinstance(payload, dict):
        return None
    for name in names:
        value = payload.get(name)
        if isinstance(value, bool):
            continue
        if isinstance(value, int) and value >= 0:
            return str(value)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return None


def _print_items(items, indent=""):
    for item in items:
        print(indent + compact(item))


# ---------------------------------------------------------------------------
# file loading


def load_bank_file(path):
    """Read and JSON-parse a bank file (UTF-8, BOM tolerant).

    Returns (data, error_message); exactly one is None.
    """
    try:
        with open(path, "r", encoding="utf-8-sig") as handle:
            text = handle.read()
    except FileNotFoundError:
        return None, "file not found: %s" % path
    except OSError as exc:
        return None, "cannot read %s: %s" % (path, exc.strerror or exc)
    try:
        data = json.loads(text)
    except ValueError as exc:
        return None, "invalid JSON in %s: %s" % (path, exc)
    return data, None


def _print_validation_errors(errors, label):
    print("invalid: %s (%d error(s))" % (label, len(errors)))
    for number, error in enumerate(errors, 1):
        location = error.get("path") or "(bank)"
        print("  %d. %s: %s" % (number, location, error["message"]))


# ---------------------------------------------------------------------------
# commands


def cmd_status(args):
    config = load_config()
    _, payload = request_json(config, "GET", ["health"])
    if args.json:
        emit_json(payload if payload is not None else {})
        return EXIT_OK
    print("server: %s" % config.base_url)
    if isinstance(payload, dict) and payload.get("status"):
        line = "status: %s" % payload["status"]
        for field in ("version", "uptime", "time", "serverTime"):
            if field in payload:
                line += "  %s: %s" % (field, payload[field])
        print(line)
    elif payload is None:
        print("status: ok (empty response body)")
    else:
        pretty(payload)
    return EXIT_OK


def _ack_hint(args):
    parts = []
    if args.consumer != consumer.DEFAULT_CONSUMER:
        parts.append('--consumer "%s"' % args.consumer)
    if args.subject:
        parts.append('--subject "%s"' % args.subject)
    if args.chapter:
        parts.append('--chapter "%s"' % args.chapter)
    if args.bank:
        parts.append('--bank "%s"' % args.bank)
    if args.wrong_only:
        parts.append('--wrong-only')
    return (" " + " ".join(parts)) if parts else ""


def cmd_feed(args):
    config = load_config()
    params = consumer.derive_feed_params(
        args.consumer, args.subject, args.chapter, args.bank, not args.wrong_only
    )
    query = dict(params)
    if args.after:
        query["after"] = args.after
    if args.limit:
        query["limit"] = str(args.limit)
    _, payload = request_json(config, "GET", ["feed"], query=query)
    if args.json:
        emit_json(payload if payload is not None else {})
        return EXIT_OK
    print("consumer: %s" % params["consumer"])
    items = _find_list(payload, ("attempts", "items", "mistakes", "entries", "events", "results", "feed"))
    cursor = _find_cursor(payload, ("nextCursor", "next_cursor", "cursor"))
    if items is not None or cursor is not None:
        if items is not None:
            print("items: %d" % len(items))
            _print_items(items, indent="  ")
        if cursor:
            print("nextCursor: %s" % cursor)
            print("ack with: exameowctl ack \"%s\"%s" % (cursor, _ack_hint(args)))
    else:
        pretty(payload)
    return EXIT_OK


def cmd_ack(args):
    config = load_config()
    params = consumer.derive_feed_params(
        args.consumer, args.subject, args.chapter, args.bank, not args.wrong_only
    )
    body = dict(params)
    body["cursor"] = args.cursor
    _, payload = request_json(config, "POST", ["feed", "ack"], body=body)
    if args.json:
        emit_json(payload if payload is not None else {"ok": True})
        return EXIT_OK
    print("acked cursor \"%s\" for consumer %s" % (args.cursor, params["consumer"]))
    if payload is not None:
        pretty(payload)
    return EXIT_OK


def cmd_session_latest(args):
    config = load_config()
    _, payload = request_json(config, "GET", ["sessions", "latest"])
    if args.json:
        emit_json(payload if payload is not None else {})
        return EXIT_OK
    print("latest session:")
    if payload is None:
        print("  (empty response body)")
    elif isinstance(payload, list):
        print("  entries: %d" % len(payload))
        _print_items(payload, indent="  ")
    else:
        pretty(payload)
    return EXIT_OK


def cmd_question_history(args):
    config = load_config()
    query = {"limit": str(args.limit)} if args.limit else None
    _, payload = request_json(config, "GET",
                              ["questions", args.questionKey, "history"], query=query)
    if args.json:
        emit_json(payload if payload is not None else {})
        return EXIT_OK
    print("question: %s" % args.questionKey)
    items = _find_list(payload, ("items", "history", "events", "records", "attempts"))
    if items is not None:
        print("entries: %d" % len(items))
        _print_items(items, indent="  ")
    else:
        pretty(payload)
    return EXIT_OK


def cmd_bank_validate(args):
    # Fully offline: no env vars, no network.
    data, file_error = load_bank_file(args.file)
    if file_error:
        print("error: %s" % file_error, file=sys.stderr)
        return EXIT_FILE
    errors = schema.validate_bank(data)
    key, question_count = schema.bank_identity(data)
    if args.json:
        emit_json({
            "valid": not errors,
            "bankKey": key,
            "questionCount": question_count,
            "errors": errors,
        })
    elif errors:
        _print_validation_errors(errors, args.file)
    else:
        print("valid: bank %r (%s questions)" % (key or "?", question_count))
    return EXIT_OK if not errors else EXIT_VALIDATION


def cmd_bank_import(args):
    # Validate locally first; invalid banks are never uploaded.
    data, file_error = load_bank_file(args.file)
    if file_error:
        print("error: %s" % file_error, file=sys.stderr)
        return EXIT_FILE
    errors = schema.validate_bank(data)
    key, question_count = schema.bank_identity(data)
    if errors:
        if args.json:
            emit_json({
                "valid": False,
                "imported": False,
                "bankKey": key,
                "questionCount": question_count,
                "errors": errors,
            })
        else:
            print("not imported: %s failed local validation" % args.file)
            for number, error in enumerate(errors, 1):
                location = error.get("path") or "(bank)"
                print("  %d. %s: %s" % (number, location, error["message"]))
        return EXIT_VALIDATION
    config = load_config()
    _, payload = request_json(config, "POST", ["banks", "import"], body=data)
    if args.json:
        emit_json(payload if payload is not None else {"imported": True})
        return EXIT_OK
    server_key = _find_str(payload, ("key", "bankKey")) or key
    print("imported bank %r (%s questions) to %s"
          % (server_key or "?", question_count, config.base_url))
    if payload is not None:
        pretty(payload)
    return EXIT_OK


def cmd_bank_list(args):
    config = load_config()
    _, payload = request_json(config, "GET", ["banks"])
    if args.json:
        emit_json(payload if payload is not None else {})
        return EXIT_OK
    banks = _find_list(payload, ("banks", "items", "results"))
    if banks is None:
        pretty(payload if payload is not None else {})
        return EXIT_OK
    print("banks: %d" % len(banks))
    for bank in banks:
        if isinstance(bank, dict):
            key = bank.get("key") or bank.get("bankKey") or "?"
            line = "  %s" % key
            name = bank.get("name")
            if isinstance(name, str) and name:
                line += "  %s" % name
            count = bank.get("questionCount")
            if count is None and isinstance(bank.get("questions"), list):
                count = len(bank["questions"])
            if isinstance(count, int):
                line += "  (%d questions)" % count
            print(line)
        else:
            print("  " + compact(bank))
    return EXIT_OK


def cmd_bank_show(args):
    config = load_config()
    _, payload = request_json(config, "GET", ["banks", args.bankKey])
    if args.json:
        emit_json(payload if payload is not None else {})
        return EXIT_OK
    print("bank: %s" % args.bankKey)
    if payload is None:
        print("  (empty response body)")
    else:
        pretty(payload)
    return EXIT_OK


# ---------------------------------------------------------------------------
# argument parsing


def positive_int(value):
    try:
        number = int(value)
    except ValueError:
        raise argparse.ArgumentTypeError("invalid integer: %r" % value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return number


def nonnegative_int(value):
    try:
        number = int(value)
    except ValueError:
        raise argparse.ArgumentTypeError("invalid integer: %r" % value)
    if number < 0:
        raise argparse.ArgumentTypeError("must be a non-negative integer")
    return number


def _add_consumer_filters(parser, with_ack_warning=False):
    parser.add_argument(
        "--consumer", default=consumer.DEFAULT_CONSUMER, metavar="NAME",
        help="base consumer (default: %(default)s); a fully-qualified consumer "
             "(containing subject=/chapter= segments) is sent verbatim")
    parser.add_argument("--subject", default=None, metavar="TEXT",
                        help="subject filter; extends the consumer key")
    parser.add_argument("--chapter", default=None, metavar="TEXT",
                        help="chapter filter; extends the consumer key")
    parser.add_argument("--bank", default=None, metavar="BANK_KEY",
                        help="study-bank key filter; extends the consumer key")
    parser.add_argument("--wrong-only", action="store_true",
                        help="exclude flagged-only correct attempts; uses its own cursor namespace")


def build_parser():
    parser = argparse.ArgumentParser(
        prog=PROG,
        description="Local bridge to the Exameow study API (/api/study) on a "
                    "private self-hosted server. Stdlib-only Python 3.8+.",
        epilog="Environment: EXAMEOW_BASE_URL (server root) and EXAMEOW_TOKEN "
               "(bearer token). On Windows use exameowctl.ps1, which resolves "
               "them from User/Machine env scopes. Never shares cursors between "
               "filtered and unfiltered feeds. See tools/exameowctl/README.md.",
    )
    subparsers = parser.add_subparsers(dest="command", metavar="<command>", required=True)

    def add_json(p):
        p.add_argument("--json", action="store_true",
                       help="print one compact machine-friendly JSON document")

    p = subparsers.add_parser("status", help="check server health and credentials")
    add_json(p)
    p.set_defaults(func=cmd_status)

    p = subparsers.add_parser("feed", help="list unseen mistakes from the study feed")
    _add_consumer_filters(p)
    p.add_argument("--after", type=nonnegative_int, default=None, metavar="CURSOR",
                   help="resume after this numeric server cursor")
    p.add_argument("--limit", type=positive_int, default=None, metavar="N",
                   help="maximum number of items to return")
    add_json(p)
    p.set_defaults(func=cmd_feed)

    p = subparsers.add_parser(
        "ack", help="acknowledge mistakes up to a cursor; use the SAME "
                    "--consumer/--subject/--chapter as the feed that produced it")
    p.add_argument("cursor", type=nonnegative_int, help="numeric cursor returned by 'feed'")
    _add_consumer_filters(p)
    add_json(p)
    p.set_defaults(func=cmd_ack)

    p = subparsers.add_parser("session", help="inspect practice sessions")
    session_sub = p.add_subparsers(dest="session_command", required=True)
    sp = session_sub.add_parser("latest", help="show the latest practice session")
    add_json(sp)
    sp.set_defaults(func=cmd_session_latest)

    p = subparsers.add_parser("question", help="inspect questions")
    question_sub = p.add_subparsers(dest="question_command", required=True)
    qp = question_sub.add_parser("history", help="mistake history for one question")
    qp.add_argument("questionKey", help="question stable key (or server key)")
    qp.add_argument("--limit", type=positive_int, default=None, metavar="N",
                    help="maximum number of history entries")
    add_json(qp)
    qp.set_defaults(func=cmd_question_history)

    p = subparsers.add_parser("bank", help="manage study banks (schemaVersion 1)")
    bank_sub = p.add_subparsers(dest="bank_command", required=True)

    bp = bank_sub.add_parser(
        "validate", help="validate a study-bank file offline (no env vars needed)")
    bp.add_argument("file", help="path to the bank JSON file")
    add_json(bp)
    bp.set_defaults(func=cmd_bank_validate)

    bp = bank_sub.add_parser(
        "import", help="validate a study-bank file locally, then upload it")
    bp.add_argument("file", help="path to the bank JSON file")
    add_json(bp)
    bp.set_defaults(func=cmd_bank_import)

    bp = bank_sub.add_parser("list", help="list banks on the server")
    add_json(bp)
    bp.set_defaults(func=cmd_bank_list)

    bp = bank_sub.add_parser("show", help="show one bank from the server")
    bp.add_argument("bankKey", help="bank key")
    add_json(bp)
    bp.set_defaults(func=cmd_bank_show)

    bp = bank_sub.add_parser("get", help="alias for 'bank show'")
    bp.add_argument("bankKey", help="bank key")
    add_json(bp)
    bp.set_defaults(func=cmd_bank_show)

    return parser


def main(argv=None):
    _force_utf8_streams()
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return args.func(args) or EXIT_OK
    except ApiError as exc:
        print("error: %s" % exc, file=sys.stderr)
        return exc.exit_code
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        return 130


if __name__ == "__main__":
    sys.exit(main())
