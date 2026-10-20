#!/bin/sh
# discord/build.sh: build Vencord with the seimei connector plugin into ~/.local/share/seimei/vencord, the dist dir
# Vesktop loads once its Vencord Location points there (Vesktop settings, Developer Settings).
# Vencord stays at the commit in vencord-commit, the one Vesktop had shipped, so only the plugin is new.
set -eu
here=$(dirname "$(readlink -f "$0")")
src="${VENCORD_SRC:-$HOME/src/vencord}"
out="$HOME/.local/share/seimei/vencord"
commit=$(cat "$here/vencord-commit")
[ -d "$src/.git" ] || git clone -q https://github.com/Vendicated/Vencord "$src"
git -C "$src" cat-file -e "$commit^{commit}" 2>/dev/null || git -C "$src" fetch -q origin
git -C "$src" checkout -q --detach "$commit"
(cd "$src" && nice -n 19 pnpm install -s --frozen-lockfile 2>&1 | grep -v "configured to use" || true)
mkdir -p "$src/src/userplugins/seimeiConnector"
cp "$here/seimeiConnector/index.ts" "$here/seimeiConnector/native.ts" "$src/src/userplugins/seimeiConnector/"
# No updater: Vencord's own would replace this build (and the plugin) with the next upstream release.
# Updates come from moving vencord-commit and deploying.
log=$(cd "$src" && nice -n 19 pnpm -s build --standalone --disable-updater 2>&1) || { printf '%s\n' "$log"; exit 1; }
# Vesktop wants these four and a package.json; swap them in together so a running Vesktop never sees a mix.
mkdir -p "$out"
tmp="$out.new"
mkdir -p "$tmp"
for f in vencordDesktopMain.js vencordDesktopPreload.js vencordDesktopRenderer.js vencordDesktopRenderer.css; do
    cp "$src/dist/$f" "$tmp/$f"
done
printf '{}\n' > "$tmp/package.json"
for f in package.json vencordDesktopMain.js vencordDesktopPreload.js vencordDesktopRenderer.js vencordDesktopRenderer.css; do
    mv -f "$tmp/$f" "$out/$f"
done
rmdir "$tmp"
grep -q SeimeiConnector "$out/vencordDesktopRenderer.js" && grep -q discord.sock "$out/vencordDesktopMain.js"
echo "vencord with SeimeiConnector in $out"
