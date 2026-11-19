#!/bin/bash
# Shared by the trial viewer and shortcuts. Keep the certificate name out of diagnostics.
daily_sign_identity() {
    local identity=""
    if [ -f "$HOME/.config/istesso/sign.env" ]; then
        identity=$(set +u; . "$HOME/.config/istesso/sign.env" >/dev/null 2>&1; printf '%s' "${ISTESSO_SIGN_IDENTITY:-}")
    fi
    printf '%s' "${identity:--}"
}

daily_sign_bundle() { # bundle identifier identity
    local signed
    codesign --force --sign "$3" --identifier "$2" "$1" >/dev/null 2>&1 ||
        { echo "error: signing the trial bundle failed" >&2; return 1; }
    codesign --verify --strict "$1" 2>/dev/null ||
        { echo "error: the signature does not hold" >&2; return 1; }
    signed=$(codesign -dvv "$1" 2>&1)
    grep -Fxq "Identifier=$2" <<< "$signed" && grep -q '^Info.plist entries=' <<< "$signed" ||
        { echo "error: the bundle identifier or sealed Info.plist is missing" >&2; return 1; }
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    daily_sign_bundle "$1" "$2" "$(daily_sign_identity)"
fi
