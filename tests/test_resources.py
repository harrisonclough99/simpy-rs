"""Tests for Resource, PriorityResource, PreemptiveResource, Container, and Store classes."""

from __future__ import annotations

import pytest
import simpy_rs as simpy


def test_resource_fifo_and_context_manager() -> None:
    env = simpy.Environment()
    res = simpy.Resource(env, capacity=1)
    assert res.capacity == 1
    assert res.count == 0
    log: list[tuple[str, float, float]] = []

    def user(name: str, delay: float, hold: float):
        yield env.timeout(delay)
        with res.request() as req:
            yield req
            start = env.now
            assert res.count == 1
            yield env.timeout(hold)
            log.append((name, start, env.now))

    env.process(user("A", 0, 5))
    env.process(user("B", 1, 3))
    env.process(user("C", 2, 2))
    env.run()
    assert res.count == 0
    assert log == [("A", 0.0, 5.0), ("B", 5.0, 8.0), ("C", 8.0, 10.0)]

    with pytest.raises(ValueError):
        simpy.Resource(env, capacity=0)


def test_resource_reneging_cancel() -> None:
    env = simpy.Environment()
    res = simpy.Resource(env, capacity=1)
    outcomes: list[tuple[str, str, float]] = []

    def holder():
        with res.request() as req:
            yield req
            yield env.timeout(10)

    def impatient(name: str, patience: float):
        with res.request() as req:
            result = yield req | env.timeout(patience)
            if req in result:
                outcomes.append((name, "got", env.now))
                yield env.timeout(2)
            else:
                outcomes.append((name, "reneged", env.now))

    env.process(holder())
    env.process(impatient("B", 3))
    env.process(impatient("C", 15))
    env.run()
    assert outcomes == [("B", "reneged", 3.0), ("C", "got", 10.0)]
    assert res.count == 0
    assert len(res.queue) == 0


def test_priority_resource() -> None:
    env = simpy.Environment()
    res = simpy.PriorityResource(env, capacity=1)
    order: list[tuple[str, float]] = []

    def worker(name: str, delay: float, prio: int):
        yield env.timeout(delay)
        with res.request(priority=prio) as req:
            yield req
            order.append((name, env.now))
            yield env.timeout(3)

    env.process(worker("first", 0, 1))
    env.process(worker("low", 1, 5))
    env.process(worker("high", 1, -1))
    env.process(worker("mid", 2, 0))
    env.run()
    assert [n for n, _ in order] == ["first", "high", "mid", "low"]


def test_preemptive_resource() -> None:
    env = simpy.Environment()
    res = simpy.PreemptiveResource(env, capacity=1)
    events: list[tuple[str, float]] = []

    def user(name: str, delay: float, prio: int, preempt: bool = True):
        yield env.timeout(delay)
        with res.request(priority=prio, preempt=preempt) as req:
            yield req
            events.append((f"{name}:start", env.now))
            try:
                yield env.timeout(5)
                events.append((f"{name}:done", env.now))
            except simpy.Interrupt as interrupt:
                assert isinstance(interrupt.cause, simpy.Preempted)
                assert interrupt.cause.usage_since == 0.0
                assert interrupt.cause.resource is res
                events.append((f"{name}:preempted", env.now))

    env.process(user("low", 0, prio=2))
    env.process(user("high", 2, prio=1, preempt=True))
    env.run()
    assert events == [
        ("low:start", 0.0),
        ("low:preempted", 2.0),
        ("high:start", 2.0),
        ("high:done", 7.0),
    ]


def test_preemptive_resource_no_preempt_flag() -> None:
    env = simpy.Environment()
    res = simpy.PreemptiveResource(env, capacity=1)
    events: list[tuple[str, float]] = []

    def user(name: str, delay: float, prio: int, preempt: bool):
        yield env.timeout(delay)
        with res.request(priority=prio, preempt=preempt) as req:
            yield req
            events.append((f"{name}:start", env.now))
            yield env.timeout(5)
            events.append((f"{name}:done", env.now))

    env.process(user("low", 0, prio=2, preempt=True))
    env.process(user("high_polite", 2, prio=1, preempt=False))
    env.run()
    assert events == [
        ("low:start", 0.0),
        ("low:done", 5.0),
        ("high_polite:start", 5.0),
        ("high_polite:done", 10.0),
    ]


def test_container() -> None:
    env = simpy.Environment()
    tank = simpy.Container(env, capacity=100, init=20)
    assert tank.capacity == 100.0
    assert tank.level == 20.0
    log: list[tuple[str, float, float]] = []

    def consumer():
        yield tank.get(50)
        log.append(("got50", env.now, tank.level))

    def producer():
        yield env.timeout(3)
        yield tank.put(40)
        log.append(("put40", env.now, tank.level))

    env.process(consumer())
    env.process(producer())
    env.run()
    assert log == [("put40", 3.0, 10.0), ("got50", 3.0, 10.0)] or (
        tank.level == 10.0 and env.now == 3.0
    )

    with pytest.raises(ValueError):
        simpy.Container(env, capacity=0)
    with pytest.raises(ValueError):
        simpy.Container(env, capacity=10, init=20)
    with pytest.raises(ValueError):
        tank.put(0)
    with pytest.raises(ValueError):
        tank.get(-5)


def test_store_fifo() -> None:
    env = simpy.Environment()
    store = simpy.Store(env, capacity=2)
    got: list[tuple[float, str]] = []

    def producer():
        for i in range(4):
            yield store.put(f"item-{i}")

    def consumer():
        for _ in range(4):
            yield env.timeout(1)
            item = yield store.get()
            got.append((env.now, item))

    env.process(producer())
    env.process(consumer())
    env.run()
    assert got == [
        (1.0, "item-0"),
        (2.0, "item-1"),
        (3.0, "item-2"),
        (4.0, "item-3"),
    ]


def test_priority_store_and_priority_item() -> None:
    env = simpy.Environment()
    store = simpy.PriorityStore(env)
    results: list[dict[str, str]] = []

    def producer():
        yield store.put(simpy.PriorityItem(3, {"name": "low"}))
        yield store.put(simpy.PriorityItem(1, {"name": "high"}))
        yield store.put(simpy.PriorityItem(2, {"name": "mid"}))

    def consumer():
        yield env.timeout(1)
        for _ in range(3):
            pi = yield store.get()
            results.append(pi.item)

    env.process(producer())
    env.process(consumer())
    env.run()
    assert results == [{"name": "high"}, {"name": "mid"}, {"name": "low"}]


def test_filter_store() -> None:
    env = simpy.Environment()
    store = simpy.FilterStore(env)
    got: list[tuple[str, tuple[str, int]]] = []

    def consumer(label: str, color: str):
        item = yield store.get(lambda x: x[0] == color)
        got.append((label, item))

    def producer():
        yield env.timeout(1)
        yield store.put(("blue", 1))
        yield env.timeout(1)
        yield store.put(("red", 2))

    # Red consumer queues BEFORE blue consumer, but blue arrives first and must not be blocked!
    env.process(consumer("want_red", "red"))
    env.process(consumer("want_blue", "blue"))
    env.process(producer())
    env.run()
    assert got == [("want_blue", ("blue", 1)), ("want_red", ("red", 2))]
