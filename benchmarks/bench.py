"""Benchmark simpy-rs against Python's standard SimPy library."""

from __future__ import annotations

import random
import time
from collections.abc import Callable
from typing import Any

import simpy as py_simpy
import simpy_rs as rs_simpy


def bench(label: str, fn: Callable[[], object]) -> float:
    start = time.perf_counter()
    fn()
    elapsed = time.perf_counter() - start
    print(f"{label:<46} {elapsed:8.4f} s")
    return elapsed


def bench_timeouts(mod: Any, n_procs: int = 100, steps: int = 2_000) -> None:
    env = mod.Environment()

    def worker():
        for _ in range(steps):
            yield env.timeout(1)

    for _ in range(n_procs):
        env.process(worker())
    env.run()


def bench_resource(mod: Any, n_jobs: int = 50_000) -> None:
    env = mod.Environment()
    res = mod.Resource(env, capacity=4)

    def worker(n: int):
        for _ in range(n):
            with res.request() as req:
                yield req
                yield env.timeout(1)

    per = n_jobs // 10
    for _ in range(10):
        env.process(worker(per))
    env.run()


def bench_priority_resource(mod: Any, n_jobs: int = 30_000) -> None:
    env = mod.Environment()
    res = mod.PriorityResource(env, capacity=4)

    def worker(n: int, prio: int):
        for _ in range(n):
            with res.request(priority=prio) as req:
                yield req
                yield env.timeout(1)

    per = n_jobs // 10
    for i in range(10):
        env.process(worker(per, i % 5))
    env.run()


def bench_store(mod: Any, n_items: int = 50_000) -> None:
    env = mod.Environment()
    store = mod.Store(env, capacity=16)

    def producer():
        for i in range(n_items):
            yield store.put(i)

    def consumer():
        for _ in range(n_items):
            yield store.get()

    env.process(producer())
    env.process(consumer())
    env.run()


def bench_container(mod: Any, n_ops: int = 50_000) -> None:
    env = mod.Environment()
    tank = mod.Container(env, capacity=100, init=50)

    def producer():
        for _ in range(n_ops):
            yield tank.put(1)

    def consumer():
        for _ in range(n_ops):
            yield tank.get(1)

    env.process(producer())
    env.process(consumer())
    env.run()


def bench_conditions(mod: Any, n_ops: int = 30_000) -> None:
    env = mod.Environment()

    def worker():
        for _ in range(n_ops):
            yield env.timeout(1) | env.timeout(2)

    env.process(worker())
    env.run()


def bench_mm1_generator(mod: Any, until: float = 50_000.0) -> None:
    rng = random.Random(42)
    env = mod.Environment()
    server = mod.Resource(env, capacity=1)

    def customer():
        with server.request() as req:
            yield req
            yield env.timeout(rng.expovariate(1.0))

    def arrivals():
        while True:
            yield env.timeout(rng.expovariate(0.8))
            env.process(customer())

    env.process(arrivals())
    env.run(until=until)


def main() -> None:
    workloads: list[tuple[str, Callable[[Any], None]]] = [
        ("timeouts (200k events)", bench_timeouts),
        ("Resource (50k req/rel)", bench_resource),
        ("PriorityResource (30k)", bench_priority_resource),
        ("Store (50k put/get)", bench_store),
        ("Container (50k put/get)", bench_container),
        ("AnyOf conditions (30k)", bench_conditions),
        ("M/M/1 drop-in (T=50k)", bench_mm1_generator),
    ]

    print(f"{'Workload':<46} {'Time':>10}")
    print("-" * 58)

    results: list[tuple[str, float, float]] = []
    for name, fn in workloads:
        t_py = bench(f"Python SimPy: {name}", lambda f=fn: f(py_simpy))
        t_rs = bench(f"simpy-rs:     {name}", lambda f=fn: f(rs_simpy))
        results.append((name, t_py, t_rs))
        print("-" * 58)

    t_py_mm1 = results[-1][1]
    t_rs_native = bench(
        "simpy-rs native: simulate_mm1(T=50k)",
        lambda: rs_simpy.simulate_mm1(0.8, 1.0, until=50_000.0, seed=42),
    )

    print("\nSummary Speedups vs Python SimPy:")
    print(f"{'Workload':<28} {'SimPy (s)':>11} {'simpy-rs (s)':>13} {'Speedup':>10}")
    for name, t_py, t_rs in results:
        print(f"{name:<28} {t_py:11.4f} {t_rs:13.4f} {t_py / t_rs:9.2f}x")
    print(
        f"{'M/M/1 native (Rust)':<28} {t_py_mm1:11.4f} {t_rs_native:13.4f} "
        f"{t_py_mm1 / t_rs_native:9.1f}x"
    )


if __name__ == "__main__":
    main()
