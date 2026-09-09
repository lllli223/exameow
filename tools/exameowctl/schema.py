"""Study-bank schema (schemaVersion 1) validation for Exameow.

Pure-stdlib, fully offline: `validate_bank()` takes an already-parsed JSON
object and returns a list of ``{"path": ..., "message": ...}`` error dicts.
An empty list means the bank is valid. See tools/exameowctl/README.md for
the full specification.
"""

from __future__ import annotations

SCHEMA_VERSION = 1

# SGCC workflow exam types. short_answer is deliberately NOT allowed.
QUESTION_TYPES = ("single_choice", "multi_choice", "true_false", "fill_blank")
CHOICE_TYPES = ("single_choice", "multi_choice")
DIFFICULTIES = ("easy", "medium", "hard")

BANK_FIELDS = (
    "schemaVersion",
    "key",
    "name",
    "questions",
    "subject",
    "chapter",
    "tags",
    "sourceMeta",
)
QUESTION_FIELDS = (
    "id",
    "stableKey",
    "type",
    "stem",
    "options",
    "answer",
    "analysis",
    "subject",
    "chapter",
    "knowledgePoint",
    "difficulty",
    "tags",
    "sourceMeta",
)

MAX_KEY_LENGTH = 128
TRUE_FALSE_ANSWERS = ("true", "false")


def _err(errors, path, message):
    errors.append({"path": path, "message": message})


def _is_nonempty_text(value):
    return isinstance(value, str) and value.strip() != ""


def _check_text(errors, path, value, label):
    """Free-text field: must be a non-empty, non-whitespace-only string."""
    if not isinstance(value, str):
        _err(errors, path, "%s must be a string" % label)
    elif not value.strip():
        _err(errors, path, "%s must not be empty or whitespace-only" % label)


def _check_identifier(errors, path, value, label):
    """Machine-facing key (bank key / stableKey): nonempty, no whitespace,
    no path separators, no control characters."""
    if not isinstance(value, str):
        _err(errors, path, "%s must be a string" % label)
        return
    if value == "":
        _err(errors, path, "%s must not be empty" % label)
        return
    if len(value) > MAX_KEY_LENGTH:
        _err(errors, path, "%s must be at most %d characters (got %d)"
             % (label, MAX_KEY_LENGTH, len(value)))
    if any(ch.isspace() for ch in value):
        _err(errors, path, "%s must not contain whitespace" % label)
    if "/" in value or "\\" in value:
        _err(errors, path, "%s must not contain '/' or '\\'" % label)
    if any(ord(ch) < 32 or ord(ch) == 127 for ch in value):
        _err(errors, path, "%s must not contain control characters" % label)


def _check_tags(errors, path, value):
    if not isinstance(value, list):
        _err(errors, path, "tags must be a list of strings")
        return
    for index, tag in enumerate(value):
        _check_text(errors, "%s[%d]" % (path, index), tag, "tag")


def _check_source_meta(errors, path, value):
    if not isinstance(value, dict):
        _err(errors, path, "sourceMeta must be an object")


def _check_unknown_fields(errors, path, obj, allowed):
    extra = sorted(str(key) for key in obj if key not in allowed)
    if extra:
        _err(errors, path, "unknown field(s): %s (allowed: %s)"
             % (", ".join(extra), ", ".join(allowed)))


def _check_options(errors, path, value):
    """Validate a choice question's options; returns the option count."""
    if not isinstance(value, list):
        _err(errors, path, "options must be a list of strings")
        return 0
    count = len(value)
    if not 2 <= count <= 5:
        _err(errors, path, "choice questions must have 2-5 options (got %d)" % count)
    for index, option in enumerate(value):
        _check_text(errors, "%s[%d]" % (path, index), option, "option")
    return count


def _allowed_letters(option_count):
    return "".join(chr(ord("A") + i) for i in range(option_count))


def _check_choice_answer(errors, path, answer, option_count, multi):
    if not isinstance(answer, str) or not answer:
        _err(errors, path,
             'answer must be a non-empty string of option letters (e.g. "A" or "ABD")')
        return
    letters = list(answer)
    if not all("A" <= ch <= "Z" for ch in letters):
        _err(errors, path,
             'answer letters must be uppercase A-Z option letters (e.g. "A" or "ABD"); '
             "lowercase or other characters are not accepted")
        return
    if len(set(letters)) != len(letters):
        _err(errors, path, "answer must not repeat option letters")
        return
    if not multi and len(letters) != 1:
        _err(errors, path,
             "single_choice answer must be exactly one option letter (got %r)" % answer)
        return
    # Only cross-check letters against options when the option count itself
    # is valid; otherwise the options error already covers the problem.
    if 2 <= option_count <= 5:
        allowed = _allowed_letters(option_count)
        invalid = sorted(set(ch for ch in letters if ch not in allowed))
        if invalid:
            _err(errors, path,
                 "answer references option letter(s) %r that do not exist (options are %s)"
                 % ("".join(invalid), allowed))


def _check_duplicate_stable_keys(errors, questions):
    seen = {}
    for index, question in enumerate(questions):
        if not isinstance(question, dict):
            continue
        stable_key = question.get("stableKey")
        if isinstance(stable_key, str) and stable_key:
            seen.setdefault(stable_key, []).append(index)
    for stable_key, indices in seen.items():
        if len(indices) > 1:
            _err(errors, "questions",
                 "duplicate stableKey %r in question(s) %s; stable keys must be "
                 "unique within one bank"
                 % (stable_key, ", ".join(str(i) for i in indices)))


