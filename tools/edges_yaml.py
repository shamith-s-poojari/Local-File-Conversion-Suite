"""Print a conversion adapter's EDGES as manifest YAML (`edges:` block).

    uv run python tools/edges_yaml.py adapters/image-convert

Each adapter's tests check the manifest matches its table; this keeps them
in step when the table changes.
"""

from __future__ import annotations

import sys
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "workers" / "conformance" / "src"))
from converter_conformance.harness import load_adapter_class  # noqa: E402


def edges_yaml(folder: Path) -> str:
    cls = load_adapter_class(folder)
    groups: dict[tuple[str, str], list[str]] = defaultdict(list)
    for (frm, to), tier in cls.EDGES.items():
        groups[(to, tier)].append(frm)
    lines = ["edges:"]
    for (to, tier), froms in sorted(groups.items()):
        lines.append(f"  - {{ from: [{', '.join(froms)}], to: {to}, tier: {tier} }}")
    return "\n".join(lines)


if __name__ == "__main__":
    print(edges_yaml(Path(sys.argv[1])))
