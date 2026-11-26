"""Head-to-head parity tests against upstream SimPy and tests for Rust extensions."""

from __future__ import annotations

import random
from typing import Any

import pytest
import simpy as py_simpy
import simpy_rs as rs_simpy


def _run_bank_renege(mod: Any) -> tuple[float, list[tuple[float, int, str]]]:
    rng = random.Random(42)
    env = mod.Environment()
    counter = mod.Resource(env, capacity=1)
    log: list[tuple[float, int, str]] = []

    def customer(cid: int):
        arrive = env.now
        log.append((round(arrive, 6), cid, "arrive"))
        with counter.request() as req:
            patience = rng.uniform(1.0, 3.0)
            results = yield req | env.timeout(patience)
            if req in results:
                log.append((round(env.now, 6), cid, "served"))
                yield env.timeout(rng.expovariate(1.0 / 12.0))
                log.append((round(env.now, 6), cid, "done"))
            else:
                log.append((round(env.now, 6), cid, "renege"))

    def source():
        for i in range(40):
            env.process(customer(i))
            yield env.timeout(rng.expovariate(1.0 / 10.0))

    env.process(source())
    env.run()
    return round(env.now, 6), log


def _run_gas_station(mod: Any) -> tuple[float, float, list[tuple[float, str, float]]]:
    rng = random.Random(99)
    env = mod.Environment()
    pump = mod.Resource(env, capacity=2)
    fuel_tank = mod.Container(env, capacity=200, init=200)
    log: list[tuple[float, str, float]] = []

    def tank_truck():
        yield env.timeout(15)
        amount = fuel_tank.capacity - fuel_tank.level
        yield fuel_tank.put(amount)
        log.append((round(env.now, 6), "refill", round(amount, 4)))

    def monitor():
        while True:
            if fuel_tank.level / fuel_tank.capacity < 0.25:
                yield env.process(tank_truck())
            yield env.timeout(5)

    def car(cid: int):
        with pump.request() as req:
            yield req
            liters = rng.randint(15, 45)
            yield fuel_tank.get(liters)
            yield env.timeout(liters / 2.0)
            log.append((round(env.now, 6), f"car-{cid}", round(fuel_tank.level, 4)))

    def car_gen():
        for i in range(25):
            yield env.timeout(rng.randint(3, 12))
            env.process(car(i))

    env.process(monitor())
    env.process(car_gen())
    env.run(until=250)
    return round(env.now, 6), round(fuel_tank.level, 4), log


def _run_preemptive_scenario(mod: Any) -> list[tuple[float, str]]:
    env = mod.Environment()
    res = mod.PreemptiveResource(env, capacity=1)
    log: list[tuple[float, str]] = []

    def job(name: str, start: float, prio: int, dur: float, preempt: bool):
        yield env.timeout(start)
        with res.request(priority=prio, preempt=preempt) as req:
            yield req
            log.append((round(env.now, 6), f"{name}:acquired"))
            try:
                yield env.timeout(dur)
                log.append((round(env.now, 6), f"{name}:finished"))
            except mod.Interrupt as exc:
                log.append((round(env.now, 6), f"{name}:preempted:{float(exc.cause.usage_since)}"))

    env.process(job("A", 0, 2, 10, True))
    env.process(job("B", 3, 1, 4, True))
    env.process(job("C", 4, 0, 2, False))
    env.process(job("D", 5, 0, 2, True))
    env.run()
    return log


def test_parity_bank_renege() -> None:
    assert _run_bank_renege(rs_simpy) == _run_bank_renege(py_simpy)


def test_parity_gas_station() -> None:
    assert _run_gas_station(rs_simpy) == _run_gas_station(py_simpy)


def test_parity_preemptive_resource() -> None:
    assert _run_preemptive_scenario(rs_simpy) == _run_preemptive_scenario(py_simpy)


def test_monitor_extension() -> None:
    m = rs_simpy.Monitor()
    assert len(m) == 0
    assert m.mean == 0.0
    assert m.min is None
    assert m.max is None

    m.record(10.0, time=0.0)
    m.record(20.0, time=2.0)
    m.record(30.0, time=6.0)
    assert len(m) == 3
    assert m.mean == pytest.approx(20.0)
    assert m.min == 10.0
    assert m.max == 30.0
    assert m.percentile(50.0) == pytest.approx(20.0)
    assert m.percentile(0.0) == pytest.approx(10.0)
    assert m.percentile(100.0) == pytest.approx(30.0)
    # Time average over [0, 10]: 10*2 + 20*4 + 30*4 = 220 / 10 = 22.0
    assert m.time_average(until=10.0) == pytest.approx(22.0)
    assert m.times() == [0.0, 2.0, 6.0]
    assert m.values() == [10.0, 20.0, 30.0]

    m.clear()
    assert len(m) == 0
    m.record_batch([0.0, 1.0, 2.0], [1.0, 3.0, 5.0])
    assert m.mean == pytest.approx(3.0)
    assert m.std > 0.0

    with pytest.raises(ValueError):
        m.record_batch([0.0], [1.0, 2.0])
    with pytest.raises(ValueError):
        m.percentile(150.0)


def test_native_mmc_and_mm1() -> None:
    res1 = rs_simpy.simulate_mm1(arrival_rate=0.7, service_rate=1.0, until=50_000.0, seed=42)
    res2 = rs_simpy.simulate_mm1(arrival_rate=0.7, service_rate=1.0, until=50_000.0, seed=42)
    assert res1 == res2
    assert res1["completed"] > 30_000
    assert res1["server_utilization"] == pytest.approx(0.7, abs=0.03)
    # Theoretical M/M/1 W_q = rho / (mu - lambda) = 0.7 / 0.3 = 2.333
    assert res1["mean_wait"] == pytest.approx(2.333, abs=0.35)

    mmc = rs_simpy.simulate_mmc(
        arrival_rate=2.4, service_rate=1.0, servers=3, until=50_000.0, seed=7
    )
    assert mmc["server_utilization"] == pytest.approx(0.8, abs=0.03)

    with pytest.raises(ValueError):
        rs_simpy.simulate_mmc(0.0, 1.0, 1, 100.0)
    with pytest.raises(ValueError):
        rs_simpy.simulate_mmc(1.0, 1.0, 0, 100.0)
