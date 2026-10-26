#!/bin/sh
# release.sh VERSION: build seimei VERSION with the update key built in, sign it with minisign and publish it as a
# GitHub release, the one `seimei update` installs, with the phone app beside it (the foss build, no server built in,
# signed with the app key android/publish.sh keeps). VERSION must be the version in crates/seimei/Cargo.toml.
# Once: `minisign -G -p contrib/minisign.pub -s ~/.minisign/seimei.key`, then commit contrib/minisign.pub.
# The key's password goes to minisign on stdin from ~/.config/seimei/minisign.pass (mode 600, outside the repo) when
# that file is there; otherwise minisign asks for it. It never shows in arguments or output.
set -eu
v=${1:?usage: release.sh VERSION}
root=$(dirname "$(dirname "$(readlink -f "$0")")")
cd "$root"
[ -f contrib/minisign.pub ] || { echo "no contrib/minisign.pub: make the key first (see the top of this script)"; exit 1; }
grep -q "^version = \"$v\"" crates/seimei/Cargo.toml || { echo "crates/seimei/Cargo.toml is not at $v"; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "commit first: the release is built from HEAD"; exit 1; }
# The browser extension as addons.mozilla.org signed it (plugins/browser/extension/sign.py), for the version in its manifest.
ext=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' plugins/browser/extension/manifest.json)
signed=$(python3 -c 'import json,sys,zipfile; print(json.loads(zipfile.ZipFile(sys.argv[1]).read("manifest.json"))["version"])' plugins/browser/extension/seimei.xpi)
[ "$ext" = "$signed" ] || { echo "plugins/browser/extension/seimei.xpi is $signed, the extension $ext: run plugins/browser/extension/sign.py"; exit 1; }
asset=seimei-$(uname -m)-linux
# Fully static (musl, mimalloc, vendored openssl): runs on any Linux of this arch. Needs rust-musl and musl (musl-gcc).
target=$(uname -m)-unknown-linux-musl
export "CC_$(echo "$target" | tr - _)=musl-gcc"
SEIMEI_UPDATE_KEY=$(tail -n 1 contrib/minisign.pub) nice -n 19 cargo build -q --release --locked -p seimei --target "$target"
mkdir -p dist
cp "target/$target/release/seimei" "dist/$asset"
ldd "dist/$asset" 2>&1 | grep -qE "statically linked|not a dynamic executable" || { echo "dist/$asset is not static"; exit 1; }
[ "$("dist/$asset" version)" = "seimei $v" ] || { echo "the build does not say seimei $v"; exit 1; }
pass=${SEIMEI_MINISIGN_PASS:-$HOME/.config/seimei/minisign.pass}
sign() { minisign -S -s "${SEIMEI_MINISIGN_KEY:-$HOME/.minisign/seimei.key}" -m "dist/$asset" -t "seimei $v $asset"; }
if [ -f "$pass" ]; then
    [ "$(stat -c %a "$pass")" = 600 ] || { echo "$pass must be mode 600"; exit 1; }
    sign < "$pass"
else
    sign
fi
minisign -V -p contrib/minisign.pub -m "dist/$asset"
android/publish.sh release dist/seimei.apk
cp plugins/browser/extension/seimei.xpi dist/seimei.xpi
gh release create "v$v" "dist/$asset" "dist/$asset.minisig" dist/seimei.apk dist/seimei.xpi --target "$(git rev-parse HEAD)" \
    --title "seimei $v" --notes "Install or update with \`seimei update\`; the binary is signed with contrib/minisign.pub. \
seimei.apk is the phone app (Android 11+): on first run it asks for your seimei's address. \
seimei.xpi is the browser plugin's extension for Firefox and its forks (about:addons, Install Add-on From File)."
