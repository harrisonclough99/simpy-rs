# Contributing to `simpy-rs`

Thank you for your interest in contributing to `simpy-rs`!

## Prerequisites

- **Rust** (stable toolchain via [rustup](https://rustup.rs/))
- **Python** 3.9+
- **uv** or **pip** + **venv**

## Local Development Setup

1. Clone the repository and create a virtual environment:

   ```bash
   git clone https://github.com/harrisonclough99/simpy-rs.git
   cd simpy-rs
   python -m venv .venv
   # Linux / macOS:
   source .venv/bin/activate
   # Windows (PowerShell):
   .venv\Scripts\Activate.ps1
   ```

2. Install development dependencies and build the Rust extension in-place:

   ```bash
   pip install maturin pytest ruff simpy
   maturin develop --release
   ```

   > Use `maturin develop` (without `--release`) for faster incremental debug builds while iterating, and `maturin develop --release` before running benchmarks.

3. Run the test suites and linters:

   ```bash
   cargo test
   cargo clippy --all-targets -- -D warnings
   pytest -v
   ruff check .
   ```

4. Run the benchmark suite:

   ```bash
   python benchmarks/bench.py
   ```

## Pull Request Scope Guidelines

- **Keep PRs focused**: One feature, resource, or bugfix per PR. Unrelated refactors should be submitted separately.
- **Drop-in compatibility**: Core classes, methods, exceptions, and edge-case semantics should match Python `SimPy` 4.x.
- **Tests required**: Every change must include Rust unit tests (`cargo test`) and Python parity tests (`pytest`) comparing against upstream `simpy`.
- **No performance regressions**: Avoid unnecessary Python object allocations inside hot simulation loops; keep queue scheduling and event dispatch in Rust.
- **Clean checks**: Ensure `cargo clippy --all-targets -- -D warnings` and `ruff check .` pass with zero warnings before opening a PR.

## License

By contributing, you agree that your contributions will be dual-licensed under the [MIT License](LICENSE-MIT) and [Apache License 2.0](LICENSE-APACHE).
