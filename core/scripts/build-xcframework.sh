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
# Keep Rust objects compatible with the app's declared minimum. Without this,
# rustc inherits the current SDK version and Xcode links a macOS 26 library into
# a macOS 14 app. Callers can override this when the app target changes.
: "${MACOSX_DEPLOYMENT_TARGET:=14.0}"
export MACOSX_DEPLOYMENT_TARGET

verify_macos_minimum() {
    library=$1
    incompatible=$(
        otool -l "$library" 2>/dev/null | awk -v maximum="$MACOSX_DEPLOYMENT_TARGET" '
            function version_code(version, parts) {
                split(version, parts, ".")
                return (parts[1] + 0) * 1000000 + (parts[2] + 0) * 1000 + (parts[3] + 0)
            }
            $1 == "minos" && version_code($2) > version_code(maximum) { print $2 }
        ' | sort -u
    )
    if [ -n "$incompatible" ]; then
        echo "error: $library contains objects requiring macOS $incompatible; the app targets macOS $MACOSX_DEPLOYMENT_TARGET" >&2
        echo "error: use an official rustup toolchain whose standard library supports the deployment target" >&2
        exit 1
    fi
}

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
    library="target/$target/$PROFILE/libpathlight_core.a"
    verify_macos_minimum "$library"
    LIBS="${LIBS:+$LIBS }$library"
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
# UniFFI's Swift template emits trailing spaces in a few generated declarations.
# Normalize them here so regenerating bindings leaves a reviewable worktree.
sed 's/[[:space:]]*$//' \
    "$STAGING/bindings/PathlightRustCore.swift" \
    > swift/Sources/PathlightRustCore/PathlightRustCore.swift

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
