"""What the generated models module exports."""

from __future__ import annotations

from traceqa_protocol import models


def test_no_stray_root_model() -> None:
    # The generator once emitted `class Model(RootModel[Any])` for the bundle root. Nothing uses it.
    assert not hasattr(models, "Model")
