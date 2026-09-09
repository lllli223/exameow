import unittest
from unittest.mock import patch

import exameowctl


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


if __name__ == "__main__":
    unittest.main()
