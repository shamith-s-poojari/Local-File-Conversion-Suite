import json
import typing
from pathlib import Path

import pytest
from converter_sdk import protocol
from pydantic import ValidationError

ROOT = Path(__file__).resolve().parents[3]
SCHEMA = json.loads((ROOT / "packages/protocol/schema/protocol.schema.json").read_text("utf-8"))
CASES = json.loads((ROOT / "packages/protocol/fixtures/cases.json").read_text("utf-8"))

PARSERS = {
    "core": protocol.parse_core_message,
    "worker": protocol.parse_worker_message,
}

# Schema rules the Pydantic generator can't express. The Rust core enforces
# them on everything it receives; these are core-to-worker messages, so the
# worker side only loses a redundant check.
KNOWN_GAPS = {
    "empty secrets": "minProperties on Secrets",
    "missing v": "v defaults to PROTOCOL_VERSION so adapters can build messages without it",
}


def line(message: dict) -> bytes:
    return json.dumps(message, ensure_ascii=False).encode() + b"\n"


def member_types(definition: str) -> set[str]:
    names = [ref["$ref"].rsplit("/", 1)[1] for ref in SCHEMA["definitions"][definition]["oneOf"]]
    return {SCHEMA["definitions"][name]["properties"]["type"]["const"] for name in names}


def union_types(union: object) -> set[str]:
    members = typing.get_args(typing.get_args(union)[0])
    return {m.model_fields["type"].default for m in members}


@pytest.mark.parametrize("case", CASES["valid"], ids=lambda c: c["name"])
def test_accepts_valid_messages(case: dict) -> None:
    parsed = PARSERS[case["direction"]](line(case["message"]))
    assert parsed.type == case["message"]["type"]


@pytest.mark.parametrize("case", CASES["invalid"], ids=lambda c: c["name"])
def test_rejects_invalid_messages(case: dict) -> None:
    if case["name"] in KNOWN_GAPS:
        pytest.skip(KNOWN_GAPS[case["name"]])
    with pytest.raises(ValidationError):
        PARSERS[case["direction"]](line(case["message"]))


def test_unions_match_schema() -> None:
    assert union_types(protocol.CoreMessage) == member_types("CoreMessage")
    assert union_types(protocol.WorkerMessage) == member_types("WorkerMessage")


def test_constants_match_schema() -> None:
    assert SCHEMA["definitions"]["ProtocolVersion"]["const"] == protocol.PROTOCOL_VERSION
    assert protocol.MAX_LINE_BYTES == 1_048_576


def test_rejects_long_lines() -> None:
    padding = "a" * protocol.MAX_LINE_BYTES
    message = {"v": 1, "type": "cancel", "jobId": "j", "pad": padding}
    with pytest.raises(protocol.LineTooLongError):
        protocol.parse_core_message(line(message))


def test_round_trip_worker_messages() -> None:
    for case in CASES["valid"]:
        if case["direction"] != "worker":
            continue
        parsed = protocol.parse_worker_message(line(case["message"]))
        assert json.loads(protocol.encode(parsed)) == case["message"]


def test_messages_have_defaults_for_constants() -> None:
    stage = protocol.Stage(jobId="j_1", name="merge", index=1, of=2)
    assert json.loads(protocol.encode(stage)) == {
        "v": 1,
        "type": "stage",
        "jobId": "j_1",
        "name": "merge",
        "index": 1,
        "of": 2,
    }


def test_secrets_are_hidden() -> None:
    sentinel = "sentinel-PASSWORD-1"
    message = {"v": 1, "type": "inspect", "jobId": "j", "input": "in/0.pdf"}
    parsed = protocol.parse_core_message(line({**message, "secrets": {"password": sentinel}}))
    assert parsed.secrets.password.get_secret_value() == sentinel
    assert sentinel not in repr(parsed)
    assert sentinel not in str(parsed)
    assert sentinel.encode() not in protocol.encode(parsed)


def test_errors_do_not_echo_secrets() -> None:
    sentinel = "sentinel-PASSWORD-1"
    message = {
        "v": 1,
        "type": "inspect",
        "jobId": "j",
        "input": "../escape",
        "secrets": {"password": sentinel},
    }
    with pytest.raises(ValidationError) as error:
        protocol.parse_core_message(line(message))
    assert sentinel not in str(error.value)
