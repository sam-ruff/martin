# martin-embedded

Thin wrapper crate for embedding the martin tile server inside another Rust
application, aimed at statically compiled binaries that serve offline maps.
This crate is the main addition on top of upstream maplibre/martin, but the
fork also carries a few targeted changes to `martin` and `martin-core` that back
this crate's API, plus a trimmed CI. Keep that diff small so the fork rebases
cleanly (`git fetch upstream && git rebase upstream/main`); the full list is
under "Syncing with upstream" below.

## Syncing with upstream

Everything the fork changes on top of `upstream/main`:

- **This crate** - `martin-embedded/`, plus its registration in the workspace
  `Cargo.toml` members and `Cargo.lock`.
- **`srv.disable_signals`** (backs `config.srv.disable_signals`) -
  `martin/src/config/file/srv.rs`, `martin/src/srv/server.rs`.
- **Cache invalidation / `CacheInvalidator`** (backs
  `CacheInvalidator::invalidate_source`) -
  `martin-core/src/tiles/pmtiles/cache.rs`, `martin/src/tile_source_manager.rs`,
  `martin/src/config/file/main/config.rs`,
  `martin/src/config/file/main/lifecycle.rs`.
- **CI** - reduced to a single workflow, `.github/workflows/embedded-ci.yml`,
  which only builds and tests this crate (`cargo build -p martin-embedded` /
  `cargo test -p martin-embedded`). All inherited upstream workflows were deleted
  because they failed on the fork and emailed noise; `main` has no branch
  protection, so nothing gates on them.

The first three groups touch upstream files, so expect the occasional
rebase/merge conflict there - resolve by keeping the fork's additions. Every
upstream pull also re-adds or modifies the deleted workflow files: after each
sync, re-delete everything under `.github/workflows/` except `embedded-ci.yml`
(keep the reused `.github/actions/setup-rust` action) plus
`.github/dependabot.yml`. Do not re-enable workspace-wide clippy or tests - see
"Known quirks".

## API

Everything is re-exported from this crate; consumers do not need to depend on
`martin` directly.

- `load_config(path)` - read a martin YAML config file with `${ENV_VAR}` substitution
- `start(config)` - resolve sources and bind; returns `(ServerFuture, bound_addr, CacheInvalidator)`. Call `CacheInvalidator::invalidate_source(id)` after replacing a tile file on disk so the swapped source serves fresh content while other sources stay cached
- `serve(config)` - start and run until the server exits
- Re-exports: `Config`, `FileConfigEnum`, `read_config`, `MartinError`,
  `MartinResult`, and martin's `config` and `logging` modules

`ServerFuture` is not `Send`: await it on the task that created it, or run
`serve` on a dedicated runtime or thread. The library never initialises
tracing; the embedding application owns that (martin's helpers are available
via the re-exported `logging` module).

Programmatic config example:

```rust
let mut config = martin_embedded::Config::default();
config.srv.listen_addresses = Some("127.0.0.1:3000".into());
// Host apps that own shutdown should keep actix's process-wide SIGINT/SIGTERM
// handlers out of the way and stop the server future themselves.
config.srv.disable_signals = Some(true);
config.mbtiles = martin_embedded::FileConfigEnum::new(vec!["maps/world.mbtiles".into()]);
martin_embedded::serve(config).await?;
```

## Features

Defaults: `mbtiles`, `pmtiles`, `fonts`, `sprites`, `styles`. Opt-in:
`geojson`, `postgres`, `metrics`, `webui`. All forward to the matching martin
feature. At least one tile source feature is required (compile error
otherwise). `webui` needs npm at build time; `rendering` is deliberately not
exposed because maplibre_native cannot be cross-compiled for musl.

## Consuming from another project

```toml
martin-embedded = { git = "https://github.com/sam-ruff/martin" }
```

## Commands

```bash
cargo test -p martin-embedded                  # includes the HTTP integration test
cargo clippy -p martin-embedded --all-targets
cargo run -p martin-embedded --example serve   # serves a repo fixture on 127.0.0.1:3111

# static binary; plain cargo + musl-gcc fails on a C++ dep, use zigbuild
cargo zigbuild -p martin-embedded --example serve --release --target x86_64-unknown-linux-musl
```

## Known quirks

- `cargo clippy -p martin` under this non-default feature combination shows a
  handful of pre-existing upstream warnings (feature-gating gaps). They are
  left unfixed on purpose to keep the diff against upstream minimal.
- The integration test binds a fixed port (3199); it will fail if that port is
  taken.
