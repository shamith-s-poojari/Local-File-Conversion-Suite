"""Worker protocol (spec section 7).

The models in ``_generated`` are written by ``pnpm generate`` from
``packages/protocol/schema/protocol.schema.json``. This module adds the
message unions, parsing and encoding on top of them.
"""

from __future__ import annotations

from typing import Annotated

from pydantic import Field, TypeAdapter

from ._generated import (
    Cancel,
    Estimate,
    EstimateResult,
    Execute,
    ExecuteOutput,
    ExecuteResult,
    Hello,
    Inspect,
    InspectResult,
    MediaInfo,
    PageText,
    PageTextResult,
    Progress,
    Render,
    RenderResult,
    Secrets,
    Stage,
    Validate,
    ValidateResult,
    ValidationIssue,
    WarningItem,
    WordBox,
    WorkerError,
    WorkerWarning,
)

__all__ = [
    "MAX_LINE_BYTES",
    "PROTOCOL_VERSION",
    "Cancel",
    "CoreMessage",
    "Estimate",
    "EstimateResult",
    "Execute",
    "ExecuteOutput",
    "ExecuteResult",
    "Hello",
    "Inspect",
    "InspectResult",
    "LineTooLongError",
    "MediaInfo",
    "PageText",
    "PageTextResult",
    "Progress",
    "Render",
    "RenderResult",
    "Secrets",
    "Stage",
    "Validate",
    "ValidateResult",
    "ValidationIssue",
    "WarningItem",
    "WordBox",
    "WorkerError",
    "WorkerMessage",
    "WorkerWarning",
    "encode",
    "parse_core_message",
    "parse_worker_message",
]

PROTOCOL_VERSION = 1
"""Must equal ``#/definitions/ProtocolVersion`` in the schema."""

MAX_LINE_BYTES = 1024 * 1024
"""Maximum length of one NDJSON line in bytes, excluding the newline (spec section 7.1)."""

# Must list exactly the members of #/definitions/CoreMessage and WorkerMessage;
# tests/test_protocol.py checks this against the schema.
CoreMessage = Annotated[
    Inspect | Validate | Estimate | Execute | Render | PageText | Cancel,
    Field(discriminator="type"),
]
WorkerMessage = Annotated[
    Hello
    | Stage
    | Progress
    | WorkerWarning
    | WorkerError
    | InspectResult
    | ValidateResult
    | EstimateResult
    | ExecuteResult
    | RenderResult
    | PageTextResult,
    Field(discriminator="type"),
]

_core_adapter: TypeAdapter[CoreMessage] = TypeAdapter(CoreMessage)
_worker_adapter: TypeAdapter[WorkerMessage] = TypeAdapter(WorkerMessage)


class LineTooLongError(ValueError):
    """A protocol line is longer than MAX_LINE_BYTES."""


def _check_length(line: bytes) -> bytes:
    line = line.removesuffix(b"\n")
    if len(line) > MAX_LINE_BYTES:
        raise LineTooLongError(f"line of {len(line)} bytes exceeds {MAX_LINE_BYTES}")
    return line


def parse_core_message(line: bytes) -> CoreMessage:
    """Parse one line the core sent to this worker.

    Raises ``pydantic.ValidationError`` or ``LineTooLongError``.
    Validation is strict: no type coercion, so ``"1"`` is not a page number.
    """
    return _core_adapter.validate_json(_check_length(line), strict=True)


def parse_worker_message(line: bytes) -> WorkerMessage:
    """Parse one line a worker sent. For tests and tools; the Rust core is the real consumer."""
    return _worker_adapter.validate_json(_check_length(line), strict=True)


def encode(message: WorkerMessage | CoreMessage) -> bytes:
    """Encode a message as one NDJSON line, including the trailing newline.

    Secrets are written as ``**********`` by Pydantic, so a worker can never
    echo a password back through this function.
    """
    line = message.model_dump_json(exclude_none=True).encode()
    if len(line) > MAX_LINE_BYTES:
        raise LineTooLongError(f"message of {len(line)} bytes exceeds {MAX_LINE_BYTES}")
    return line + b"\n"
