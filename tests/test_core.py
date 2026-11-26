"""Core tests for Environment, Event, Timeout, Process, Condition, RealtimeEnvironment, and util."""

from __future__ import annotations

import time

import pytest
import simpy_rs as simpy
from simpy_rs import (
    AllOf,
    AnyOf,
    Condition,
    EmptySchedule,
    Environment,
    Event,
    Infinity,
    Interrupt,
    RealtimeEnvironment,
    SimPyException,
    StopSimulation,
)
from simpy_rs.util import start_delayed, subscribe_at


def test_environment_basics() -> None:
    env = Environment()
    assert env.now == 0
    assert env.peek() == Infinity
    assert env.active_process is None

    with pytest.raises(EmptySchedule):
        env.step()

    env2 = Environment(initial_time=10.5)
    assert env2.now == 10.5
    env2.run(until=20)
    assert env2.now == 20.0

    with pytest.raises(ValueError):
        env2.run(until=20)
    with pytest.raises(ValueError):
        env2.run(until=15)


def test_event_lifecycle_and_callbacks() -> None:
    env = Environment()
    ev = env.event()
    assert not ev.triggered
    assert not ev.processed
    assert ev.callbacks == []

    with pytest.raises(AttributeError):
        _ = ev.ok
    with pytest.raises(AttributeError):
        _ = ev.value

    seen: list[object] = []
    ev.callbacks.append(lambda e: seen.append(e.value))

    ret = ev.succeed("hello")
    assert ret is ev
    assert ev.triggered
    assert not ev.processed
    assert ev.ok is True
    assert ev.value == "hello"

    with pytest.raises(RuntimeError):
        ev.succeed("again")

    env.run()
    assert ev.processed
    assert ev.callbacks is None
    assert seen == ["hello"]


def test_event_fail_and_defuse() -> None:
    env = Environment()
    ev = env.event()
    with pytest.raises(ValueError):
        ev.fail("not an exception")  # type: ignore[arg-type]

    ev.fail(ValueError("boom"))
    assert ev.triggered
    assert ev.ok is False

    with pytest.raises(ValueError, match="boom"):
        env.run()

    # Defused failure should not crash environment
    env2 = Environment()
    ev2 = env2.event()
    ev2.fail(ValueError("ignored"))
    ev2.defused = True
    env2.run()
    assert ev2.processed


def test_event_trigger() -> None:
    env = Environment()
    e1 = env.event()
    e2 = env.event()
    e1.succeed(123)
    e2.trigger(e1)
    env.run()
    assert e2.ok is True
    assert e2.value == 123


def test_timeout_and_negative_delay() -> None:
    env = Environment()
    t = env.timeout(5, value="done")
    assert isinstance(t, Event)
    assert env.peek() == 5.0
    res = env.run(until=t)
    assert res == "done"
    assert env.now == 5.0

    with pytest.raises(ValueError):
        env.timeout(-1)


def test_process_execution_and_return_value() -> None:
    env = Environment()
    log: list[tuple[float, str]] = []

    def sub():
        yield env.timeout(2)
        return 42

    def main():
        assert env.active_process is proc
        log.append((env.now, "start"))
        val = yield env.process(sub())
        log.append((env.now, f"sub={val}"))
        yield env.timeout(3)
        log.append((env.now, "end"))
        return "finished"

    proc = env.process(main())
    assert proc.name == "main"
    assert proc.is_alive
    res = env.run(until=proc)
    assert res == "finished"
    assert not proc.is_alive
    assert log == [(0.0, "start"), (2.0, "sub=42"), (5.0, "end")]


def test_process_invalid_generator_and_yield() -> None:
    env = Environment()
    with pytest.raises(ValueError):
        env.process(123)  # type: ignore[arg-type]

    def bad_yield():
        yield "not an event"  # type: ignore[misc]

    env.process(bad_yield())
    with pytest.raises(RuntimeError, match="Invalid yield value"):
        env.run()


def test_process_interrupt() -> None:
    env = Environment()
    caught: list[tuple[float, object]] = []

    def victim():
        try:
            yield env.timeout(10)
            caught.append((env.now, "not interrupted"))
        except Interrupt as exc:
            assert isinstance(exc, SimPyException)
            caught.append((env.now, exc.cause))
            yield env.timeout(2)
            caught.append((env.now, "recovered"))

    def attacker(target):
        yield env.timeout(3)
        target.interrupt("wake up")

    v = env.process(victim())
    env.process(attacker(v))
    env.run()
    assert caught == [(3.0, "wake up"), (5.0, "recovered")]
    assert env.now == 10.0  # original timeout still passes harmlessly without re-waking victim


