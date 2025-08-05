# cargo-customs 🛃

`cargo-customs` is a cargo subcommand that enforces **regulations** across the crates of a Rust workspace — especially useful for large, heterogeneous workspaces targeting multiple (or embedded) architectures.

The regulations defined for `cargo-customs` simplify the manual invocation of commands like `cargo build && cargo check && cargo clippy && cargo test`
into a single step. They can be customized to selectively run on specific target platforms or build targets.

---

## Why `cargo-customs`?

When running `cargo build` at workspace level, `cargo build` is invoked on all workspace members. 
To optimize the build process, all features from all dependencies are unified across the workspace.

The somewhat unfortunate consequence is that any features which are **not additive** can break the compilation of other workspace members.

A typical example would be a crate `foo` that depends on `thiserror` with the `std` feature enabled, and a crate `bar` targeting an embedded system, which depends on `thiserror` without `std`.
Due to feature unification, `thiserror` is compiled with `std`, which breaks the compilation of `bar` when Cargo is invoked at the workspace level.

Similar blockers occur when building binaries for embedded systems using `#![no_main]`, as those fail to compile for the native system, or when invoking `cargo test` on crates that do not support it.

`cargo-customs` solves this by allowing you to explicitly specify which build targets are intended for which platform targets (architectures), and which operations (e.g. `build`, `check`, `test`) should be run on them.

After defining these **regulations** for your packages, `cargo-customs` automatically performs the inspections and identifies any violations. 😄

---

## Usage

`cargo install cargo-customs`

Each crate can define a `Customs.toml` file next to its `Cargo.toml`:

```toml
# ./my-crate/Customs.toml

[[regulation]]
platform-targets = ["host"]
build-targets = ["all"]
jobs = ["build", "check", "clippy", "test"]

[[regulation]]
platform-targets = ["thumbv7em-none-eabihf"]
build-targets = ["bin:sensor_node"]
jobs = ["build"]
```

You can also define a default regulation at the workspace root to avoid repetition.

Then just run:

```bash
cargo customs
```

`cargo-customs` will expand and execute every regulation to all combinations of `platform-targets`, `build-targets`, and `jobs`.

The `"all"` build target translates to cargo's `--all-targets`, and the `"host"` platform target is automatically resolved to your native host architecture.

---

## Status

`cargo-customs` is under active development. Contributions, feedback, and use cases are welcome!

This is currently a small utility based on my own day-to-day development needs, but from digging through cargo issues and forum threads, it appears this might be useful to others as well.

🚧 The `Customs.toml` format may change without a major version bump for any releases before `1.0.0`.

