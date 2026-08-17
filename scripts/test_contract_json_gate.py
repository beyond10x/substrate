#!/usr/bin/env python3
"""Negative tests proving the closed contract JSON gate fails safely."""

from __future__ import annotations

import base64
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))

from contract_json_gate import check_json_authority  # noqa: E402


def load_checker() -> object:
    path = SCRIPTS / "check-contract-bundle-0.2.0.py"
    specification = importlib.util.spec_from_file_location("substrate_bundle_checker", path)
    assert specification is not None and specification.loader is not None
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


CHECKER = load_checker()


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


class ContractJsonGateTests(unittest.TestCase):
    def run_gate(self, root: Path) -> list[str]:
        failures: list[str] = []
        documents = CHECKER.Documents(failures)
        check_json_authority(root, "0.2.0", documents, CHECKER.validate, failures)
        return failures

    def test_unclassified_json_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_json(root / "unexpected.json", {"value": "unclassified"})
            failures = self.run_gate(root)
            self.assertTrue(any("unclassified JSON authority" in item for item in failures))

    def test_every_fixed_authority_is_rejected_by_its_bundled_exact_schema(self) -> None:
        bundle = SCRIPTS.parent / "contracts" / "substrate-wire" / "0.2.0"
        cases = {
            "bundle.json": "name",
            "compatibility.json": "errata_from",
            "coverage.json": "requirements",
            "hashing.json": "canonical_query",
            "operations.json": "operations",
            "origins.json": "inputs",
            "packaging.json": "archive",
            "runner.json": "protocol",
        }
        for relative, missing in cases.items():
            with self.subTest(relative=relative), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                document = json.loads((bundle / relative).read_text(encoding="utf-8"))
                declaration = document["$schema"]
                document.pop(missing)
                write_json(root / relative, document)
                schema_target = (root / relative).parent / declaration
                source_schema = (bundle / relative).parent / declaration
                write_json(
                    schema_target,
                    json.loads(source_schema.read_text(encoding="utf-8")),
                )
                failures = self.run_gate(root)
                self.assertTrue(
                    any(f"missing required property '{missing}'" in item for item in failures),
                    failures,
                )

    def test_invalid_declared_payload_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_json(
                root / "schemas/example.json",
                {
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "additionalProperties": False,
                    "properties": {"value": {"type": "string"}},
                    "required": ["value"],
                    "type": "object",
                },
            )
            write_json(
                root / "payload.json",
                {"$schema": "schemas/example.json", "value": 7},
            )
            failures = self.run_gate(root)
            self.assertTrue(any("expected type string" in item for item in failures))

    def test_schema_invalid_under_declared_meta_schema_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_json(
                root / "schemas/invalid.json",
                {
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": 7,
                },
            )
            failures = self.run_gate(root)
            self.assertTrue(any("not valid under any of the schemas" in item for item in failures))

    def test_payload_schema_target_must_itself_be_a_schema_authority(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_json(root / "schemas/not-a-schema.json", {"value": "data"})
            write_json(
                root / "payload.json",
                {"$schema": "schemas/not-a-schema.json", "value": "test"},
            )
            failures = self.run_gate(root)
            self.assertTrue(
                any("declared target is not a Draft 2020-12 schema authority" in item for item in failures)
            )

    def test_file_base64_schema_enforces_decoded_one_mib_boundary(self) -> None:
        bundle = SCRIPTS.parent / "contracts" / "substrate-wire" / "0.2.0"
        common_path = bundle / "schemas" / "common.json"
        failures: list[str] = []
        documents = CHECKER.Documents(failures)
        common = documents.load(common_path)
        self.assertIsInstance(common, dict)
        assert isinstance(common, dict)
        definition = common["$defs"]["canonical-base64-file"]
        exact = base64.b64encode(bytes(1_048_576)).decode("ascii")
        oversized_same_encoded_length = base64.b64encode(bytes(1_048_577)).decode("ascii")
        self.assertEqual(len(exact), len(oversized_same_encoded_length))
        self.assertEqual(
            CHECKER.validate(exact, definition, common_path, documents),
            [],
        )
        self.assertTrue(
            CHECKER.validate(
                oversized_same_encoded_length,
                definition,
                common_path,
                documents,
            )
        )

    def test_event_stream_last_cursor_is_canonical_bounded_or_null(self) -> None:
        bundle = SCRIPTS.parent / "contracts" / "substrate-wire" / "0.2.0"
        schema_path = bundle / "schemas" / "event-stream-frame.json"
        failures: list[str] = []
        documents = CHECKER.Documents(failures)
        frame_schema = documents.load(schema_path)
        self.assertIsInstance(frame_schema, dict)
        assert isinstance(frame_schema, dict)
        base = {
            "code": "event.stream-backpressure",
            "kind": "backpressure",
            "recovery": "pull",
        }
        for cursor in (
            None,
            "ev2.scope_subject_01.41.0",
            "ev2.scope_subject_01.41.7",
        ):
            with self.subTest(cursor=cursor):
                self.assertEqual(
                    CHECKER.validate(
                        {**base, "last_cursor": cursor},
                        frame_schema,
                        schema_path,
                        documents,
                    ),
                    [],
                )
        for cursor in (
            "not-an-event-cursor",
            "ev2.scope_subject_01.41.01",
            f"ev2.scope_{'a' * 500}.41.7",
        ):
            with self.subTest(cursor=cursor[:32]):
                self.assertTrue(
                    CHECKER.validate(
                        {**base, "last_cursor": cursor},
                        frame_schema,
                        schema_path,
                        documents,
                    )
                )


if __name__ == "__main__":
    unittest.main()
