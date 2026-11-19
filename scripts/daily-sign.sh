#!/bin/bash
# Shared by the trial viewer and shortcuts. Keep the certificate name out of diagnostics.
daily_sign_identity() {
    local identity=""
    if [ -f "$HOME/.config/istesso/sign.env" ]; then
        identity=$(set +u; . "$HOME/.config/istesso/sign.env" >/dev/null 2>&1; printf '%s' "${ISTESSO_SIGN_IDENTITY:-}")
    fi
    printf '%s' "${identity:--}"
}

daily_sign_bundle() { # bundle identifier identity [signing options...]
    local bundle=$1 identifier=$2 identity=$3 signed
    shift 3
    codesign --force --sign "$identity" --identifier "$identifier" "$@" "$bundle" >/dev/null 2>&1 ||
        { echo "error: signing the trial bundle failed" >&2; return 1; }
    codesign --verify --strict "$bundle" 2>/dev/null ||
        { echo "error: the signature does not hold" >&2; return 1; }
    signed=$(codesign -dvv "$bundle" 2>&1)
    grep -Fxq "Identifier=$identifier" <<< "$signed" && grep -q '^Info.plist entries=' <<< "$signed" ||
        { echo "error: the bundle identifier or sealed Info.plist is missing" >&2; return 1; }
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    daily_sign_bundle "$1" "$2" "$(daily_sign_identity)" "${@:3}"
fi
