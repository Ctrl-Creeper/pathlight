#!/bin/sh
# Builds the Rust static library, regenerates the Swift bindings, and packages
# everything as core/swift/PathlightRustCoreFFI.xcframework for the Xcode app.
#
# Builds a universal (arm64 + x86_64) library when both Rust targets are
# installed (`rustup target add x86_64-apple-darwin`); otherwise host-only.
set -eu
cd "$(dirname "$0")/.."

PROFILE=${1:-release}
CARGO_FLAGS=""
[ "$PROFILE" = "release" ] && CARGO_FLAGS="--release"

HOST=$(rustc -vV | sed -n 's/^host: //p')
OTHER=""
case "$HOST" in
    aarch64-apple-darwin) OTHER=x86_64-apple-darwin ;;
    x86_64-apple-darwin) OTHER=aarch64-apple-darwin ;;
esac

TARGETS="$HOST"
if [ -n "$OTHER" ] && [ -d "$(rustc --print sysroot)/lib/rustlib/$OTHER" ]; then
    TARGETS="$HOST $OTHER"
fi

LIBS=""
for target in $TARGETS; do
    cargo build $CARGO_FLAGS --target "$target"
    LIBS="${LIBS:+$LIBS }target/$target/$PROFILE/libpathlight_core.a"
done

STAGING=target/xcframework
rm -rf "$STAGING" && mkdir -p "$STAGING/include"

# Any one target's dylib carries the UniFFI metadata the generator needs.
FIRST_TARGET=${TARGETS%% *}
cargo run --quiet --features cli --bin uniffi-bindgen -- \
    generate --library "target/$FIRST_TARGET/$PROFILE/libpathlight_core.dylib" \
    --language swift --out-dir "$STAGING/bindings"

cp "$STAGING/bindings/PathlightRustCoreFFI.h" "$STAGING/include/"
cp "$STAGING/bindings/PathlightRustCoreFFI.modulemap" "$STAGING/include/module.modulemap"
cp "$STAGING/bindings/PathlightRustCore.swift" swift/Sources/PathlightRustCore/PathlightRustCore.swift

if [ "$(echo $LIBS | wc -w)" -gt 1 ]; then
    lipo -create $LIBS -output "$STAGING/libpathlight_core.a"
    LIB="$STAGING/libpathlight_core.a"
else
    LIB=$LIBS
fi

rm -rf swift/PathlightRustCoreFFI.xcframework
xcodebuild -create-xcframework \
    -library "$LIB" -headers "$STAGING/include" \
    -output swift/PathlightRustCoreFFI.xcframework

echo "Built swift/PathlightRustCoreFFI.xcframework for: $TARGETS"