def test_process_interrupt_errors() -> None:
    env = Environment()

    def self_interrupt():
        yield env.timeout(1)
        assert env.active_process is not None
        env.active_process.interrupt("self")

    env.process(self_interrupt())
    with pytest.raises(RuntimeError, match="not allowed to interrupt itself"):
        env.run()

    env2 = Environment()

    def quick():
        yield env2.timeout(1)

    p = env2.process(quick())
    env2.run()
    with pytest.raises(RuntimeError, match="has terminated"):
        p.interrupt()


def test_conditions_all_of_any_of_and_operators() -> None:
    env = Environment()
    results: list[object] = []

    def proc():
        t1 = env.timeout(2, value="a")
        t2 = env.timeout(5, value="b")
        res_any = yield t1 | t2
        assert t1 in res_any
        assert t2 not in res_any
        assert res_any[t1] == "a"
        assert res_any == {t1: "a"}
        assert len(res_any) == 1
        results.append(("any", env.now, res_any.todict()))

        t3 = env.timeout(1, value="c")
        t4 = env.timeout(3, value="d")
        res_all = yield t3 & t4
        assert t3 in res_all and t4 in res_all
        assert res_all[t3] == "c" and res_all[t4] == "d"
        assert list(res_all.values()) == ["c", "d"]
        results.append(("all", env.now, res_all.todict()))

        # Nested condition flattening
        n1 = env.timeout(1, value=1)
        n2 = env.timeout(2, value=2)
        n3 = env.timeout(3, value=3)
        nested = yield (n1 & n2) & n3
        assert len(nested) == 3
        assert nested[n1] == 1 and nested[n2] == 2 and nested[n3] == 3
        results.append(("nested", env.now))

    env.process(proc())
    env.run()
    assert results[0][0:2] == ("any", 2.0)
    assert results[1][0:2] == ("all", 5.0)
    assert results[2] == ("nested", 8.0)


def test_condition_empty_and_custom_and_cross_env() -> None:
    env = Environment()
    env2 = Environment()

    with pytest.raises(ValueError, match="different environments"):
        AllOf(env, [env2.timeout(1)])

    empty_all = env.all_of([])
    empty_any = env.any_of([])
    env.run()
    assert empty_all.ok and len(empty_all.value) == 0
    assert empty_any.ok and len(empty_any.value) == 0

    # Custom Condition: trigger when 2 of 3 events complete
    env3 = Environment()
    evs = [env3.timeout(1, "a"), env3.timeout(2, "b"), env3.timeout(5, "c")]
    cond = Condition(env3, lambda events, count: count >= 2, evs)
    res = env3.run(until=cond)
    assert env3.now == 2.0
    assert len(res) == 2


def test_run_until_untriggered_event_raises() -> None:
    env = Environment()
    never = env.event()
    with pytest.raises(RuntimeError, match="not triggered"):
        env.run(until=never)


def test_stop_simulation_callback() -> None:
    env = Environment()
    ev = env.timeout(3, value="stop-val")
    ev.callbacks.append(StopSimulation.callback)
    env.timeout(10)
    res = env.run()
    assert res == "stop-val"
    assert env.now == 3.0


def test_realtime_environment() -> None:
    env = RealtimeEnvironment(factor=0.02, strict=True)
    assert env.factor == 0.02
    assert env.strict is True

    def ticker():
        yield env.timeout(1)
        yield env.timeout(1)

    t0 = time.monotonic()
    env.process(ticker())
    env.run()
    elapsed = time.monotonic() - t0
    assert env.now == 2.0
    assert elapsed >= 0.03  # 2 * 0.02s = 0.04s (allow small timer jitter)

    # Strict mode raises if simulation lags behind real time
    env_slow = RealtimeEnvironment(factor=0.01, strict=True)

    def slow_proc():
        time.sleep(0.05)
        yield env_slow.timeout(1)

    env_slow.process(slow_proc())
    with pytest.raises(RuntimeError, match="too slow"):
        env_slow.run()


def test_util_helpers() -> None:
    env = Environment()
    started_at: list[float] = []

    def worker():
        started_at.append(env.now)
        yield env.timeout(2)

    start_delayed(env, worker(), delay=5)
    env.run()
    assert started_at == [5.0]
    assert env.now == 7.0

    # subscribe_at
    env2 = Environment()
    interrupted: list[tuple[float, object]] = []
    trigger_ev = env2.timeout(4, value="signal")

    def listener():
        subscribe_at(trigger_ev)
        try:
            yield env2.timeout(10)
        except Interrupt as exc:
            interrupted.append((env2.now, exc.cause[1]))

    env2.process(listener())
    env2.run()
    assert interrupted == [(4.0, "signal")]


def test_submodule_exports() -> None:
    assert simpy.core.Environment is Environment
    assert simpy.events.AllOf is AllOf
    assert simpy.events.AnyOf is AnyOf
    assert simpy.exceptions.Interrupt is Interrupt
    assert simpy.rt.RealtimeEnvironment is RealtimeEnvironment
    assert simpy.resources.Resource is simpy.Resource
