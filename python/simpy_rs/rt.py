"""Real-time simulation environment synchronized with wall-clock time."""

from __future__ import annotations

import time
from typing import Any

from ._simpy_rs import EmptySchedule, Environment, Event, Infinity, StopSimulation


class RealtimeEnvironment(Environment):
    """Execution environment for a real-time simulation.

    Synchronizes simulation time with wall-clock ``time.monotonic()`` scaled by ``factor``.
    """

    def __new__(
        cls,
        initial_time: float = 0.0,
        factor: float = 1.0,
        strict: bool = True,
    ) -> RealtimeEnvironment:
        return super().__new__(cls, initial_time)

    def __init__(
        self,
        initial_time: float = 0.0,
        factor: float = 1.0,
        strict: bool = True,
    ) -> None:
        self._env_start = initial_time
        self._real_start = time.monotonic()
        self._factor = factor
        self._strict = strict

    @property
    def factor(self) -> float:
        return self._factor

    @property
    def strict(self) -> bool:
        return self._strict

    def sync(self) -> None:
        """Synchronize the internal time with the current wall-clock time."""
        self._env_start = self.now
        self._real_start = time.monotonic()

    def step(self) -> None:
        """Process the next event after waiting until real time catches up."""
        evt_time = self.peek()
        if evt_time is Infinity or evt_time == Infinity:
            raise EmptySchedule()

        real_time = self._real_start + (evt_time - self._env_start) * self._factor

        if self._strict and time.monotonic() - real_time > self._factor:
            delta = time.monotonic() - real_time
            raise RuntimeError(f"Simulation too slow for real time ({delta:.3f}s).")

        while True:
            delta = real_time - time.monotonic()
            if delta <= 0:
                break
            time.sleep(delta)

        super().step()

    def run(self, until: float | Event | None = None) -> Any:
        """Execute ``step()`` until ``until`` is reached or no events remain."""
        if until is not None:
            if not isinstance(until, Event):
                at = float(until)
                if at <= self.now:
                    raise ValueError(f"until(={at}) must be > the current simulation time.")
                until_ev = self.event()
                until_ev.succeed()
                until_ev.callbacks.append(StopSimulation.callback)
                self.schedule(until_ev, priority=0, delay=at - self.now)
                is_numeric = True
            else:
                if until.processed:
                    if until.ok:
                        return until.value
                    raise until.value
                until_ev = until
                until_ev.callbacks.append(StopSimulation.callback)
                is_numeric = False
        else:
            until_ev = None
            is_numeric = False

        try:
            while True:
                self.step()
        except StopSimulation as exc:
            return None if is_numeric else exc.value
        except EmptySchedule:
            if until_ev is not None:
                if not until_ev.triggered:
                    raise RuntimeError(
                        f'No scheduled events left but "until" event was not triggered: {until_ev}'
                    ) from None
                return None if is_numeric else until_ev.value
            return None


__all__ = ["RealtimeEnvironment"]
