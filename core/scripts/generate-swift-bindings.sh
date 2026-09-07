#!/bin/sh
# Builds the dylib and emits Swift bindings + modulemap into core/bindings/swift.
set -eu
cd "$(dirname "$0")/.."
cargo build
cargo run --quiet --features cli --bin uniffi-bindgen -- \
    generate --library target/debug/libpathlight_core.dylib \
    --language swift --out-dir bindings/swift
echo "Swift bindings written to core/bindings/swift"
