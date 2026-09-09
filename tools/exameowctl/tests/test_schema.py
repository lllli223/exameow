"""Unit tests for the study-bank schema (schemaVersion 1) validator."""

import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import schema  # noqa: E402

TOOL_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXAMPLE_BANK = os.path.join(TOOL_DIR, "example_bank.json")


def make_question(**overrides):
    question_type = overrides.get("type", "single_choice")
    question = {
        "stableKey": "q-1",
        "type": question_type,
        "stem": "Stem?",
        "answer": "A",
        "analysis": "Because.",
    }
    # Default options only make sense on choice questions (they can still be
    # overridden or explicitly provided for the forbidden-options tests).
    if question_type in ("single_choice", "multi_choice") or "options" in overrides:
        question["options"] = ["Option A", "Option B", "Option C", "Option D"]
    question.update(overrides)
    return question


def make_bank(**overrides):
    bank = {
        "schemaVersion": 1,
        "key": "bank-1",
        "name": "Bank One",
        "questions": [make_question()],
    }
    bank.update(overrides)
    return bank


class SchemaTestCase(unittest.TestCase):
    def assert_invalid(self, data, *substrings):
        errors = schema.validate_bank(data)
        combined = " | ".join("%s: %s" % (e["path"], e["message"]) for e in errors)
        for substring in substrings:
            matched = any(substring in e["message"] or substring in e["path"]
                          for e in errors)
            self.assertTrue(
                matched,
                "expected %r among errors, got: %s" % (substring, combined or "(none)"))
        return errors

    def assert_valid(self, data):
        errors = schema.validate_bank(data)
        self.assertEqual(errors, [], "expected a valid bank, got: %r" % (errors,))


class ExampleBankTest(SchemaTestCase):
    def test_example_bank_file_is_valid(self):
        with open(EXAMPLE_BANK, "r", encoding="utf-8-sig") as handle:
            data = json.load(handle)
        self.assert_valid(data)

    def test_example_bank_uses_all_four_types(self):
        with open(EXAMPLE_BANK, "r", encoding="utf-8-sig") as handle:
            data = json.load(handle)
        types = {q["type"] for q in data["questions"]}
        self.assertEqual(types, {"single_choice", "multi_choice", "true_false", "fill_blank"})


class BankLevelTest(SchemaTestCase):
    def test_minimal_bank_is_valid(self):
        self.assert_valid(make_bank())

    def test_empty_question_list_is_valid(self):
        self.assert_valid(make_bank(questions=[]))

    def test_full_optional_fields_are_valid(self):
        self.assert_valid(make_bank(
            subject="电气安全",
            chapter="第一章",
            tags=["sgcc", "安全"],
            sourceMeta={"origin": "test"},
        ))

    def test_root_must_be_object(self):
        self.assert_invalid([1, 2, 3], "must be a JSON object")
        self.assert_invalid("not an object", "must be a JSON object")

    def test_missing_required_fields(self):
        self.assert_invalid({"schemaVersion": 1}, "missing required field: key")
        self.assert_invalid({"key": "k", "schemaVersion": 1}, "missing required field: name")
        self.assert_invalid({"key": "k", "name": "n"}, "missing required field: schemaVersion")

    def test_schema_version_must_be_one(self):
        self.assert_invalid(make_bank(schemaVersion=2), "unsupported schemaVersion")
        self.assert_invalid(make_bank(schemaVersion="1"), "must be the integer")
        self.assert_invalid(make_bank(schemaVersion=True), "must be the integer")

    def test_invalid_bank_keys(self):
        self.assert_invalid(make_bank(key=""), "bank key must not be empty")
        self.assert_invalid(make_bank(key="my bank"), "must not contain whitespace")
        self.assert_invalid(make_bank(key="a/b"), "must not contain '/'")

    def test_questions_must_be_list(self):
        self.assert_invalid(make_bank(questions={"0": {}}), "questions must be a list")

    def test_unknown_bank_field(self):
        self.assert_invalid(make_bank(questoins=[]), "unknown field(s): questoins")

    def test_optional_field_types(self):
        self.assert_invalid(make_bank(subject=123), "subject must be a string")
        self.assert_invalid(make_bank(chapter="  "), "must not be empty or whitespace-only")
        self.assert_invalid(make_bank(tags="not-a-list"), "tags must be a list of strings")
        self.assert_invalid(make_bank(tags=["ok", ""]), "must not be empty")
        self.assert_invalid(make_bank(sourceMeta=[]), "sourceMeta must be an object")


