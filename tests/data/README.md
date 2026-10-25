# tests/data

`sarif-schema-2.1.0.json` is the OASIS SARIF 2.1.0 JSON Schema (draft-04), unmodified, from
https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json
(the errata 01 text: https://docs.oasis-open.org/sarif/sarif/v2.1.0/errata01/os/schemas/sarif-schema-2.1.0.json).
It is published under the OASIS IPR policy. `tests/test_baseline.py` validates `--format sarif` output against it.
