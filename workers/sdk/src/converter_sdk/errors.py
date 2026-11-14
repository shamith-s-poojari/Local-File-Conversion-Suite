"""Error codes from the catalog (spec section 13.1) as exceptions."""

from __future__ import annotations

from typing import Any, Literal

ErrorCode = Literal[
    "E_INPUT_CORRUPT",
    "E_PASSWORD_REQUIRED",
    "E_PASSWORD_WRONG",
    "E_LIMIT_EXCEEDED",
    "E_RESOURCE_MEMORY",
    "E_RESOURCE_DISK",
    "E_TIMEOUT",
    "E_CANCELLED",
    "E_PROTOCOL",
    "E_OUTPUT_INVALID",
    "E_SANDBOX_UNAVAILABLE",
    "E_INSTALL_CORRUPT",
    "E_REDACTION_VERIFICATION_FAILED",
    "E_NO_TEXT",
    "E_INTERNAL",
]

RETRYABLE: frozenset[str] = frozenset(
    {
        "E_PASSWORD_REQUIRED",
        "E_PASSWORD_WRONG",
        "E_RESOURCE_MEMORY",
        "E_RESOURCE_DISK",
        "E_TIMEOUT",
        "E_INTERNAL",
    }
)
"""Codes the catalog marks retryable. ``E_CANCELLED`` is "n/a" and sent as not retryable."""


class AdapterError(Exception):
    """Raise from an adapter method to fail the request with a catalog code.

    ``detail`` is sent to the core and may be shown to the user, so it must
    never contain document content, file names or secrets: only counts,
    page numbers, parameter names and similar facts.
    """

    def __init__(
        self,
        code: ErrorCode,
        detail: dict[str, Any] | None = None,
        *,
        retryable: bool | None = None,
    ) -> None:
        super().__init__(code)
        self.code: ErrorCode = code
        self.detail = detail
        self.retryable = code in RETRYABLE if retryable is None else retryable


class Cancelled(AdapterError):
    """Raised by ``ctx.check_cancelled()`` once the core has sent ``cancel``."""

    def __init__(self) -> None:
        super().__init__("E_CANCELLED", retryable=False)
