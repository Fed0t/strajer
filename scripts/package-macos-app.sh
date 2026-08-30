#!/bin/bash

set -euo pipefail

SCRIPT_DIRECTORY="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIRECTORY}/.." && pwd)"
APP_BUNDLE="${PROJECT_ROOT}/dist/Strajer.app"

if [[ ! -d "${APP_BUNDLE}" ]]; then
    echo "Strajer.app is missing; run scripts/build-macos-app.sh first" >&2
    exit 1
fi

VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "${APP_BUNDLE}/Contents/Info.plist")"
PACKAGE_PATH="${PROJECT_ROOT}/dist/Strajer-${VERSION}-macos-universal.zip"
CHECKSUM_PATH="${PACKAGE_PATH}.sha256"

for executable in Strajer strajer-agent; do
    architectures="$(lipo -archs "${APP_BUNDLE}/Contents/MacOS/${executable}")"
    case " ${architectures} " in
        *" arm64 "*)
            ;;
        *)
            echo "${executable} is not universal: ${architectures}" >&2
            exit 1
            ;;
    esac
    case " ${architectures} " in
        *" x86_64 "*)
            ;;
        *)
            echo "${executable} is not universal: ${architectures}" >&2
            exit 1
            ;;
    esac
done

codesign --verify --deep --strict "${APP_BUNDLE}"

rm -f "${PACKAGE_PATH}" "${CHECKSUM_PATH}"
ditto -c -k --sequesterRsrc --keepParent "${APP_BUNDLE}" "${PACKAGE_PATH}"

checksum="$(shasum -a 256 "${PACKAGE_PATH}" | awk '{print $1}')"
printf '%s  %s\n' "${checksum}" "$(basename "${PACKAGE_PATH}")" > "${CHECKSUM_PATH}"

echo "Packaged ${PACKAGE_PATH}"
echo "Checksum ${CHECKSUM_PATH}"
