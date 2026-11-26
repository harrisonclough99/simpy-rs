"""Utility functions for ``simpy-rs`` matching ``simpy.util``."""

from __future__ import annotations

from collections.abc import Generator
from typing import Any

from ._simpy_rs import Environment, Event, Process


def start_delayed(
    env: Environment,
    generator: Generator[Event, Any, Any],
    delay: float,
) -> Process:
    """Return a helper process that starts ``generator`` after ``delay``."""
    if delay < 0:
        raise ValueError(f"delay(={delay}) must be >= 0.")

    def starter() -> Generator[Event, Any, Process]:
        yield env.timeout(delay)
        proc = env.process(generator)
        return proc

    return env.process(starter())


def subscribe_at(event: Event) -> None:
    """Register at ``event`` to receive an interrupt when it occurs."""
    env = event.env
    subscriber = env.active_process
    if subscriber is None:
        raise RuntimeError("subscribe_at() must be called from within a running Process.")

    def signaller(signaller_ev: Event, receiver: Process) -> Generator[Event, Any, None]:
        result = yield signaller_ev
        if receiver.is_alive:
            receiver.interrupt((signaller_ev, result))

    if event.callbacks is not None:
        env.process(signaller(event, subscriber))
    else:
        raise RuntimeError(f"{subscriber} has already been triggered.")


__all__ = ["start_delayed", "subscribe_at"]
