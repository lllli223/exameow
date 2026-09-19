"""Consumer-key derivation for the Exameow study feed/ack workflow.

The server tracks read cursors per exact consumer key, so `feed` and `ack`
must derive the same key from the same flags. Because filtered feeds get
their own keys, a filtered ack can never consume the unfiltered cursor
(and vice versa).
"""

from __future__ import annotations

DEFAULT_CONSUMER = "chatgpt"

_SUBJECT = "subject="
_CHAPTER = "chapter="
_BANK = "bank="
_FLAGGED = "flagged="


def clean_filter(value):
    """Trim and collapse whitespace while preserving meaningful casing."""
    if value is None:
        return ""
    if not isinstance(value, str):
        value = str(value)
    return " ".join(value.split())


def normalize_filter(value):
    """Canonical case-insensitive form used only inside consumer keys."""
    return clean_filter(value).casefold()


def _encode_segment(value):
    """Escape consumer-key delimiters without making CJK keys unreadable."""
    value = value.replace("%2f", "%2F")
    return value.replace("%", "%25").replace("/", "%2F")


def _decode_segment(value):
    """Inverse of _encode_segment for fully-qualified consumer replay."""
    return value.replace("%2F", "/").replace("%2f", "/").replace("%25", "%")


def is_fully_qualified(consumer):
    """True when the consumer string already carries subject=/chapter= segments.

    Fully-qualified consumers are used verbatim so callers can target an
    exact cursor namespace without re-derivation.
    """
    if not consumer:
        return False
    return any(
        segment.startswith((_SUBJECT, _CHAPTER, _BANK, _FLAGGED))
        for segment in consumer.split("/")
    )


def derive_consumer_key(
    consumer=None, subject=None, chapter=None, bank=None, include_flagged=True
):
    """Derive the exact cursor namespace from the base consumer and filters."""
    base = (consumer or "").strip() or DEFAULT_CONSUMER
    if is_fully_qualified(base):
        return base
    parts = [base]
    normalized_subject = normalize_filter(subject)
    normalized_chapter = normalize_filter(chapter)
    normalized_bank = clean_filter(bank)
    if normalized_subject:
        parts.append(_SUBJECT + _encode_segment(normalized_subject))
    if normalized_chapter:
        parts.append(_CHAPTER + _encode_segment(normalized_chapter))
    if normalized_bank:
        parts.append(_BANK + _encode_segment(normalized_bank))
    if not include_flagged:
        parts.append(_FLAGGED + "0")
    return "/".join(parts)


def derive_feed_params(
    consumer=None, subject=None, chapter=None, bank=None, include_flagged=True
):
    """Build feed query params and the matching ACK consumer namespace."""
    base = (consumer or "").strip() or DEFAULT_CONSUMER
    if is_fully_qualified(base):
        params = {"consumer": base}
        for segment in base.split("/"):
            if segment.startswith(_SUBJECT):
                params["subject"] = _decode_segment(segment[len(_SUBJECT):])
            elif segment.startswith(_CHAPTER):
                params["chapter"] = _decode_segment(segment[len(_CHAPTER):])
            elif segment.startswith(_BANK):
                params["bankKey"] = _decode_segment(segment[len(_BANK):])
            elif segment == _FLAGGED + "0":
                params["includeFlagged"] = "false"
        return params

    params = {
        "consumer": derive_consumer_key(
            base, subject, chapter, bank, include_flagged
        )
    }
    cleaned_subject = clean_filter(subject)
    cleaned_chapter = clean_filter(chapter)
    cleaned_bank = clean_filter(bank)
    if cleaned_subject:
        params["subject"] = cleaned_subject
    if cleaned_chapter:
        params["chapter"] = cleaned_chapter
    if cleaned_bank:
        params["bankKey"] = cleaned_bank
    if not include_flagged:
        params["includeFlagged"] = "false"
    return params
