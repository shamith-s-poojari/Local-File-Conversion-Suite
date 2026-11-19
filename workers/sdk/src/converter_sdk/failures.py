"""Turn an engine's exception into the catalog error it means.

A job that hits a hostile or damaged input must fail for good (``E_LIMIT_EXCEEDED``,
``E_INPUT_CORRUPT``), not as ``E_INTERNAL``, which the core retries and which
tells the user that the app is at fault. Engines are recognised by the module
of the exception class, so this file imports none of them. Messages are only
read to tell a limit from damage and are never copied anywhere: they can quote
document content.
"""

from __future__ import annotations

from .errors import AdapterError

_ENGINE_MODULES = frozenset({"pikepdf", "pymupdf", "fitz", "mupdf", "pypdfium2"})
_LIMIT_WORDS = ("too deep", "stack overflow", "nesting", "recursion", "overflow")


def _engine(e: BaseException) -> bool:
    return any(cls.__module__.split(".")[0] in _ENGINE_MODULES for cls in type(e).__mro__)


def _is_limit(e: BaseException) -> bool:
    if any(cls.__name__ == "FzErrorLimit" for cls in type(e).__mro__):
        return True
    message = str(e).lower()
    return any(word in message for word in _LIMIT_WORDS)


def map_exception(e: BaseException) -> AdapterError | None:
    """The error for ``e``, or ``None`` when it isn't one this module knows."""
    if isinstance(e, AdapterError):  # includes Cancelled: already a catalog error
        return e
    if isinstance(e, RecursionError):
        # Objects nested deeper than the interpreter or an engine can walk.
        return AdapterError("E_LIMIT_EXCEEDED", {"reason": "nesting"})
    if type(e).__name__ == "PasswordError" and _engine(e):
        return None  # a password problem, not damage; the caller decides
    if _engine(e):
        if _is_limit(e):
            return AdapterError("E_LIMIT_EXCEEDED", {"reason": "nesting"})
        return AdapterError("E_INPUT_CORRUPT", {"reason": "damaged"})
    return None
