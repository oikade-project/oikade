#!/bin/sh
set -eu

for script in scripts/*.sh; do
    bash -n "$script"
done

cargo fmt --all -- --check
cargo test --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps
cargo build --locked -p oikade-matter-adapter
python3 apps/oikade-matter-adapter/tests/rpc_integration.py \
    --binary target/debug/oikade-matter-adapter
./scripts/package-native_test.sh
