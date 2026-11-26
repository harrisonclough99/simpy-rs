"""Drop-in submodule matching ``simpy.core``."""

from ._simpy_rs import EmptySchedule, Environment, Infinity, StopSimulation

__all__ = [
    "EmptySchedule",
    "Environment",
    "Infinity",
    "StopSimulation",
]
