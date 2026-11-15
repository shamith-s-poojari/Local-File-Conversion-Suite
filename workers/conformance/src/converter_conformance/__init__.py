"""Adapter conformance kit (spec section 8.4) and the generated test corpus.

``tests/test_conformance.py`` runs every check against every adapter that
ships, using the cases in ``adapters/<id>/tests/conformance.yaml``. Adapter
unit tests import ``corpus`` and ``harness`` from here.
"""

from . import corpus, harness

__all__ = ["corpus", "harness"]
