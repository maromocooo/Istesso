#!/bin/bash
# Builds an installer package (.pkg) from an application bundle that is already built.
#
# Usage: packaging/macos/build-pkg.sh <Istesso.app> [--version <x.y.z>] [--out <directory>]
#
# Signing, only when the environment names an identity (a certificate name or its SHA-1 hash;
# never put a name into a script, a document or a log):
#   ISTESSO_SIGN_APP_IDENTITY        "Developer ID Application" certificate: signs the bundle
#   ISTESSO_SIGN_INSTALLER_IDENTITY  "Developer ID Installer" certificate: signs the package
#   ISTESSO_ENTITLEMENTS             optional entitlements file for the bundle
#   ISTESSO_NOTARY_PROFILE           notarytool keychain profile: submit the package and staple
# Without ISTESSO_SIGN_APP_IDENTITY the bundle is not touched, and without
# ISTESSO_SIGN_INSTALLER_IDENTITY the package is unsigned. The script says so. An unsigned
# package is for trying the layout only: macOS will not keep the Local Network permission
# of a bundle that is not signed as a whole.
#
# The script installs nothing and never reads the keychain itself (codesign and productsign do).
set -euo pipefail
# No AppleDouble (._*) files in the payload.
export COPYFILE_DISABLE=1

app_dir_default="Applications"
label="com.maromoco.istesso.agent"
pkg_id="com.maromoco.istesso.pkg"

usage() {
    sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'
}

die() {
    echo "error: $*" >&2
    exit 1
}

bundle=""
version=""
out="."
while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) [ "$#" -ge 2 ] || die "--version needs a value"; version=$2; shift 2 ;;
        --out) [ "$#" -ge 2 ] || die "--out needs a value"; out=$2; shift 2 ;;
        -h | --help) usage; exit 0 ;;
        -*) usage >&2; die "unknown option $1" ;;
        *) [ -z "$bundle" ] || die "only one bundle can be given"; bundle=$1; shift ;;
    esac
done
[ -n "$bundle" ] || { usage >&2; exit 2; }

script_dir=$(cd "$(dirname "$0")" && pwd)
bundle=${bundle%/}
case "$bundle" in *.app) ;; *) die "$bundle is not an .app bundle" ;; esac
[ -d "$bundle" ] || die "$bundle is not a directory"
info="$bundle/Contents/Info.plist"
[ -f "$info" ] || die "$info is missing"

bundle_id=$(plutil -extract CFBundleIdentifier raw -o - "$info") || die "no CFBundleIdentifier"
executable=$(plutil -extract CFBundleExecutable raw -o - "$info") || die "no CFBundleExecutable"
[ -x "$bundle/Contents/MacOS/$executable" ] || die "Contents/MacOS/$executable is missing or not executable"
if [ -z "$version" ]; then
    version=$(plutil -extract CFBundleShortVersionString raw -o - "$info" 2>/dev/null || true)
fi
[ -n "$version" ] || die "no version: give --version or set CFBundleShortVersionString"
case "$version" in *[!0-9A-Za-z.+-]*) die "odd characters in the version" ;; esac
if [ "$bundle_id" != "com.maromoco.istesso" ]; then
    echo "warning: the bundle identifier is $bundle_id; the launchd plist names com.maromoco.istesso in AssociatedBundleIdentifiers" >&2
fi

app_name=$(basename "$bundle")
mkdir -p "$out"
out=$(cd "$out" && pwd)

# Staging folder. Everything the script deletes is inside it, and only after the check below.
stage_parent=${TMPDIR:-/tmp}
stage_parent=$(cd "$stage_parent" && pwd -P)
stage=$(mktemp -d "$stage_parent/istesso-pkg.XXXXXX")
cleanup() {
    case "$stage" in
        "$stage_parent"/istesso-pkg.??????)
            if [ -d "$stage" ] && [ ! -L "$stage" ]; then
                rm -rf -- "$stage"
            fi
            ;;
    esac
}
trap cleanup EXIT

root="$stage/root"
mkdir -p "$root/$app_dir_default" "$root/Library/LaunchAgents" "$root/usr/local/bin"
ditto --norsrc --noextattr "$bundle" "$root/$app_dir_default/$app_name"
staged_app="$root/$app_dir_default/$app_name"

# A measuring build (a local way in for the measurement's keys, ADR 0018) is never handed out.
# Same marks as `macos/client/build.sh check`; keep the two lists equal.
for mark in measure-keys istesso_measure_keys; do
    if grep -a -q -F -e "$mark" "$bundle/Contents/MacOS/$executable"; then
        die "the executable carries the mark of the measuring build ($mark); not releasing"
    fi
