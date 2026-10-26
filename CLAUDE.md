# oxiserial

Serial port library for Python, implemented in Rust with PyO3 and built with
maturin. `import oxiserial as serial` replaces `pyserial` 3.5.
`oxiserial.aio.Serial` has the same API, but its I/O methods return a future
that can be waited on (`.wait()`) or awaited.

## Layout

- `src/`: all runtime code (Rust). The extension module is
  `oxiserial._oxiserial`.
- `python/oxiserial/`: `.pyi` stubs, `py.typed`, an `__init__.py` that
  only re-exports `oxiserial._oxiserial`, and `threaded.py`.
- `.github/workflows/`: `ci.yaml` calls `run-tests.yaml` and
  `build-wheels.yaml`, then publishes. Started from `maturin generate-ci
  github`, now maintained by hand.

## Rules

- `pyserial` 3.5 is the specification for the public API: names, signatures,
  defaults, exception types and behaviour, except for documented differences
  that make usage simpler, each listed in the README's "Differences from
  pyserial".
- Code or text taken from pyserial gets a one-line comment saying so; the
  notice is in `LICENSES/pyserial.txt`.
- OS access goes through crates (`serialport`, `tokio-serial`, and `nix` or
  `windows-sys` for what those lack). No hand-written FFI, except a kernel
  struct that `libc` lacks, declared with the header it comes from named in
  a comment.
- Every item exposed to Python has a matching stub entry; `stubtest` checks
  they agree.
- Blocking calls release the GIL. Shared state is behind a `Mutex` or
  atomics, since the module supports free-threaded CPython.
- CPython 3.11 or newer. Wheels are `abi3-py311`, `cp314t` and `abi3t-py315`;
  the last is built with the `abi3t` feature, which only that build turns on.
- Releases are cut by pushing a `vX.Y.Z` tag; CI sets the version from it.
- Dependencies go through `uv add` and `cargo add`, never by editing
  `pyproject.toml` or `Cargo.toml` by hand.
- Pure-Python pyserial modules that do no I/O of their own, such as
  `threaded.py`, live in `python/oxiserial/`, rewritten for Python 3.11
  with full annotations.
- No Rust doctests: the crate is a `cdylib`, and `cargo test` and pytest
  cover its behaviour.

## Changelog entries

Each change a user will notice gets a file in `changelog.d/`, named after the
pull request and the kind of change, such as `412.fixed.md`. Open the pull
request first, so you know its number. A release collects the files into the
[changelog](CHANGELOG.md), one entry each, and links each entry to its pull
request.

| kind | use it when |
| --- | --- |
| `breaking` | code that uses `oxiserial` has to change |
| `added` | something new: a class, a function, a keyword, an option |
| `changed` | something that already existed behaves differently |
| `deprecated` | something still works but warns, and goes in a later release |
| `removed` | something is gone |
| `fixed` | something now works as its documentation says |
| `security` | a vulnerability is closed |

`towncrier` writes the file for you. Keep the text in single quotes, so the
shell leaves its backticks alone:

```bash
uv run towncrier create 412.fixed.md --content 'A timeout given to `read` is applied.'
```

A second change of the same kind in one pull request goes in `412.fixed.2.md`.

An entry says what changed, in a line or two, and names what a user would
type to reach it: a name the package exports as it is, and any other with
its module. It doesn't say why: that belongs in the pull request.

```markdown
Good: Add `timeout=` to `read`, which gives up when no reply comes in time.
Bad:  Improve read.
```

A change to something not released yet gets no entry of its own. Edit the
entry that added it instead, so the release says what users get.

A pull request nobody using `oxiserial` would notice, such as tests, CI, a
refactor or docs alone, needs no entry. A maintainer gives it the
`skip-changelog` label, and CI accepts it without one.

## Commands

```sh
uv sync
uv run maturin develop --uv --features test-backend
cargo fmt --check
cargo clippy --all-targets --features test-backend -- -D warnings
cargo test --features test-backend
uv run pytest
uv run ruff check python tests benchmarks
uv run ruff format --check python tests benchmarks
uv run mypy
uv run python -m mypy.stubtest oxiserial --allowlist stubtest-allowlist.txt
```

Run all of them before reporting a change as done. Real-port tests need a
connected pair: `uv run pytest --port-a=<A> --port-b=<B>`. Keep the `=`:
pytest takes a value after a space for a test path, and then does not read
`pyproject.toml`.

Benchmarks against pyserial, on a release build:
`uv run maturin develop --uv --release --features test-backend`, then
`uv run python benchmarks/run.py [--port-a <A> --port-b <B>]`.
