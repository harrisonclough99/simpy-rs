# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-11-26

### Added
- Initial Rust + PyO3 (`abi3-py39`) implementation of `simpy-rs` with drop-in `SimPy` 4.x compatibility.
- Core discrete-event simulation engine in Rust: `Environment`, `Event`, `Timeout`, `Process`, `Initialize`, `Interruption`, `Condition`, `AllOf`, `AnyOf`, `ConditionValue`, `Interrupt`, `SimPyException`, `EmptySchedule`, `StopSimulation`.
- Full resource suite in Rust: `Resource`, `PriorityResource`, `PreemptiveResource` (`Preempted`), `Container`, `Store`, `PriorityStore` (`PriorityItem`), `FilterStore`.
- `RealtimeEnvironment` for wall-clock synchronized simulations.
- High-performance Rust extensions: `Monitor` for zero-overhead time-series/wait-time statistics and `simulate_mmc` / `simulate_mm1` native Rust queueing simulation runners.
- PEP 561 type stubs (`py.typed`, `__init__.pyi`), GitHub Actions CI (`cargo test`, `cargo clippy`, `pytest`, `ruff`), and Trusted Publishing release workflow (`PyO3/maturin-action`).
