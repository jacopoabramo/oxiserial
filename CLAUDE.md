# oxiserial

Serial port library for Python, implemented in Rust with PyO3 and built with
maturin. `import oxiserial as serial` replaces `pyserial` 3.5.
`oxiserial.aio.Serial` has the same API, but its I/O methods return a future
that can be waited on (`.wait()`) or awaited.

## Layout

- `src/`: all runtime code (Rust). The extension module is
  `oxiserial._oxiserial`.
- `python/oxiserial/`: `.pyi` stubs, `py.typed`, and an `__init__.py` that
  only re-exports `oxiserial._oxiserial`.
- `.github/workflows/CI.yml`: test and wheel jobs. Started from
  `maturin generate-ci github`, now maintained by hand.

## Rules

- `pyserial` 3.5 is the specification for the public API: names, signatures,
  defaults, exception types and behaviour.
- Code or text taken from pyserial gets a one-line comment saying so; the
  notice is in `LICENSES/pyserial.txt`.
- OS access goes through crates (`serialport`, `tokio-serial`, and `nix` or
  `windows-sys` for what those lack). No hand-written FFI.
- Every item exposed to Python has a matching stub entry; `stubtest` checks
  they agree.
- Blocking calls release the GIL. Shared state is behind a `Mutex` or
  atomics, since the module supports free-threaded CPython.
- CPython 3.11 or newer. Wheels are `abi3-py311` plus `cp314t`.
- Releases are cut by pushing a `vX.Y.Z` tag; CI sets the version from it.
- Dependencies go through `uv add` and `cargo add`, never by editing
  `pyproject.toml` or `Cargo.toml` by hand.

## Commands

```sh
uv sync
uv run maturin develop --uv --features test-backend
cargo fmt --check
cargo clippy --all-targets --features test-backend -- -D warnings
cargo test --features test-backend
uv run pytest
uv run ruff check python tests
uv run ruff format --check python tests
uv run mypy
uv run python -m mypy.stubtest oxiserial
```

Run all of them before reporting a change as done. Real-port tests need a
connected pair: `uv run pytest --port-a <A> --port-b <B>`.