done

signed_app=no
if [ -n "${ISTESSO_SIGN_APP_IDENTITY:-}" ]; then
    echo "signing the bundle as a whole (identifier $bundle_id, hardened runtime, timestamp)"
    args=(--force --timestamp --options runtime --identifier "$bundle_id" --sign "$ISTESSO_SIGN_APP_IDENTITY")
    if [ -n "${ISTESSO_ENTITLEMENTS:-}" ]; then
        [ -f "$ISTESSO_ENTITLEMENTS" ] || die "ISTESSO_ENTITLEMENTS is not a file"
        args+=(--entitlements "$ISTESSO_ENTITLEMENTS")
    fi
    codesign "${args[@]}" "$staged_app"
    codesign --verify --strict --verbose=2 "$staged_app"
    # The identifier of the executable must equal the bundle identifier, and Info.plist must be sealed.
    details=$(codesign -dvv "$staged_app" 2>&1)
    echo "$details" | grep -q "^Identifier=$bundle_id\$" || die "the signed identifier is not $bundle_id"
    echo "$details" | grep -q "^Info.plist=not bound" && die "Info.plist is not sealed"
    signed_app=yes
else
    echo "UNSIGNED: ISTESSO_SIGN_APP_IDENTITY is not set; the bundle is copied as it is."
fi

# launchd agent. The path follows the bundle's real name.
agent_plist="$root/Library/LaunchAgents/$label.plist"
cp "$script_dir/$label.plist" "$agent_plist"
plutil -replace ProgramArguments.0 -string "/$app_dir_default/$app_name/Contents/MacOS/$executable" "$agent_plist"
plutil -lint "$agent_plist" >/dev/null

# The command line tool: a link to the bundle's executable (so the same signed program is used).
ln -s "/$app_dir_default/$app_name/Contents/MacOS/$executable" "$root/usr/local/bin/istesso"

mkdir -p "$stage/scripts"
cp "$script_dir/pkg-scripts/preinstall" "$script_dir/pkg-scripts/postinstall" "$stage/scripts/"
chmod 755 "$stage/scripts/preinstall" "$stage/scripts/postinstall"

# Extended attributes would turn into ._* files in the payload.
xattr -cr "$root" 2>/dev/null || true

# The bundle must be installed at its place, never "relocated" to another copy of it found
# elsewhere (a build folder, a disk image): the permissions are tied to that one place.
pkgbuild --analyze --root "$root" "$stage/components.plist" >/dev/null
count=$(grep -c "<key>RootRelativeBundlePath</key>" "$stage/components.plist" || true)
i=0
while [ "$i" -lt "$count" ]; do
    plutil -replace "$i.BundleIsRelocatable" -bool false "$stage/components.plist"
    plutil -replace "$i.BundleOverwriteAction" -string upgrade "$stage/components.plist"
    i=$((i + 1))
done

pkgbuild --root "$root" --component-plist "$stage/components.plist" \
    --identifier "$pkg_id" --version "$version" --install-location / \
    --scripts "$stage/scripts" --ownership recommended "$stage/component.pkg"

if [ -n "${ISTESSO_SIGN_INSTALLER_IDENTITY:-}" ]; then
    final="$out/istesso-$version-macos.pkg"
    productbuild --package "$stage/component.pkg" --sign "$ISTESSO_SIGN_INSTALLER_IDENTITY" --timestamp "$final"
    pkgutil --check-signature "$final" >/dev/null || die "the package signature does not verify"
    signed_pkg=yes
else
    final="$out/istesso-$version-macos-unsigned.pkg"
    productbuild --package "$stage/component.pkg" "$final"
    signed_pkg=no
    echo "UNSIGNED: ISTESSO_SIGN_INSTALLER_IDENTITY is not set; the package is not signed and not for release."
fi

if [ -n "${ISTESSO_NOTARY_PROFILE:-}" ]; then
    [ "$signed_app" = yes ] && [ "$signed_pkg" = yes ] || die "ISTESSO_NOTARY_PROFILE is set, but the bundle and the package are not both signed"
    echo "submitting for notarisation (this waits for Apple's answer)"
    xcrun notarytool submit "$final" --keychain-profile "$ISTESSO_NOTARY_PROFILE" --wait
    xcrun stapler staple "$final"
    xcrun stapler validate "$final"
else
    echo "NOT NOTARISED: ISTESSO_NOTARY_PROFILE is not set."
fi

echo "$final"
