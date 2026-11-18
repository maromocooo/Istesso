#!/bin/bash
# PROTOTYPE, for home use only (docs/m8-aura-prototype.md): the Istesso client on AURA, built
# without Gradle, from the Android SDK's own tools.
#
#   android/aura/build.sh            build the Rust library for aarch64-linux-android and the APK
#   android/aura/build.sh keystore   make the development signing key, once (outside the repository)
#   android/aura/build.sh check      what the APK holds: identifier, name, library, signature
#   android/aura/build.sh install    install it (adb, the device named by ANDROID_SERIAL if several)
#   android/aura/build.sh uninstall  remove it from the device, with its storage (its key and pairing)
#
# Only the identifier com.maromoco.istesso.dev.aura is ever installed or removed (the owner's
# permission covers that one alone). Nothing else on the device is touched.
#
# The measuring signal (AGENTS.md) is obeyed: nothing starts while it says busy, and every long
# step looks at it every 5 seconds and is stopped when it turns busy.
#
# Tools (all user-level): the Android SDK (build-tools 36.0.0, platform android-37.0, NDK
# 27.1.12297006, platform-tools), the JDK that comes with Android Studio, rustup's target
# aarch64-linux-android. Nothing system-wide.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
package=com.maromoco.istesso.dev.aura
version_name=0.0.1-proto

sdk=${ANDROID_HOME:-$HOME/Library/Android/sdk}
build_tools=$sdk/build-tools/${AURA_BUILD_TOOLS:-36.0.0}
platform_jar=$sdk/platforms/${AURA_PLATFORM:-android-37.0}/android.jar
ndk=$sdk/ndk/${AURA_NDK:-27.1.12297006}
llvm=$ndk/toolchains/llvm/prebuilt/darwin-x86_64/bin
adb=$sdk/platform-tools/adb
java_home=${JAVA_HOME:-"/Applications/Android Studio.app/Contents/jbr/Contents/Home"}
out=$repo/target/android-aura
# The development signing key: outside the repository, for this user only.
key_dir=${AURA_KEY_DIR:-$HOME/.config/istesso/android-dev}
keystore=$key_dir/istesso-dev.p12
key_pass=$key_dir/istesso-dev.pass
apk=$out/istesso-aura-proto.apk

signal_busy() { ! "$repo/scripts/measuring.sh" status > /dev/null; }

refuse_when_busy() {
    if signal_busy; then
        echo "a latency measurement is running (scripts/measuring.sh status): nothing is started" >&2
        exit 3
    fi
}

# Runs a command in the background and stops it within 10 s when the signal turns busy.
guarded() {
    refuse_when_busy
    "$@" &
    local pid=$!
    while kill -0 "$pid" 2> /dev/null; do
        sleep 5
        if signal_busy; then
            echo "the measuring signal turned busy: stopping (process $pid)" >&2
            kill -TERM "$pid" 2> /dev/null || true
            wait "$pid" 2> /dev/null || true
            exit 3
        fi
    done
    wait "$pid"
}

need() { [ -e "$1" ] || { echo "missing: $1 ($2)" >&2; exit 1; }; }

do_keystore() {
    if [ -e "$keystore" ]; then
        echo "the development signing key is there already: $keystore"
        return
    fi
    need "$java_home/bin/keytool" "the JDK of Android Studio"
    mkdir -p "$key_dir"
    chmod 700 "$key_dir"
    # A random password in a file of this user's only; never printed.
    # (Not `tr < /dev/urandom | head`: under pipefail, tr's broken pipe would end this script.)
    (umask 077 && od -An -tx1 -N24 /dev/urandom | tr -d ' \n' > "$key_pass")
    "$java_home/bin/keytool" -genkeypair -keystore "$keystore" -storetype PKCS12 \
        -storepass:file "$key_pass" -keypass:file "$key_pass" \
        -alias istesso-dev -keyalg RSA -keysize 3072 -validity 3650 \
        -dname "CN=Istesso prototype (home use only)" > /dev/null
    chmod 600 "$keystore"
    echo "made the development signing key: $keystore (password in $key_pass, this user only)"
}