class QuestionIdentityTest(SchemaTestCase):
    def test_id_only_is_valid(self):
        question = make_question(stableKey=None)
        question.pop("stableKey")
        question["id"] = "legacy-1"
        self.assert_valid(make_bank(questions=[question]))

    def test_id_and_stable_key_both_valid(self):
        self.assert_valid(make_bank(questions=[make_question(id="x-1")]))

    def test_missing_both_id_and_stable_key(self):
        question = make_question()
        question.pop("stableKey")
        self.assert_invalid(make_bank(questions=[question]),
                            "'id' or 'stableKey' (at least one)")

    def test_empty_id(self):
        self.assert_invalid(make_bank(questions=[make_question(id="  ")]),
                            "id must not be empty")

    def test_invalid_stable_key(self):
        self.assert_invalid(make_bank(questions=[make_question(stableKey="")]),
                            "stableKey must not be empty")
        self.assert_invalid(make_bank(questions=[make_question(stableKey="a b")]),
                            "must not contain whitespace")
        self.assert_invalid(make_bank(questions=[make_question(stableKey="a/b")]),
                            "must not contain '/'")

    def test_duplicate_stable_keys_rejected(self):
        bank = make_bank(questions=[
            make_question(stableKey="dup-1"),
            make_question(stableKey="dup-1", stem="Other?", options=["A", "B"], answer="B"),
        ])
        self.assert_invalid(bank, "duplicate stableKey")

    def test_unique_stable_keys_valid(self):
        bank = make_bank(questions=[
            make_question(stableKey="k-1"),
            make_question(stableKey="k-2", stem="Other?", options=["A", "B"], answer="B"),
        ])
        self.assert_valid(bank)


class QuestionTypeTest(SchemaTestCase):
    def test_short_answer_is_rejected(self):
        self.assert_invalid(make_bank(questions=[make_question(type="short_answer")]),
                            "'short_answer' is not allowed")

    def test_unknown_type_rejected(self):
        self.assert_invalid(make_bank(questions=[make_question(type="essay")]),
                            "unknown question type")

    def test_missing_type(self):
        question = make_question()
        question.pop("type")
        self.assert_invalid(make_bank(questions=[question]), "missing required field: type")

    def test_question_must_be_object(self):
        self.assert_invalid(make_bank(questions=["not an object"]), "must be an object")


class OptionsTest(SchemaTestCase):
    def test_two_and_five_options_valid(self):
        self.assert_valid(make_bank(questions=[
            make_question(options=["A one", "B two"], answer="A"),
            make_question(stableKey="q-2",
                         options=["A", "B", "C", "D", "E"], answer="E"),
        ]))

    def test_too_few_options(self):
        self.assert_invalid(make_bank(questions=[make_question(options=["Only one"])]),
                            "2-5 options (got 1)")

    def test_too_many_options(self):
        self.assert_invalid(make_bank(
            questions=[make_question(options=["A", "B", "C", "D", "E", "F"], answer="A")]),
            "2-5 options (got 6)")

    def test_missing_options_on_choice(self):
        question = make_question()
        question.pop("options")
        self.assert_invalid(make_bank(questions=[question]),
                            "missing required field: options")

    def test_options_forbidden_on_true_false(self):
        self.assert_invalid(make_bank(questions=[
            make_question(type="true_false", answer="true", options=["true", "false"])]),
            "options are only allowed for")

    def test_options_forbidden_on_fill_blank(self):
        self.assert_invalid(make_bank(questions=[
            make_question(type="fill_blank", answer="50", options=["A", "B"])]),
            "options are only allowed for")

    def test_blank_option_text(self):
        self.assert_invalid(make_bank(questions=[
            make_question(options=["A ok", "  ", "C ok"])]),
            "option must not be empty")

    def test_non_string_option(self):
        self.assert_invalid(make_bank(questions=[make_question(options=["A", 2, "C"])]),
                            "option must be a string")


