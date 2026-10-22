#!/bin/sh
# release.sh VERSION: build seimei VERSION with the update key built in, sign it with minisign and publish it as a
# GitHub release, the one `seimei update` installs. VERSION must be the version in crates/seimei/Cargo.toml.
# Once: `minisign -G -p contrib/minisign.pub -s ~/.minisign/seimei.key`, then commit contrib/minisign.pub.
# minisign asks for the key's password itself; it never passes through here.
set -eu
v=${1:?usage: release.sh VERSION}
root=$(dirname "$(dirname "$(readlink -f "$0")")")
cd "$root"
[ -f contrib/minisign.pub ] || { echo "no contrib/minisign.pub: make the key first (see the top of this script)"; exit 1; }
grep -q "^version = \"$v\"" crates/seimei/Cargo.toml || { echo "crates/seimei/Cargo.toml is not at $v"; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "commit first: the release is built from HEAD"; exit 1; }
asset=seimei-$(uname -m)-linux
SEIMEI_UPDATE_KEY=$(tail -n 1 contrib/minisign.pub) nice -n 19 cargo build -q --release --locked -p seimei
mkdir -p dist
cp target/release/seimei "dist/$asset"
[ "$("dist/$asset" version)" = "seimei $v" ] || { echo "the build does not say seimei $v"; exit 1; }
minisign -S -s "${SEIMEI_MINISIGN_KEY:-$HOME/.minisign/seimei.key}" -m "dist/$asset" -t "seimei $v $asset"
minisign -V -p contrib/minisign.pub -m "dist/$asset"
gh release create "v$v" "dist/$asset" "dist/$asset.minisig" --target "$(git rev-parse HEAD)" --title "seimei $v" \
    --notes "Install or update with \`seimei update\`; the binary is signed with contrib/minisign.pub."
