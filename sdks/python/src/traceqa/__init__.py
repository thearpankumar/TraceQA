"""Python client for TraceQA.

    import traceqa

    result = traceqa.extract("https://example.com", browsers=["chromium", "firefox"])
    for target in result.targets:
        for component in result.components(target.dir).components:
            print(component.id, component.role, component.name, len(component.instances))

This package holds no logic. It starts the `traceqa` engine as a child process
and exchanges the JSON messages defined in spec/client-protocol.schema.json.
"""

from ._client import Engine, ExtractResult, TraceQAError, extract

__all__ = ["Engine", "ExtractResult", "TraceQAError", "extract"]
