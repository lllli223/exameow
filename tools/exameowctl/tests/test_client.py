"""Unit tests for client helpers (URL building, config, error mapping, parsing).

Pure functions only - no network access.
"""

import contextlib
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import client  # noqa: E402


_UNSET = object()


@contextlib.contextmanager
def env(base_url=_UNSET, token=_UNSET):
    """Temporarily set/del the two environment variables (None deletes)."""
    old_base = os.environ.get("EXAMEOW_BASE_URL")
    old_token = os.environ.get("EXAMEOW_TOKEN")
    try:
        if base_url is not _UNSET:
            if base_url is None:
                os.environ.pop("EXAMEOW_BASE_URL", None)
            else:
                os.environ["EXAMEOW_BASE_URL"] = base_url
        if token is not _UNSET:
            if token is None:
                os.environ.pop("EXAMEOW_TOKEN", None)
            else:
                os.environ["EXAMEOW_TOKEN"] = token
        yield
    finally:
        for name, old in (("EXAMEOW_BASE_URL", old_base),
                          ("EXAMEOW_TOKEN", old_token)):
            if old is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = old


class BuildUrlTest(unittest.TestCase):
    BASE = "http://192.168.1.10:3000"

    def test_simple_path(self):
        self.assertEqual(client.build_url(self.BASE, ["health"]),
                         "http://192.168.1.10:3000/api/study/health")

    def test_multi_segment_path(self):
        self.assertEqual(client.build_url(self.BASE, ["feed", "ack"]),
                         "http://192.168.1.10:3000/api/study/feed/ack")

    def test_base_url_trailing_slash(self):
        self.assertEqual(client.build_url("http://h/", ["health"]),
                         "http://h/api/study/health")

    def test_path_values_are_url_encoded(self):
        url = client.build_url(self.BASE, ["questions", "电力 安/全", "history"])
        self.assertEqual(
            url,
            "http://192.168.1.10:3000/api/study/questions/"
            "%E7%94%B5%E5%8A%9B%20%E5%AE%89%2F%E5%85%A8/history")

    def test_query_values_are_url_encoded(self):
        url = client.build_url(self.BASE, ["feed"],
                               {"consumer": "chatgpt/subject=电工 基础"})
        self.assertEqual(
            url,
            "http://192.168.1.10:3000/api/study/feed"
            "?consumer=chatgpt%2Fsubject%3D%E7%94%B5%E5%B7%A5+%E5%9F%BA%E7%A1%80")

    def test_query_multiple_values_in_order(self):
        url = client.build_url(self.BASE, ["feed"],
                               {"consumer": "chatgpt", "limit": "10"})
        self.assertEqual(url, "http://192.168.1.10:3000/api/study/feed?consumer=chatgpt&limit=10")

    def test_empty_and_none_query_values_skipped(self):
        url = client.build_url(self.BASE, ["feed"],
                               {"after": None, "subject": "", "consumer": "chatgpt"})
        self.assertEqual(url, "http://192.168.1.10:3000/api/study/feed?consumer=chatgpt")

    def test_no_query_marker_without_values(self):
        self.assertFalse("?" in client.build_url(self.BASE, ["feed"]))


class LoadConfigTest(unittest.TestCase):
    def test_missing_env_vars_exit_config(self):
        with env(base_url=None, token=None):
            with self.assertRaises(client.ApiError) as ctx:
                client.load_config()
        self.assertEqual(ctx.exception.exit_code, client.EXIT_CONFIG)
        self.assertIn("EXAMEOW_BASE_URL", ctx.exception.message)
        self.assertNotIn("secret", ctx.exception.message.lower())

    def test_missing_token_exit_config(self):
        with env(base_url="http://h:3000", token=None):
            with self.assertRaises(client.ApiError) as ctx:
                client.load_config()
        self.assertEqual(ctx.exception.exit_code, client.EXIT_CONFIG)
        self.assertIn("EXAMEOW_TOKEN", ctx.exception.message)

    def test_bad_scheme_exit_config(self):
        with env(base_url="ftp://h", token="t"):
            with self.assertRaises(client.ApiError) as ctx:
                client.load_config()
        self.assertEqual(ctx.exception.exit_code, client.EXIT_CONFIG)

    def test_whitespace_in_base_url_exit_config(self):
        with env(base_url="http://h:3000 hidden", token="t"):
            with self.assertRaises(client.ApiError) as ctx:
                client.load_config()
        self.assertEqual(ctx.exception.exit_code, client.EXIT_CONFIG)

    def test_values_are_trimmed(self):
        with env(base_url=" http://h:3000/ ", token="  t  "):
            config = client.load_config()
        self.assertEqual(config.base_url, "http://h:3000")
        self.assertEqual(config.token, "t")

    def test_config_error_messages_never_contain_token_value(self):
        with env(base_url="http://h:3000", token=None):
            try:
                client.load_config()
                self.fail("expected ApiError")
            except client.ApiError as exc:
                # The env var NAME may appear; a real token value never would,
                # but assert the message only mentions the variable name.
                self.assertIn("EXAMEOW_TOKEN", exc.message)
                self.assertNotIn("Bearer", exc.message)


class StatusCodeMappingTest(unittest.TestCase):
    def test_auth_errors(self):
        self.assertEqual(client.exit_code_for_status(401), client.EXIT_AUTH)
        self.assertEqual(client.exit_code_for_status(403), client.EXIT_AUTH)

    def test_client_errors(self):
        self.assertEqual(client.exit_code_for_status(400), client.EXIT_CLIENT)
        self.assertEqual(client.exit_code_for_status(404), client.EXIT_CLIENT)
        self.assertEqual(client.exit_code_for_status(422), client.EXIT_CLIENT)

    def test_server_errors(self):
        self.assertEqual(client.exit_code_for_status(500), client.EXIT_SERVER)
        self.assertEqual(client.exit_code_for_status(502), client.EXIT_SERVER)
        self.assertEqual(client.exit_code_for_status(503), client.EXIT_SERVER)


class ExtractErrorMessageTest(unittest.TestCase):
    def test_json_error_field(self):
        self.assertEqual(client.extract_error_message(b'{"error": "boom"}'), "boom")

    def test_json_message_field(self):
        self.assertEqual(client.extract_error_message(b'{"message": "nope"}'), "nope")

    def test_html_is_stripped_and_truncated(self):
        raw = b"<html><body>Service Unavailable</body></html>"
        self.assertEqual(client.extract_error_message(raw), "Service Unavailable")

    def test_long_text_truncated(self):
        raw = ("x" * 500).encode("ascii")
        message = client.extract_error_message(raw)
        self.assertTrue(len(message) <= 303)
        self.assertTrue(message.endswith("..."))

    def test_empty_body(self):
        self.assertEqual(client.extract_error_message(b""), "")
        self.assertEqual(client.extract_error_message(b"   "), "")


class ParseBodyTest(unittest.TestCase):
    def test_json_body(self):
        self.assertEqual(client.parse_body(b'{"a": 1}'), {"a": 1})

    def test_empty_body(self):
        self.assertIsNone(client.parse_body(b""))

    def test_non_json_body_wrapped(self):
        parsed = client.parse_body("plain text".encode("utf-8"))
        self.assertEqual(parsed, {"raw": "plain text"})


if __name__ == "__main__":
    unittest.main()
