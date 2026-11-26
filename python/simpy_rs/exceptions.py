"""Drop-in submodule matching ``simpy.exceptions``."""

from ._simpy_rs import EmptySchedule, Interrupt, SimPyException, StopSimulation

__all__ = [
    "EmptySchedule",
    "Interrupt",
    "SimPyException",
    "StopSimulation",
]