do_build() {
    need "$build_tools/aapt2" "build-tools"
    need "$platform_jar" "the platform"
    need "$llvm/aarch64-linux-android34-clang" "the NDK"
    need "$java_home/bin/javac" "the JDK of Android Studio"
    need "$keystore" "android/aura/build.sh keystore"
    rustup target list --installed | grep -qx aarch64-linux-android ||
        { echo "missing: rustup target aarch64-linux-android (rustup target add aarch64-linux-android)" >&2; exit 1; }

    rm -rf "$out/stage" "$out/classes" "$out/gen" "$out/dex"
    mkdir -p "$out/stage/lib/arm64-v8a" "$out/classes" "$out/gen" "$out/dex"

    echo "== the Rust library"
    # The NDK's linker for this target, for this command only (no cargo configuration is written).
    (cd "$repo" &&
        export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$llvm/aarch64-linux-android34-clang" \
            CC_aarch64_linux_android="$llvm/aarch64-linux-android34-clang" \
            AR_aarch64_linux_android="$llvm/llvm-ar" &&
        guarded cargo build --release --target aarch64-linux-android -p istesso-aura --features quickack)
    cp "$repo/target/aarch64-linux-android/release/libistesso_aura.so" "$out/stage/lib/arm64-v8a/"

    echo "== resources and manifest"
    guarded "$build_tools/aapt2" compile --dir "$here/res" -o "$out/res.zip"
    guarded "$build_tools/aapt2" link -o "$out/base.apk" -I "$platform_jar" \
        --manifest "$here/AndroidManifest.xml" --java "$out/gen" \
        --min-sdk-version 34 --target-sdk-version 34 \
        --version-code 1 --version-name "$version_name" "$out/res.zip"

    echo "== Java"
    guarded "$java_home/bin/javac" -encoding UTF-8 -source 17 -target 17 -Xlint:-options \
        -classpath "$platform_jar" -d "$out/classes" \
        $(find "$here/src" "$out/gen" -name '*.java')
    guarded "$build_tools/d8" --release --min-api 34 --lib "$platform_jar" --output "$out/dex" \
        $(find "$out/classes" -name '*.class')

    echo "== the package"
    cp "$out/base.apk" "$out/unsigned.apk"
    cp "$out/dex/classes.dex" "$out/stage/"
    (cd "$out/stage" && zip -q -r "$out/unsigned.apk" classes.dex lib)
    guarded "$build_tools/zipalign" -f -P 16 4 "$out/unsigned.apk" "$out/aligned.apk"
    guarded "$build_tools/apksigner" sign --ks "$keystore" --ks-key-alias istesso-dev \
        --ks-pass "file:$key_pass" --out "$apk" "$out/aligned.apk"
    rm -f "$out/unsigned.apk" "$out/aligned.apk"
    echo "built: $apk"
    do_check
}

do_check() {
    need "$apk" "android/aura/build.sh"
    local badging
    badging=$("$build_tools/aapt2" dump badging "$apk")
    grep -q "^package: name='$package'" <<< "$badging" || { echo "fail: the identifier is not $package" >&2; exit 1; }
    grep -q "versionName='$version_name'" <<< "$badging" || { echo "fail: the version is not $version_name" >&2; exit 1; }
    grep -q "application-label:'.*試作" <<< "$badging" || { echo "fail: the name does not say that it is a prototype" >&2; exit 1; }
    # Read whole before searching: under pipefail, grep -q ending early fails the pipe.
    local listing
    listing=$(unzip -l "$apk")
    grep -q "lib/arm64-v8a/libistesso_aura.so" <<< "$listing" || { echo "fail: the library is not in the package" >&2; exit 1; }
    "$build_tools/apksigner" verify "$apk" > /dev/null || { echo "fail: the signature does not verify" >&2; exit 1; }
    echo "ok: $package $version_name, the library for arm64-v8a, signed"
}

# One device, the one adb names (ANDROID_SERIAL when there are several).
do_install() {
    need "$apk" "android/aura/build.sh"
    do_check
    guarded "$adb" install -r "$apk"
}

do_uninstall() {
    refuse_when_busy
    "$adb" uninstall "$package"
}

case "${1:-build}" in
build) do_build ;;
keystore) do_keystore ;;
check) do_check ;;
install) do_install ;;
uninstall) do_uninstall ;;
*) echo "usage: android/aura/build.sh [build|keystore|check|install|uninstall]" >&2; exit 2 ;;
esac
