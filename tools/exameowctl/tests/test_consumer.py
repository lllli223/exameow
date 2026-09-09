"""Unit tests for consumer-key derivation and filter normalization."""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import consumer  # noqa: E402


class NormalizeFilterTest(unittest.TestCase):
    def test_none_is_empty(self):
        self.assertEqual(consumer.normalize_filter(None), "")

    def test_trims_and_collapses_whitespace(self):
        self.assertEqual(consumer.normalize_filter("  电工\t基础  "), "电工 基础")

    def test_collapses_fullwidth_spaces(self):
        self.assertEqual(consumer.normalize_filter("　电工\u3000　基础"), "电工 基础")

    def test_casefolds_ascii(self):
        self.assertEqual(consumer.normalize_filter("Power Systems"), "power systems")

    def test_whitespace_only_is_empty(self):
        self.assertEqual(consumer.normalize_filter("   \u3000  "), "")

    def test_non_string_is_stringified(self):
        self.assertEqual(consumer.normalize_filter(42), "42")


class IsFullyQualifiedTest(unittest.TestCase):
    def test_plain_base(self):
        self.assertFalse(consumer.is_fully_qualified("chatgpt"))

    def test_subject_segment(self):
        self.assertTrue(consumer.is_fully_qualified("chatgpt/subject=电工基础"))

    def test_chapter_segment(self):
        self.assertTrue(consumer.is_fully_qualified("chatgpt/chapter=1"))

    def test_slash_in_base_is_not_qualified(self):
        self.assertFalse(consumer.is_fully_qualified("prod/chatgpt"))

    def test_empty(self):
        self.assertFalse(consumer.is_fully_qualified(""))
        self.assertFalse(consumer.is_fully_qualified(None))


class DeriveConsumerKeyTest(unittest.TestCase):
    def test_default_consumer(self):
        self.assertEqual(consumer.derive_consumer_key(), "chatgpt")

    def test_explicit_base(self):
        self.assertEqual(consumer.derive_consumer_key("openai"), "openai")

    def test_base_whitespace_stripped(self):
        self.assertEqual(consumer.derive_consumer_key("  chatgpt  "), "chatgpt")

    def test_base_with_slash_plus_filters(self):
        self.assertEqual(consumer.derive_consumer_key("prod/chatgpt", subject="s"),
                         "prod/chatgpt/subject=s")

    def test_subject_appended(self):
        self.assertEqual(consumer.derive_consumer_key("chatgpt", subject="电工基础"),
                         "chatgpt/subject=电工基础")

    def test_chapter_appended(self):
        self.assertEqual(consumer.derive_consumer_key("chatgpt", chapter="第一章"),
                         "chatgpt/chapter=第一章")

    def test_subject_then_chapter_canonical_order(self):
        self.assertEqual(
            consumer.derive_consumer_key("chatgpt", chapter="第一章", subject="电工基础"),
            "chatgpt/subject=电工基础/chapter=第一章")

    def test_subject_normalized_in_key(self):
        self.assertEqual(
            consumer.derive_consumer_key("chatgpt", subject="  电工\t基础 "),
            "chatgpt/subject=电工 基础")

    def test_subject_casefolded_in_key(self):
        self.assertEqual(consumer.derive_consumer_key("chatgpt", subject="Power Systems"),
                         "chatgpt/subject=power systems")

    def test_blank_filters_ignored(self):
        self.assertEqual(consumer.derive_consumer_key("chatgpt", subject="  ", chapter=""),
                         "chatgpt")

    def test_fully_qualified_consumer_verbatim(self):
        self.assertEqual(
            consumer.derive_consumer_key("chatgpt/subject=电工基础", subject="其他"),
            "chatgpt/subject=电工基础")

    def test_fully_qualified_chapter_consumer_verbatim(self):
        self.assertEqual(
            consumer.derive_consumer_key("chatgpt/chapter=2", chapter="9"),
            "chatgpt/chapter=2")

    def test_fully_qualified_preserves_unnormalized_form(self):
        # Verbatim means verbatim: no normalization is applied.
        self.assertEqual(
            consumer.derive_consumer_key("chatgpt/subject=Raw Value"),
            "chatgpt/subject=Raw Value")

    def test_derivation_is_deterministic_across_casing_and_spacing(self):
        a = consumer.derive_consumer_key("chatgpt", subject="  电工基础 ")
        b = consumer.derive_consumer_key("chatgpt", subject="电工基础")
        self.assertEqual(a, b)

    def test_filtered_key_differs_from_unfiltered(self):
        # The core safety property: filtered feeds never share the
        # unfiltered cursor namespace (and vice versa).
        plain = consumer.derive_consumer_key("chatgpt")
        filtered = consumer.derive_consumer_key("chatgpt", subject="电工基础")
        self.assertNotEqual(plain, filtered)

    def test_different_subjects_do_not_share_namespace(self):
        a = consumer.derive_consumer_key("chatgpt", subject="电工基础")
        b = consumer.derive_consumer_key("chatgpt", subject="继电保护")
        self.assertNotEqual(a, b)

    def test_subject_with_slash_round_trips_as_fully_qualified(self):
        key = consumer.derive_consumer_key("chatgpt", subject="a/b")
        self.assertEqual(key, "chatgpt/subject=a/b")
        # Feeding the key back as a consumer must target the same namespace.
        self.assertEqual(consumer.derive_consumer_key(key), key)


class DeriveFeedParamsTest(unittest.TestCase):
    def test_defaults(self):
        self.assertEqual(consumer.derive_feed_params(), {"consumer": "chatgpt"})

    def test_filters_included_as_params(self):
        params = consumer.derive_feed_params(None, " 电工 基础 ", "第一章")
        self.assertEqual(params, {
            "consumer": "chatgpt/subject=电工 基础/chapter=第一章",
            "subject": "电工 基础",
            "chapter": "第一章",
        })

    def test_fully_qualified_sends_no_extra_filters(self):
        params = consumer.derive_feed_params("chatgpt/subject=x", "y", "z")
        self.assertEqual(params, {"consumer": "chatgpt/subject=x"})

    def test_blank_filters_omitted(self):
        params = consumer.derive_feed_params("chatgpt", "  ", None)
        self.assertEqual(params, {"consumer": "chatgpt"})

    def test_feed_and_ack_share_params(self):
        # ack must be able to derive the exact same namespace as feed.
        feed_params = consumer.derive_feed_params(None, subject="电工基础", chapter="1")
        ack_params = consumer.derive_feed_params(None, subject="电工基础", chapter="1")
        self.assertEqual(feed_params, ack_params)


if __name__ == "__main__":
    unittest.main()
