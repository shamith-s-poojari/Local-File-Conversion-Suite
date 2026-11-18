"""What kind of link a URL is.

Shared by the active-content detectors, the PDF disarm verifier and the
metadata cleaners, so a link one of them calls "plain web" is plain web to all.
Nothing here opens or resolves a URL.
"""

from __future__ import annotations

import re
from urllib.parse import urlsplit

WEB_SCHEMES = frozenset({"http", "https", "mailto", "tel"})
CONTROL = re.compile(r"[\x00-\x20\x7f]")
"""Characters browsers drop from a URL before reading its scheme."""


def scheme(url: str) -> str:
    """Lower-case scheme of ``url`` after removing the control characters
    browsers ignore. ``""`` for relative references, ``"//"`` for network
    paths (``//host`` and ``\\\\host``), ``"invalid"`` when it can't be split."""
    u = CONTROL.sub("", url)
    if u.startswith(("//", "\\\\", "\\/", "/\\")):
        return "//"
    try:
        return urlsplit(u).scheme.lower()
    except ValueError:
        return "invalid"


def is_web_url(url: str) -> bool:
    """Whether ``url`` is a plain web or mail link (``http``, ``https``,
    ``mailto``, ``tel``), the kind of link that is a link and nothing more."""
    return scheme(url) in WEB_SCHEMES
