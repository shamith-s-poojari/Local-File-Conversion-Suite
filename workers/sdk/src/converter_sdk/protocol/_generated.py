# Generated from packages/protocol/schema/protocol.schema.json by packages/protocol/scripts/generate.mjs.
# Do not edit by hand. Run `pnpm generate` instead.

from __future__ import annotations

from typing import Annotated, Any, Literal

from pydantic import BaseModel, ConfigDict, Field, RootModel, SecretStr


class JobPath(RootModel[str]):
    root: Annotated[
        str,
        Field(
            description="A path relative to the job directory, using forward slashes. No absolute paths, drive letters, backslashes, empty segments, or segments starting with a dot (so no '.' or '..'). The core still resolves and checks every path on disk.",
            pattern="^[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}(/[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}){0,7}$",
        ),
    ]


class Params(BaseModel):
    model_config = ConfigDict(
        extra="allow",
    )


class Detail(RootModel[dict[str, Any]]):
    root: Annotated[dict[str, Any], Field(max_length=32)]


class Secrets(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    password: Annotated[
        SecretStr | None,
        Field(
            description="Password to open an encrypted input.",
            max_length=1024,
            min_length=1,
        ),
    ] = None
    userPassword: Annotated[
        SecretStr | None,
        Field(
            description="Open password to set on a protected output (pdf-protect).",
            max_length=1024,
            min_length=1,
        ),
    ] = None
    ownerPassword: Annotated[
        SecretStr | None,
        Field(
            description="Owner password to set on a protected output (pdf-protect).",
            max_length=1024,
            min_length=1,
        ),
    ] = None


class WarningCode(
    RootModel[
        Literal[
            "FORMS_FLATTENED",
            "XFA_UNSUPPORTED",
            "SIGNATURE_INVALIDATED",
            "TAGS_DROPPED",
            "OUTLINE_DROPPED",
            "ACTIVE_CONTENT_FOUND",
            "NO_TEXT_LAYER",
            "REDACTED_TEXT_REMAINS_ELSEWHERE",
            "GLYPH_UNSUPPORTED",
            "W_LARGE_JOB",
            "TRANSPARENCY_FLATTENED",
            "ANIMATION_FLATTENED",
            "IMAGE_DOWNSCALED",
            "COLORS_REDUCED",
            "METADATA_DROPPED",
            "AUDIO_RESAMPLED",
            "AUDIO_DOWNMIXED",
            "STREAMS_DROPPED",
            "LAYOUT_APPROXIMATED",
            "FORMULAS_DROPPED",
            "SHEETS_SPLIT",
            "ONLY_FIRST_SHEET",
            "IMAGES_DROPPED",
            "EMBEDDED_FONTS_MISSING",
            "NESTED_DATA_FLATTENED",
            "COMMENTS_DROPPED",
            "REENCODED",
            "KEPT_ORIGINAL",
            "TARGET_NOT_REACHED",
            "COLUMNS_MISMATCHED",
        ]
    ]
):
    root: Annotated[
        Literal[
            "FORMS_FLATTENED",
            "XFA_UNSUPPORTED",
            "SIGNATURE_INVALIDATED",
            "TAGS_DROPPED",
            "OUTLINE_DROPPED",
            "ACTIVE_CONTENT_FOUND",
            "NO_TEXT_LAYER",
            "REDACTED_TEXT_REMAINS_ELSEWHERE",
            "GLYPH_UNSUPPORTED",
            "W_LARGE_JOB",
            "TRANSPARENCY_FLATTENED",
            "ANIMATION_FLATTENED",
            "IMAGE_DOWNSCALED",
            "COLORS_REDUCED",
            "METADATA_DROPPED",
            "AUDIO_RESAMPLED",
            "AUDIO_DOWNMIXED",
            "STREAMS_DROPPED",
            "LAYOUT_APPROXIMATED",
            "FORMULAS_DROPPED",
            "SHEETS_SPLIT",
            "ONLY_FIRST_SHEET",
            "IMAGES_DROPPED",
            "EMBEDDED_FONTS_MISSING",
            "NESTED_DATA_FLATTENED",
            "COMMENTS_DROPPED",
            "REENCODED",
            "KEPT_ORIGINAL",
            "TARGET_NOT_REACHED",
            "COLUMNS_MISMATCHED",
        ],
        Field(
            description="Warning codes in the catalog (spec section 13.1). Workers may send other codes; the core shows those as a generic warning, so the wire format accepts any WarningCodeString."
        ),
    ]


class WarningItem(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    code: Annotated[
        str,
        Field(
            description="A warning code as sent on the wire. Unknown codes are allowed here and handled by the core.",
            pattern="^[A-Z][A-Z0-9_]{0,63}$",
        ),
    ]
    detail: Detail | None = None


class Inspect(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["inspect"] = "inspect"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    input: Annotated[
        str,
        Field(
            description="A path relative to the job directory, using forward slashes. No absolute paths, drive letters, backslashes, empty segments, or segments starting with a dot (so no '.' or '..'). The core still resolves and checks every path on disk.",
            pattern="^[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}(/[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}){0,7}$",
        ),
    ]
    secrets: Secrets | None = None


class Validate(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["validate"] = "validate"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    inputs: Annotated[list[JobPath], Field(max_length=1000, min_length=1)]
    params: Params
    secrets: Secrets | None = None


class Estimate(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["estimate"] = "estimate"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    inputs: Annotated[list[JobPath], Field(max_length=1000, min_length=1)]
    params: Params
    metadata: Detail


class Execute(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["execute"] = "execute"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    inputs: Annotated[list[JobPath], Field(max_length=1000, min_length=1)]
    outDir: Literal["out"] = "out"
    workDir: Literal["work"] = "work"
    params: Params
    secrets: Secrets | None = None


class Render(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["render"] = "render"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    input: Annotated[
        str,
        Field(
            description="A path relative to the job directory, using forward slashes. No absolute paths, drive letters, backslashes, empty segments, or segments starting with a dot (so no '.' or '..'). The core still resolves and checks every path on disk.",
            pattern="^[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}(/[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}){0,7}$",
        ),
    ]
    page: Annotated[int, Field(description="1-based page number.", ge=1, le=100000)]
    dpi: Annotated[int, Field(ge=18, le=600)]
    out: Annotated[
        str,
        Field(
            description="A JobPath that must be inside out/.",
            pattern="^out(/[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}){1,7}$",
        ),
    ]
    secrets: Secrets | None = None


class PageText(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["pageText"] = "pageText"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    input: Annotated[
        str,
        Field(
            description="A path relative to the job directory, using forward slashes. No absolute paths, drive letters, backslashes, empty segments, or segments starting with a dot (so no '.' or '..'). The core still resolves and checks every path on disk.",
            pattern="^[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}(/[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}){0,7}$",
        ),
    ]
    page: Annotated[int, Field(description="1-based page number.", ge=1, le=100000)]
    secrets: Secrets | None = None


class Cancel(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["cancel"] = "cancel"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]


class Hello(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[int, Field(ge=1, le=65535)]
    type: Literal["hello"] = "hello"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    adapterId: Annotated[str, Field(pattern="^[a-z0-9][a-z0-9-]{0,63}$")]
    protocolVersion: Annotated[int, Field(ge=1, le=65535)]
    pid: Annotated[int, Field(ge=1, le=4294967295)]


class Stage(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["stage"] = "stage"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    name: Annotated[str, Field(pattern="^[A-Za-z][A-Za-z0-9_-]{0,31}$")]
    index: Annotated[int, Field(ge=1, le=1000)]
    of: Annotated[int, Field(ge=1, le=1000)]


class Progress(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["progress"] = "progress"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    stage: Annotated[str, Field(pattern="^[A-Za-z][A-Za-z0-9_-]{0,31}$")]
    done: Annotated[int, Field(ge=0, le=9007199254740991)]
    total: Annotated[int | None, Field(ge=0, le=9007199254740991)] = None


class WorkerWarning(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["warning"] = "warning"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    code: Annotated[
        str,
        Field(
            description="A warning code as sent on the wire. Unknown codes are allowed here and handled by the core.",
            pattern="^[A-Z][A-Z0-9_]{0,63}$",
        ),
    ]
    detail: Detail | None = None


class WorkerError(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["error"] = "error"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    code: Annotated[
        Literal[
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
        ],
        Field(
            description="Error codes in the catalog (spec section 13.1). A worker sending any other code fails the schema, which the core treats as E_PROTOCOL."
        ),
    ]
    detail: Detail | None = None
    retryable: bool


class ValidationIssue(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    code: Annotated[
        Literal[
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
        ],
        Field(
            description="Error codes in the catalog (spec section 13.1). A worker sending any other code fails the schema, which the core treats as E_PROTOCOL."
        ),
    ]
    input: Annotated[
        str | None,
        Field(
            description="A path relative to the job directory, using forward slashes. No absolute paths, drive letters, backslashes, empty segments, or segments starting with a dot (so no '.' or '..'). The core still resolves and checks every path on disk.",
            pattern="^[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}(/[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}){0,7}$",
        ),
    ] = None
    param: Annotated[str | None, Field(max_length=128)] = None
    detail: Detail | None = None


class ValidateResult(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["validateResult"] = "validateResult"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    errors: Annotated[list[ValidationIssue], Field(max_length=256)]
    warnings: Annotated[list[WarningItem], Field(max_length=256)]
    metadata: Detail


class EstimateResult(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["estimateResult"] = "estimateResult"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    ramBytes: Annotated[int, Field(ge=0, le=9007199254740991)]
    diskBytes: Annotated[int, Field(ge=0, le=9007199254740991)]
    costUnits: Annotated[int, Field(ge=0, le=9007199254740991)]


class ExecuteOutput(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    path: Annotated[
        str,
        Field(
            description="A JobPath that must be inside out/.",
            pattern="^out(/[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}){1,7}$",
        ),
    ]
    suggestedName: Annotated[
        str,
        Field(
            description="Untrusted. The core sanitizes it before use.",
            max_length=255,
            min_length=1,
        ),
    ]
    pages: Annotated[int | None, Field(ge=0, le=100000)] = None
    sizeBytes: Annotated[int, Field(ge=0, le=9007199254740991)]
    sha256: Annotated[
        str,
        Field(description="Lowercase hex SHA-256 digest.", pattern="^[0-9a-f]{64}$"),
    ]


class ExecuteResult(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["executeResult"] = "executeResult"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    outputs: Annotated[list[ExecuteOutput], Field(max_length=10000, min_length=1)]
    fidelity: Annotated[
        Literal["E", "H", "R", "L"],
        Field(
            description="Fidelity tier (blueprint section 3.1): Exact, High, Reconstructed, Lossy."
        ),
    ]


class RenderResult(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["renderResult"] = "renderResult"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    out: Annotated[
        str,
        Field(
            description="A JobPath that must be inside out/.",
            pattern="^out(/[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}){1,7}$",
        ),
    ]
    widthPx: Annotated[int, Field(ge=1, le=20000)]
    heightPx: Annotated[int, Field(ge=1, le=20000)]
    sizeBytes: Annotated[int, Field(ge=0, le=9007199254740991)]
    sha256: Annotated[
        str,
        Field(description="Lowercase hex SHA-256 digest.", pattern="^[0-9a-f]{64}$"),
    ]


class WordBox(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    text: Annotated[str, Field(max_length=512, min_length=1)]
    x0: float
    y0: float
    x1: float
    y1: float


class PageTextResult(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["pageTextResult"] = "pageTextResult"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    page: Annotated[int, Field(description="1-based page number.", ge=1, le=100000)]
    width: Annotated[float, Field(gt=0.0, le=200000.0)]
    height: Annotated[float, Field(gt=0.0, le=200000.0)]
    words: Annotated[list[WordBox], Field(max_length=20000)]


class MediaInfo(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    kind: Literal["image", "audio", "video", "document"]
    durationMs: Annotated[int | None, Field(ge=0)] = None
    width: Annotated[int | None, Field(ge=0, le=1000000)] = None
    height: Annotated[int | None, Field(ge=0, le=1000000)] = None
    frames: Annotated[int | None, Field(ge=0)] = None
    videoCodec: Annotated[str | None, Field(max_length=64)] = None
    audioCodec: Annotated[str | None, Field(max_length=64)] = None
    sampleRate: Annotated[int | None, Field(ge=0)] = None
    channels: Annotated[int | None, Field(ge=0, le=64)] = None
    bitrateKbps: Annotated[int | None, Field(ge=0)] = None
    fps: Annotated[float | None, Field(ge=0.0)] = None
    hasAudio: bool | None = None
    hasVideo: bool | None = None


class InspectResult(BaseModel):
    model_config = ConfigDict(
        extra="forbid",
    )
    v: Annotated[
        Literal[1],
        Field(
            description="The protocol version this schema describes. App and workers ship together, so they must match exactly (spec section 7.5)."
        ),
    ] = 1
    type: Literal["inspectResult"] = "inspectResult"
    jobId: Annotated[
        str,
        Field(
            description="Opaque job identifier chosen by the core.",
            pattern="^[A-Za-z0-9_-]{1,64}$",
        ),
    ]
    pages: Annotated[int, Field(ge=0, le=100000)]
    encryption: Annotated[
        Literal["none", "permissionsOnly", "openPassword"],
        Field(
            description="Authoritative encryption state from inspect: none, restrictions only (opens without a password), or an open password."
        ),
    ]
    signed: bool | None = None
    warnings: Annotated[list[WarningItem] | None, Field(max_length=64)] = None
    media: MediaInfo | None = None


class Message(
    RootModel[
        Inspect
        | Validate
        | Estimate
        | Execute
        | Render
        | PageText
        | Cancel
        | Hello
        | Stage
        | Progress
        | WorkerWarning
        | WorkerError
        | InspectResult
        | ValidateResult
        | EstimateResult
        | ExecuteResult
        | RenderResult
        | PageTextResult
    ]
):
    root: Annotated[
        Inspect
        | Validate
        | Estimate
        | Execute
        | Render
        | PageText
        | Cancel
        | Hello
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
        Field(
            description="One line of the worker protocol (spec section 7). Every line on the wire is exactly one of these messages, encoded as UTF-8 JSON, at most 1 MiB.",
            title="Message",
        ),
    ]
