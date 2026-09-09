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


def normalize_filter(value):
    """Canonical form for filter values.

    Trim, collapse ALL whitespace (including full-width spaces) to single
    ASCII spaces, and casefold, so the same logical filter always maps to
    the same key regardless of quoting or casing.
    """
    if value is None:
        return ""
    if not isinstance(value, str):
        value = str(value)
    return " ".join(value.split()).casefold()


def is_fully_qualified(consumer):
    """True when the consumer string already carries subject=/chapter= segments.

    Fully-qualified consumers are used verbatim so callers can target an
    exact cursor namespace without re-derivation.
    """
    if not consumer:
        return False
    return any(
        segment.startswith(_SUBJECT) or segment.startswith(_CHAPTER)
        for segment in consumer.split("/")
    )


def derive_consumer_key(consumer=None, subject=None, chapter=None):
    """Derive the exact consumer key from a base consumer plus filters.

    Layout (filter order is canonical, independent of flag order):

        <base>                                     no filters
        <base>/subject=<normalized subject>        --subject
        <base>/chapter=<normalized chapter>       --chapter
        <base>/subject=<s>/chapter=<c>             both

    A fully-qualified consumer (already containing subject=/chapter=
    segments) is returned verbatim and filters are ignored.
    """
    base = (consumer or "").strip() or DEFAULT_CONSUMER
    if is_fully_qualified(base):
        return base
    parts = [base]
    normalized_subject = normalize_filter(subject)
    normalized_chapter = normalize_filter(chapter)
    if normalized_subject:
        parts.append(_SUBJECT + normalized_subject)
    if normalized_chapter:
        parts.append(_CHAPTER + normalized_chapter)
    return "/".join(parts)


def derive_feed_params(consumer=None, subject=None, chapter=None):
    """Build the request params shared by `feed` (query) and `ack` (body).

    Returns a dict with 'consumer' (the exact derived key) plus the
    normalized 'subject'/'chapter' when they were derived from flags.
    Fully-qualified consumers already encode their filters, so no extra
    filter params are sent in that case.
    """
    base = (consumer or "").strip() or DEFAULT_CONSUMER
    params = {"consumer": derive_consumer_key(base, subject, chapter)}
    if not is_fully_qualified(base):
        normalized_subject = normalize_filter(subject)
        normalized_chapter = normalize_filter(chapter)
        if normalized_subject:
            params["subject"] = normalized_subject
        if normalized_chapter:
            params["chapter"] = normalized_chapter
    return params
