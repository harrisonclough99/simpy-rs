# `simpy-rs` Architecture & Design Principles

`simpy-rs` applies the PyO3 + Maturin engineering principles distilled from `faker-rs` to discrete-event simulation:

## 1. Rust-Native Event Loop & Direct Dispatch
- `Environment` stores its event queue in a Rust `BinaryHeap` ordered by `(time, priority, eid)` using `f64::total_cmp` (NaN values are rejected at API boundaries).
- Built-in callbacks (`Process`, `Condition`, `StopSimulation`, and all `Resource`/`Container`/`Store` triggers) are represented as a Rust `InternalCallback` enum rather than Python bound methods.
- `EventInner` stores an inline `first_cb: Option<InternalCallback>` slot alongside `extra_cbs: Vec<InternalCallback>`, eliminating per-event `Vec` heap allocations for the 99% single-waiter case.
- `py_callbacks: Option<Py<PyList>>` is allocated lazily only when user Python code accesses `event.callbacks`.

## 2. Re-entrancy Safety
- Simulation callbacks routinely re-enter `Environment` (`env.timeout`, `env.schedule`, `env.now`, `env.process`) and `Event` (`succeed`, `fail`).
- Every `Mutex` lock in `Environment`, `Event`, `Process`, and resources is strictly scoped to state mutation and **always released** before invoking any callback or Python generator (`send`/`throw`).

## 3. Cached Generator Dispatch
- `Process` caches the bound `generator.send` and `generator.throw` callables once upon creation, avoiding repeated attribute lookups on every `yield`.
- `EventInner::is_interruption` provides an O(1) Rust boolean check on the hot path instead of a CPython `isinstance(..., Interruption)` check.

## 4. Packaging & ABI Compatibility
- `Cargo.toml` enables `pyo3 = { version = "0.30", features = ["abi3-py39"] }` **without** `extension-module`, so `cargo test` links cleanly on all platforms.
- `pyproject.toml` passes `features = ["pyo3/extension-module"]` to `maturin` for single `cp39-abi3` wheel builds supporting Python 3.9–3.15.
- Hybrid `python/simpy_rs` + `_simpy_rs` layout ships PEP 561 `py.typed` and `__init__.pyi` stubs alongside drop-in submodules (`core`, `events`, `exceptions`, `resources`, `rt`, `util`).
