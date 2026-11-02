"""Deciding when a page has come to rest, with a fake clock so no test really waits."""

from __future__ import annotations

from typing import Any

from traceqa_protocol.settle import wait_until_settled


class World:
    """A fake clock, and a page whose reads and sleeps both take time."""

    def __init__(self, *, read_cost: float, sleep_cost: float, states: Any) -> None:
        self.time = 0.0
        self.reads = 0
        self.read_cost = read_cost
        self.sleep_cost = sleep_cost
        self.states = states

    def now(self) -> float:
        return self.time

    def sleep(self, seconds: float) -> None:
        self.time += self.sleep_cost or seconds

    def read(self) -> dict[str, Any]:
        self.reads += 1
        self.time += self.read_cost
        return self.states(self.reads)


def growing(reads: int) -> dict[str, Any]:
    return {"height": 1000 + reads, "animating": 0}


def test_slow_reads_count_against_the_limit() -> None:
    world = World(read_cost=1.0, sleep_cost=0.1, states=growing)
    wait_until_settled(world.read, world.sleep, 5, now=world.now)
    assert world.reads <= 6


def test_returns_after_two_quiet_checks() -> None:
    world = World(read_cost=0.0, sleep_cost=0.1, states=lambda _n: {"height": 1000, "animating": 0})
    wait_until_settled(world.read, world.sleep, 5, now=world.now)
    assert world.reads == 3


def test_animations_false_ignores_running_animations() -> None:
    world = World(read_cost=0.0, sleep_cost=0.1, states=lambda _n: {"height": 1000, "animating": 4})
    wait_until_settled(world.read, world.sleep, 5, animations=False, now=world.now)
    assert world.reads == 3


def test_running_animations_hold_the_page_until_the_limit() -> None:
    world = World(read_cost=0.0, sleep_cost=0.1, states=lambda _n: {"height": 1000, "animating": 1})
    wait_until_settled(world.read, world.sleep, 1, now=world.now)
    assert 9 <= world.reads <= 12
    assert world.time <= 1.3


def test_gives_up_when_the_page_never_rests() -> None:
    world = World(read_cost=0.0, sleep_cost=0.1, states=growing)
    wait_until_settled(world.read, world.sleep, 2, now=world.now)
    assert 19 <= world.reads <= 22


def test_says_whether_the_page_was_quiet() -> None:
    quiet = World(read_cost=0.0, sleep_cost=0.1, states=lambda _n: {"height": 1000, "animating": 0})
    assert wait_until_settled(quiet.read, quiet.sleep, 5, now=quiet.now) is True
    busy = World(read_cost=0.0, sleep_cost=0.1, states=growing)
    assert wait_until_settled(busy.read, busy.sleep, 1, now=busy.now) is False
