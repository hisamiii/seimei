#!/bin/sh
# webrtc.sh ABI NDK OUT: builds webrtc's audio processing (the freedesktop extraction, pinned below) as static libraries for one Android ABI into OUT.
# CMakeLists.txt runs it once per ABI; it needs git, meson and ninja, and the network the first time.
set -eu
abi=$1
ndk=$2
out=$3
tag=v2.1
commit=846fe90a289f58b7c9303a635142aa2c7caa93e5
[ -f "$out/lib/libwebrtc-audio-processing-2.a" ] && exit 0
case "$abi" in
    arm64-v8a) triple=aarch64-linux-android; family=aarch64; cpu=aarch64 ;;
    armeabi-v7a) triple=armv7a-linux-androideabi; family=arm; cpu=armv7a ;;
    x86_64) triple=x86_64-linux-android; family=x86_64; cpu=x86_64 ;;
    *) echo "webrtc.sh: no build for $abi" >&2; exit 1 ;;
esac
work=$(dirname "$out")
src="$work/src"
if [ ! -d "$src" ]; then
    git -c advice.detachedHead=false clone -q --depth 1 -b "$tag" https://gitlab.freedesktop.org/pulseaudio/webrtc-audio-processing.git "$src.tmp" 2>/dev/null
    [ "$(git -C "$src.tmp" rev-parse HEAD)" = "$commit" ] || { echo "webrtc.sh: $tag is not $commit" >&2; exit 1; }
    mv "$src.tmp" "$src"
fi
bin="$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin"
cross="$work/$abi.cross"
cat > "$cross" <<CROSS
[binaries]
c = '$bin/${triple}30-clang'
cpp = '$bin/${triple}30-clang++'
ar = '$bin/llvm-ar'
strip = '$bin/llvm-strip'
[host_machine]
system = 'android'
cpu_family = '$family'
cpu = '$cpu'
endian = 'little'
CROSS
# The bundled abseil, built with the library's own C++17: an installed one may want another standard.
meson setup "$work/build-$abi" "$src" --cross-file "$cross" --buildtype=release --prefix="$out" -Ddefault_library=static -Dgnustl=disabled \
    --force-fallback-for=absl_base,absl_flags,absl_strings,absl_numeric,absl_synchronization,absl_bad_optional_access >/dev/null
# The installed archive lacks abseil, whose thin archives stay in the build folder: CMakeLists.txt links them from there.
ninja -C "$work/build-$abi" install >/dev/null
