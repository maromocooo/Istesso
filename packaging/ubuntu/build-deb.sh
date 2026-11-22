#!/bin/bash
# Builds the Ubuntu package (.deb) from programs that are already built. Run it where
# dpkg-deb exists (an Ubuntu or Debian machine); it installs nothing.
#
# Usage: packaging/ubuntu/build-deb.sh --version <x.y.z> [--arch amd64] [--out <directory>]
#            [--licenses <directory>] <istesso> [<more programs>...]
#   <istesso>      the built command line tool; installed as /usr/bin/istesso
#   <more>         other built programs, installed in /usr/bin under their own names
#   --licenses     a folder of third-party licence texts (third-party-licenses/); copied to
#                  /usr/share/doc/istesso/third-party-licenses/. Default: the one in the
#                  repository, if it exists.
#   ISTESSO_DEB_DEPENDS  value of the Depends field (default: libc6). For a release derive it
#                  from the programs with dpkg-shlibdeps and set it here.
#
# The package is not signed here: the package repository is signed as a whole (docs/m9-plan.md).
set -euo pipefail

die() {
    echo "error: $*" >&2
    exit 1
}
usage() {
    sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
}

version=""
arch="amd64"
out="."
licenses=""
programs=()
while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) [ "$#" -ge 2 ] || die "--version needs a value"; version=$2; shift 2 ;;
        --arch) [ "$#" -ge 2 ] || die "--arch needs a value"; arch=$2; shift 2 ;;
        --out) [ "$#" -ge 2 ] || die "--out needs a value"; out=$2; shift 2 ;;
        --licenses) [ "$#" -ge 2 ] || die "--licenses needs a value"; licenses=$2; shift 2 ;;
        -h | --help) usage; exit 0 ;;
        -*) usage >&2; die "unknown option $1" ;;
        *) programs+=("$1"); shift ;;
    esac
done
if [ -z "$version" ] || [ "${#programs[@]}" -lt 1 ]; then
    usage >&2
    exit 2
fi
case "$version" in *[!0-9A-Za-z.+~-]*) die "odd characters in the version" ;; esac
case "$arch" in amd64 | arm64) ;; *) die "architecture must be amd64 or arm64" ;; esac
command -v dpkg-deb >/dev/null 2>&1 || die "dpkg-deb is not installed here; run this on Ubuntu or Debian"

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
if [ -z "$licenses" ] && [ -d "$repo/third-party-licenses" ]; then
    licenses="$repo/third-party-licenses"
fi
for p in "${programs[@]}"; do
    if [ ! -f "$p" ] || [ ! -x "$p" ]; then
        die "$p is not an executable file"
    fi
done
if [ -n "$licenses" ] && [ ! -d "$licenses" ]; then
    die "$licenses is not a directory"
fi
mkdir -p "$out"
out=$(cd "$out" && pwd)

# Staging folder. Everything the script deletes is inside it, and only after the check below.
stage_parent=${TMPDIR:-/tmp}
stage_parent=$(cd "$stage_parent" && pwd -P)
stage=$(mktemp -d "$stage_parent/istesso-deb.XXXXXX")
cleanup() {
    case "$stage" in
        "$stage_parent"/istesso-deb.??????)
            if [ -d "$stage" ] && [ ! -L "$stage" ]; then
                rm -rf -- "$stage"
            fi
            ;;
    esac
}
trap cleanup EXIT

root="$stage/root"
install -d -m 755 "$root/usr/bin" "$root/usr/lib/systemd/user" "$root/usr/lib/udev/rules.d" \
    "$root/etc/security/limits.d" "$root/usr/share/doc/istesso" "$root/DEBIAN"
for p in "${programs[@]}"; do
    install -m 755 "$p" "$root/usr/bin/$(basename "$p")"
done
install -m 644 "$here/systemd/istesso-agent.service" "$root/usr/lib/systemd/user/istesso-agent.service"
install -m 644 "$here/etc/70-istesso-usb.rules" "$root/usr/lib/udev/rules.d/70-istesso-usb.rules"
install -m 644 "$here/etc/40-istesso-limits.conf" "$root/etc/security/limits.d/40-istesso.conf"
install -m 644 "$repo/LICENSE-MIT" "$repo/LICENSE-APACHE" "$root/usr/share/doc/istesso/"
if [ -n "$licenses" ]; then
    mkdir -p "$root/usr/share/doc/istesso/third-party-licenses"
    cp -R "$licenses/." "$root/usr/share/doc/istesso/third-party-licenses/"
fi
cat > "$root/usr/share/doc/istesso/copyright" <<'COPYRIGHT'
Istesso is licensed under either of the MIT licence (LICENSE-MIT) or the Apache
Licence 2.0 (LICENSE-APACHE), at your option. Licence texts of third-party code that is
compiled into the programs are in the folder third-party-licenses/.
COPYRIGHT
chmod 644 "$root/usr/share/doc/istesso/copyright"

size=$(du -sk "$root" | cut -f1)
sed -e "s|@VERSION@|$version|" -e "s|@ARCH@|$arch|" -e "s|@SIZE@|$size|" \
    -e "s|@DEPENDS@|${ISTESSO_DEB_DEPENDS:-libc6}|" "$here/debian/control.in" > "$root/DEBIAN/control"
install -m 644 "$here/debian/conffiles" "$root/DEBIAN/conffiles"
install -m 755 "$here/debian/postinst" "$here/debian/postrm" "$root/DEBIAN/"

final="$out/istesso_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$root" "$final" >/dev/null
echo "$final"
echo "NOT SIGNED: the repository that serves this package is signed, not the file."
