# traceqa-protocol

TraceQA message and file types for Python, generated from the JSON Schemas in
[spec/](https://github.com/thearpankumar/TraceQA/tree/master/spec) by
`scripts/gen_python_protocol.py`. Both Python drivers depend
on it. Models take snake_case names, validate camelCase wire names and serialise
to camelCase.

This package is built from the repository and is not published to PyPI yet. Do
not edit `models.py` or `contracts.py` by hand; change the schemas and
regenerate.

Licensed under Apache-2.0.
