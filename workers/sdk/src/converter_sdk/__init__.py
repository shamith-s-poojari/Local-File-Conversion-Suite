"""Adapter SDK for converter workers (spec section 8).

An adapter subclasses ``Adapter``, overrides the methods it supports, and
ends with ``if __name__ == "__main__": run_main(MyAdapter)``. The runtime
speaks the worker protocol (``converter_sdk.protocol``) for it.
"""

from .adapter import (
    Adapter,
    InspectReport,
    PageWords,
    RenderedPage,
    ResourceEstimate,
    ValidationReport,
)
from .context import JobContext, OutputFile, safe_display_name
from .errors import AdapterError, Cancelled
from .runtime import Worker, run_main

__version__ = "0.1.0"

__all__ = [
    "Adapter",
    "AdapterError",
    "Cancelled",
    "InspectReport",
    "JobContext",
    "OutputFile",
    "PageWords",
    "RenderedPage",
    "ResourceEstimate",
    "ValidationReport",
    "Worker",
    "run_main",
    "safe_display_name",
]
