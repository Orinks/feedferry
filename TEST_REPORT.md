# Verification report

Generated on 2026-04-29.

What was checked in this environment:

- `cargo fmt --check`
- `cargo check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test -q`
- `cargo build -q`

Environment notes:

- The wxDragon build required installing LLVM/libclang and Ninja on Windows.
- Verification commands were run with LLVM/libclang and Ninja available.

Recommended local verification:

```bash
set LIBCLANG_PATH=<path-to-llvm-bin>
rustup update stable
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets
cargo run --release
```