class AnswerTest(SchemaTestCase):
    def test_single_choice_letter_in_range(self):
        self.assert_valid(make_bank(questions=[
            make_question(options=["A", "B", "C"], answer="C")]))

    def test_single_choice_letter_out_of_range(self):
        self.assert_invalid(make_bank(questions=[
            make_question(options=["A", "B"], answer="C")]),
            "do not exist")

    def test_single_choice_needs_exactly_one_letter(self):
        self.assert_invalid(make_bank(questions=[make_question(answer="AB")]),
                            "exactly one option letter")
        self.assert_invalid(make_bank(questions=[make_question(answer="")]),
                            "non-empty string of option letters")

    def test_choice_answer_must_be_uppercase(self):
        self.assert_invalid(make_bank(questions=[make_question(answer="a")]),
                            "must be uppercase A-Z")
        self.assert_invalid(make_bank(questions=[
            make_question(type="multi_choice", answer="aB")]),
            "must be uppercase A-Z")

    def test_multi_choice_answer(self):
        self.assert_valid(make_bank(questions=[
            make_question(stableKey="q-m1", type="multi_choice", answer="DBA"),
            make_question(stableKey="q-m2", type="multi_choice", answer="ABCD"),
        ]))

    def test_multi_choice_rejects_repeated_letters(self):
        self.assert_invalid(make_bank(questions=[
            make_question(stableKey="q-m", type="multi_choice", answer="ABA")]),
            "must not repeat option letters")

    def test_multi_choice_rejects_unknown_letters(self):
        self.assert_invalid(make_bank(questions=[
            make_question(stableKey="q-m", type="multi_choice", answer="ABE")]),
            "do not exist")

    def test_multi_choice_answer_must_be_string(self):
        self.assert_invalid(make_bank(questions=[
            make_question(stableKey="q-m", type="multi_choice", answer=["A", "B"])]),
            "non-empty string of option letters")

    def test_true_false_answer(self):
        self.assert_valid(make_bank(questions=[make_question(type="true_false", answer="false")]))
        self.assert_invalid(make_bank(questions=[make_question(type="true_false", answer="对")]),
                            'must be exactly "true" or "false"')
        self.assert_invalid(make_bank(questions=[make_question(type="true_false", answer="True")]),
                            'must be exactly "true" or "false"')

    def test_fill_blank_answer(self):
        self.assert_valid(make_bank(questions=[make_question(type="fill_blank", answer="50")]))
        self.assert_invalid(make_bank(questions=[make_question(type="fill_blank", answer=" ")]),
                            "answer must not be empty")
        self.assert_invalid(make_bank(questions=[make_question(type="fill_blank", answer="")]),
                            "answer must not be empty")

    def test_missing_answer(self):
        question = make_question()
        question.pop("answer")
        self.assert_invalid(make_bank(questions=[question]),
                            "missing required field: answer")


class QuestionTextFieldsTest(SchemaTestCase):
    def test_missing_stem_and_analysis(self):
        question = make_question()
        question.pop("stem")
        question.pop("analysis")
        self.assert_invalid(make_bank(questions=[question]),
                            "missing required field: stem",
                            "missing required field: analysis")

    def test_blank_stem_and_analysis(self):
        self.assert_invalid(make_bank(questions=[make_question(stem=" \t ")]),
                            "stem must not be empty or whitespace-only")

    def test_full_optional_question_fields_valid(self):
        self.assert_valid(make_bank(questions=[make_question(
            subject="电气安全",
            chapter="第一章",
            knowledgePoint="安全色",
            difficulty="medium",
            tags=["色标"],
            sourceMeta={"source": "GB 2893"},
        )]))

    def test_optional_question_field_types(self):
        self.assert_invalid(make_bank(questions=[make_question(difficulty="Easy")]),
                            "difficulty must be one of")
        self.assert_invalid(make_bank(questions=[make_question(knowledgePoint="")]),
                            "knowledgePoint must not be empty")
        self.assert_invalid(make_bank(questions=[make_question(tags=[1, 2])]),
                            "tag must be a string")
        self.assert_invalid(make_bank(questions=[make_question(sourceMeta="x")]),
                            "sourceMeta must be an object")

    def test_unknown_question_field(self):
        self.assert_invalid(make_bank(questions=[make_question(hint="why")]),
                            "unknown field(s): hint")


class BankIdentityTest(unittest.TestCase):
    def test_identity(self):
        self.assertEqual(schema.bank_identity(make_bank()), ("bank-1", 1))

    def test_identity_on_malformed(self):
        self.assertEqual(schema.bank_identity("junk"), (None, None))
        self.assertEqual(schema.bank_identity({"questions": "x"}), (None, None))


if __name__ == "__main__":
    unittest.main()
