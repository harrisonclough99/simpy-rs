# simpy-rs

[![CI](https://github.com/harrisonclough99/simpy-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/harrisonclough99/simpy-rs/actions/workflows/ci.yml)
[![PyPI version](https://img.shields.io/pypi/v/simpy-rs.svg)](https://pypi.org/project/simpy-rs/)
[![Python versions](https://img.shields.io/pypi/pyversions/simpy-rs.svg)](https://pypi.org/project/simpy-rs/)
[![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-blue.svg)](LICENSE-MIT)

A high-performance, drop-in replacement for Python's `SimPy` discrete-event simulation library, backed by Rust and [PyO3](https://pyo3.rs/).

## Why `simpy-rs`?

- **2.5x–3.7x faster drop-in simulations** for standard Python generator processes, resources, containers, stores, and conditions — with zero changes to your model logic.
- **150x+ faster native Rust queueing runners** (`simulate_mm1`, `simulate_mmc`) that execute the entire event loop without GIL or Python generator overhead.
- **Zero-overhead `Monitor`** for recording time-series observations, percentiles, and time-weighted averages in Rust.
- **Drop-in SimPy 4.x compatible**: `Environment`, `RealtimeEnvironment`, `Event`, `Timeout`, `Process`, `AllOf`, `AnyOf`, `Condition`, `Resource`, `PriorityResource`, `PreemptiveResource`, `Container`, `Store`, `PriorityStore`, `FilterStore`, `Interrupt`, `simpy_rs.util`, and PEP 561 type stubs (`py.typed`).
- **Pre-built `abi3` wheels** for Linux (`manylinux`), macOS, and Windows (Python 3.9–3.15) — no Rust compiler needed to `pip install`.

## 10-Second Quickstart

```bash
pip install simpy-rs
```

```python
import simpy_rs as simpy

def car(env: simpy.Environment):
    while True:
        print(f"Start parking at {env.now}")
        yield env.timeout(5)
        print(f"Start driving at {env.now}")
        yield env.timeout(2)

env = simpy.Environment()
env.process(car(env))
env.run(until=15)
```

## Drop-In Migration

Replace your import and keep your existing SimPy model unchanged:

```python
# Before
# import simpy

# After
import simpy_rs as simpy

env = simpy.Environment()
bcs = simpy.Resource(env, capacity=2)

def charge_car(env, name, bcs, driving_time, charge_duration):
    yield env.timeout(driving_time)
    with bcs.request() as req:
        yield req
        yield env.timeout(charge_duration)

for i in range(4):
    env.process(charge_car(env, f"Car {i}", bcs, i * 2, 5))

env.run()
```

### Native Rust Extensions (Zero-GIL Mode)

For high-throughput queueing sweeps and zero-overhead metric collection:

```python
from simpy_rs import Monitor, simulate_mm1, simulate_mmc

# Run a 50,000 time-unit M/M/1 simulation 100% in Rust (~2 ms)
stats = simulate_mm1(arrival_rate=0.8, service_rate=1.0, until=50_000.0, seed=42)
print(stats["mean_wait"], stats["server_utilization"])

# Multi-server M/M/c
stats_c = simulate_mmc(arrival_rate=2.4, service_rate=1.0, servers=3, until=50_000.0, seed=7)

# Zero-overhead statistics & time-weighted average monitor
mon = Monitor()
mon.record(0.0, time=0.0)
mon.record(3.0, time=2.0)
mon.record(1.0, time=5.0)
print(mon.mean, mon.percentile(95.0), mon.time_average(until=10.0))
```

## API Compatibility

| Category | Supported Symbols |
| --- | --- |
| **Core** | `Environment`, `RealtimeEnvironment`, `Infinity`, `URGENT`, `NORMAL`, `PENDING` |
| **Events & Processes** | `Event`, `Timeout`, `Process`, `Initialize`, `Interruption`, `Condition`, `AllOf`, `AnyOf`, `ConditionValue`, `&` / `\|` operators |
| **Resources** | `Resource`, `Request`, `Release`, `PriorityResource`, `PriorityRequest`, `PreemptiveResource`, `Preempted` |
| **Containers & Stores** | `Container`, `ContainerPut`, `ContainerGet`, `Store`, `StorePut`, `StoreGet`, `PriorityStore`, `PriorityItem`, `FilterStore`, `FilterStoreGet` |
| **Exceptions & Utils** | `SimPyException`, `Interrupt`, `EmptySchedule`, `StopSimulation`, `start_delayed`, `subscribe_at` |
| **Rust Extensions** | `Monitor`, `simulate_mm1`, `simulate_mmc` |

## Benchmarks

Measured on Windows (CPython 3.12.11, `simpy==4.1.2`, built with `maturin develop --release`) via `python benchmarks/bench.py`:

| Workload | Python `SimPy` | `simpy-rs` | Speedup |
| --- | ---: | ---: | ---: |
| `PriorityResource` (30k req/rel) | 0.1694 s | 0.0456 s | **3.72x** |
| `Resource` (50k req/rel) | 0.2346 s | 0.0638 s | **3.67x** |
| `Store` (50k put/get) | 0.1580 s | 0.0486 s | **3.25x** |
| `AnyOf` conditions (30k) | 0.1111 s | 0.0354 s | **3.14x** |
| `Container` (50k put/get) | 0.1650 s | 0.0547 s | **3.02x** |
| M/M/1 drop-in (`T=50k`) | 0.3404 s | 0.1178 s | **2.89x** |
| Timeouts (200k events) | 0.1995 s | 0.0797 s | **2.50x** |
| M/M/1 native (`simulate_mm1`, `T=50k`) | 0.3404 s | 0.0022 s | **153.5x** |

## Contributing

Contributions are welcome! See [CONTRIBUTING.md](CONTRIBUTING.md) for local `maturin develop` setup instructions and PR guidelines.

## License

Dual-licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
