import os
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import exameowctl  # noqa: E402


class FeedContractTest(unittest.TestCase):
    def test_numeric_cursor_is_displayable(self):
        payload = {"nextCursor": 7}
        self.assertEqual(exameowctl._find_cursor(payload, ("nextCursor",)), "7")

    def test_feed_attempts_key_is_detected(self):
        payload = {"attempts": [{"seq": 1}]}
        items = exameowctl._find_list(payload, ("attempts", "items"))
        self.assertEqual(items, [{"seq": 1}])

    def test_ack_cursor_is_json_integer(self):
        parser = exameowctl.build_parser()
        args = parser.parse_args(["ack", "12", "--json"])
        self.assertEqual(args.cursor, 12)
        self.assertIsInstance(args.cursor, int)

    def test_after_cursor_is_integer(self):
        parser = exameowctl.build_parser()
        args = parser.parse_args(["feed", "--after", "9", "--json"])
        self.assertEqual(args.after, 9)
        self.assertIsInstance(args.after, int)

    def test_bank_and_wrong_only_filters_parse_for_feed_and_ack(self):
        parser = exameowctl.build_parser()
        feed = parser.parse_args(["feed", "--bank", "sgcc-iot", "--wrong-only"])
        ack = parser.parse_args(["ack", "7", "--bank", "sgcc-iot", "--wrong-only"])
        self.assertEqual(feed.bank, "sgcc-iot")
        self.assertTrue(feed.wrong_only)
        self.assertEqual(ack.bank, "sgcc-iot")
        self.assertTrue(ack.wrong_only)

    def test_bank_get_alias_uses_show_handler(self):
        parser = exameowctl.build_parser()
        args = parser.parse_args(["bank", "get", "sgcc-iot", "--json"])
        self.assertEqual(args.bankKey, "sgcc-iot")
        self.assertIs(args.func, exameowctl.cmd_bank_show)


if __name__ == "__main__":
    unittest.main()