def _validate_question(errors, index, question):
    path = "questions[%d]" % index
    if not isinstance(question, dict):
        _err(errors, path, "question must be an object")
        return
    _check_unknown_fields(errors, path, question, QUESTION_FIELDS)

    question_type = question.get("type")
    if "type" not in question:
        _err(errors, path, "missing required field: type")
    elif not isinstance(question_type, str) or question_type not in QUESTION_TYPES:
        allowed = ", ".join(QUESTION_TYPES)
        if question_type == "short_answer":
            _err(errors, path,
                 "question type 'short_answer' is not allowed in study banks "
                 "(allowed: %s)" % allowed)
        else:
            _err(errors, "%s.type" % path,
                 "unknown question type %r (allowed: %s)" % (question_type, allowed))

    for field in ("stem", "analysis"):
        if field not in question:
            _err(errors, path, "missing required field: %s" % field)
        else:
            _check_text(errors, "%s.%s" % (path, field), question[field], field)

    has_id = "id" in question
    has_stable_key = "stableKey" in question
    if not has_id and not has_stable_key:
        _err(errors, path, "missing required field: 'id' or 'stableKey' (at least one)")
    if has_id:
        _check_text(errors, "%s.id" % path, question["id"], "id")
    if has_stable_key:
        _check_identifier(errors, "%s.stableKey" % path, question["stableKey"], "stableKey")

    is_choice = question_type in CHOICE_TYPES
    option_count = 0
    if "options" in question:
        if not is_choice:
            _err(errors, "%s.options" % path,
                 "options are only allowed for %s questions" % "/".join(CHOICE_TYPES))
        option_count = _check_options(errors, "%s.options" % path, question["options"])
    elif is_choice:
        _err(errors, path,
             "missing required field: options (%s questions must have 2-5 options)"
             % question_type)

    if "answer" not in question:
        _err(errors, path, "missing required field: answer")
    elif question_type in CHOICE_TYPES:
        _check_choice_answer(
            errors, "%s.answer" % path, question["answer"], option_count,
            multi=(question_type == "multi_choice"))
    elif question_type == "true_false":
        if not isinstance(question["answer"], str) or question["answer"] not in TRUE_FALSE_ANSWERS:
            _err(errors, "%s.answer" % path,
                 'true_false answer must be exactly "true" or "false"')
    elif question_type == "fill_blank":
        _check_text(errors, "%s.answer" % path, question["answer"], "answer")
    else:
        # Unknown question type; still require a non-empty answer.
        if not _is_nonempty_text(question["answer"]):
            _err(errors, "%s.answer" % path, "answer must be a non-empty string")

    for field in ("subject", "chapter", "knowledgePoint"):
        if field in question:
            _check_text(errors, "%s.%s" % (path, field), question[field], field)

    if "difficulty" in question:
        difficulty = question["difficulty"]
        if not isinstance(difficulty, str) or difficulty not in DIFFICULTIES:
            _err(errors, "%s.difficulty" % path,
                 "difficulty must be one of: %s" % ", ".join(DIFFICULTIES))

    if "tags" in question:
        _check_tags(errors, "%s.tags" % path, question["tags"])
    if "sourceMeta" in question:
        _check_source_meta(errors, "%s.sourceMeta" % path, question["sourceMeta"])


def validate_bank(data):
    """Validate a parsed study-bank object; returns a list of error dicts."""
    errors = []
    if not isinstance(data, dict):
        _err(errors, "(bank)", "bank must be a JSON object")
        return errors

    _check_unknown_fields(errors, "(bank)", data, BANK_FIELDS)

    if "schemaVersion" not in data:
        _err(errors, "schemaVersion",
             "missing required field: schemaVersion (must be %d)" % SCHEMA_VERSION)
    else:
        version = data["schemaVersion"]
        if isinstance(version, bool) or not isinstance(version, int):
            _err(errors, "schemaVersion", "schemaVersion must be the integer %d" % SCHEMA_VERSION)
        elif version != SCHEMA_VERSION:
            _err(errors, "schemaVersion",
                 "unsupported schemaVersion %d (this tool supports version %d only)"
                 % (version, SCHEMA_VERSION))

    if "key" not in data:
        _err(errors, "(bank)", "missing required field: key")
    else:
        _check_identifier(errors, "key", data["key"], "bank key")

    if "name" not in data:
        _err(errors, "(bank)", "missing required field: name")
    else:
        _check_text(errors, "name", data["name"], "name")

    if "questions" not in data:
        _err(errors, "(bank)", "missing required field: questions")
    elif not isinstance(data["questions"], list):
        _err(errors, "questions", "questions must be a list")
    else:
        for index, question in enumerate(data["questions"]):
            _validate_question(errors, index, question)
        _check_duplicate_stable_keys(errors, data["questions"])

    if "subject" in data:
        _check_text(errors, "subject", data["subject"], "subject")
    if "chapter" in data:
        _check_text(errors, "chapter", data["chapter"], "chapter")
    if "tags" in data:
        _check_tags(errors, "tags", data["tags"])
    if "sourceMeta" in data:
        _check_source_meta(errors, "sourceMeta", data["sourceMeta"])
    return errors


def bank_identity(data):
    """Best-effort (key, question_count) summary, used in JSON output."""
    if not isinstance(data, dict):
        return (None, None)
    key = data.get("key")
    questions = data.get("questions")
    return (
        key if isinstance(key, str) and key else None,
        len(questions) if isinstance(questions, list) else None,
    )
