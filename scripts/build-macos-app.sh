#!/bin/bash

set -euo pipefail

SCRIPT_DIRECTORY="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIRECTORY}/.." && pwd)"
SOURCE_DIRECTORY="${PROJECT_ROOT}/apps/strajer-macos"
OUTPUT_APP_BUNDLE="${PROJECT_ROOT}/dist/Strajer.app"
APP_BUNDLE="${PROJECT_ROOT}/dist/Strajer.build.app"
APP_CONTENTS="${APP_BUNDLE}/Contents"
APP_EXECUTABLES="${APP_CONTENTS}/MacOS"
APP_RESOURCES="${APP_CONTENTS}/Resources"
BUILD_DIRECTORY="${PROJECT_ROOT}/dist/build-macos"
SERVER_URL="${STRAJER_SERVER_URL:-http://127.0.0.1:18080}"
BUILD_ARCHITECTURES="${STRAJER_ARCHS:-arm64 x86_64}"
MACOS_SDK="$(xcrun --sdk macosx --show-sdk-path)"

case "${SERVER_URL}" in
    http://*|https://*)
        ;;
    *)
        echo "STRAJER_SERVER_URL must use http:// or https://" >&2
        exit 1
        ;;
esac

rust_target_for_architecture() {
    case "$1" in
        arm64)
            echo "aarch64-apple-darwin"
            ;;
        x86_64)
            echo "x86_64-apple-darwin"
            ;;
        *)
            echo "Unsupported macOS architecture: $1" >&2
            return 1
            ;;
    esac
}

swift_target_for_architecture() {
    case "$1" in
        arm64)
            echo "arm64-apple-macos13.0"
            ;;
        x86_64)
            echo "x86_64-apple-macos13.0"
            ;;
        *)
            echo "Unsupported macOS architecture: $1" >&2
            return 1
            ;;
    esac
}

cd "${PROJECT_ROOT}"

rm -rf "${APP_BUNDLE}" "${BUILD_DIRECTORY}"
mkdir -p "${APP_EXECUTABLES}" "${APP_RESOURCES}" "${BUILD_DIRECTORY}"

RUST_BINARIES=()
SWIFT_BINARIES=()

for architecture in ${BUILD_ARCHITECTURES}; do
    rust_target="$(rust_target_for_architecture "${architecture}")"
    swift_target="$(swift_target_for_architecture "${architecture}")"

    cargo build \
        --locked \
        --release \
        --package strajer-agent \
        --target "${rust_target}"

    rust_binary="${PROJECT_ROOT}/target/${rust_target}/release/strajer-agent"
    swift_binary="${BUILD_DIRECTORY}/Strajer-${architecture}"
    module_cache="${BUILD_DIRECTORY}/module-cache-${architecture}"
    mkdir -p "${module_cache}"

    xcrun swiftc \
        -parse-as-library \
        -swift-version 5 \
        -warnings-as-errors \
        -O \
        -module-cache-path "${module_cache}" \
        -target "${swift_target}" \
        -sdk "${MACOS_SDK}" \
        -framework AppKit \
        -framework SwiftUI \
        "${SOURCE_DIRECTORY}/Sources/AgentController.swift" \
        "${SOURCE_DIRECTORY}/Sources/StrajerApp.swift" \
        -o "${swift_binary}"

    RUST_BINARIES+=("${rust_binary}")
    SWIFT_BINARIES+=("${swift_binary}")
done

lipo -create "${RUST_BINARIES[@]}" -output "${APP_EXECUTABLES}/strajer-agent"
lipo -create "${SWIFT_BINARIES[@]}" -output "${APP_EXECUTABLES}/Strajer"
chmod 0755 "${APP_EXECUTABLES}/strajer-agent" "${APP_EXECUTABLES}/Strajer"

install -m 0644 "${SOURCE_DIRECTORY}/Info.plist" "${APP_CONTENTS}/Info.plist"
/usr/libexec/PlistBuddy \
    -c "Set :StrajerServerURL ${SERVER_URL}" \
    "${APP_CONTENTS}/Info.plist"
plutil -lint "${APP_CONTENTS}/Info.plist"

codesign --force --sign - --timestamp=none "${APP_EXECUTABLES}/strajer-agent"
codesign --force --sign - --timestamp=none "${APP_BUNDLE}"
codesign --verify --deep --strict "${APP_BUNDLE}"

rm -rf "${OUTPUT_APP_BUNDLE}"
mv "${APP_BUNDLE}" "${OUTPUT_APP_BUNDLE}"
rm -rf "${BUILD_DIRECTORY}"

echo "Built ${OUTPUT_APP_BUNDLE}"
echo "Architectures: ${BUILD_ARCHITECTURES}"
echo "Server: ${SERVER_URL}"
