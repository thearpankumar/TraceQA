"""Deciding when a page has come to rest. Shared by every driver."""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

# A JavaScript expression. `animating` counts animations that are running and will end:
# an intro, a reveal on scroll. Animations that loop forever are not waited for.
PAGE_STATE = """(() => {
  var running = 0;
  (document.getAnimations ? document.getAnimations() : []).forEach(function (animation) {
    var timing = animation.effect && animation.effect.getComputedTiming ? animation.effect.getComputedTiming() : null;
    if (animation.playState === "running" && timing && isFinite(timing.endTime)) running++;
  });
  return { height: document.documentElement.scrollHeight, animating: running };
})()"""

STEP_S = 0.1


def wait_until_settled(
    read_state: Callable[[], dict[str, Any]], sleep: Callable[[float], None], limit_s: float
) -> None:
    """Return once the page height has stopped changing and no ending animation is running,
    for two checks in a row. Gives up after `limit_s`, because some pages never rest.

    `read_state` evaluates `PAGE_STATE` in the page. `sleep` waits the given number of seconds.
    """
    sleep(STEP_S)
    height = read_state()["height"]
    quiet = 0
    waited = 0.0
    while quiet < 2 and waited < limit_s:
        sleep(STEP_S)
        waited += STEP_S
        state = read_state()
        quiet = quiet + 1 if state["height"] == height and state["animating"] == 0 else 0
        height = state["height"]
