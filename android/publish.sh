#!/bin/sh
# publish.sh: build a signed release APK and put it where the web page's Phone app sheet offers it.
# The signing key is made once and kept outside the repo. Back it up: updates only install over the same key.
set -eu
here=$(dirname "$(readlink -f "$0")")
state="${XDG_DATA_HOME:-$HOME/.local/share}/seimei"
sign="$state/signing"
mkdir -p "$sign" "$state/app"
chmod 700 "$sign"
if [ ! -f "$sign/signing.properties" ]; then
    pw=$(head -c 24 /dev/urandom | base64 | tr -d '/+=')
    keytool -genkeypair -keystore "$sign/seimei.jks" -alias seimei -keyalg EC -groupname secp256r1 -sigalg SHA256withECDSA \
        -validity 36500 -storepass "$pw" -keypass "$pw" -dname "CN=seimei" >/dev/null 2>&1
    umask 077
    printf 'storeFile=%s\npassword=%s\nalias=seimei\n' "$sign/seimei.jks" "$pw" > "$sign/signing.properties"
    chmod 600 "$sign/seimei.jks" "$sign/signing.properties"
    echo "made the release signing key in $sign (back it up)"
fi
code=$(( $(cat "$sign/version" 2>/dev/null || echo 0) + 1 ))
name="0.$code"
cd "$here"
nice -n 19 ./gradlew -q --console=plain assembleRelease -Pseimei.versionCode="$code" -Pseimei.versionName="$name"
apk="$here/app/build/outputs/apk/release/app-release.apk"
cp "$apk" "$state/app/seimei.apk.tmp"
mv "$state/app/seimei.apk.tmp" "$state/app/seimei.apk"
printf '{"version": "%s", "code": %d, "built": %d}\n' "$name" "$code" "$(date +%s)" > "$state/app/app.json.tmp"
mv "$state/app/app.json.tmp" "$state/app/app.json"
echo "$code" > "$sign/version"
echo "published seimei $name ($(du -h "$state/app/seimei.apk" | cut -f1))"
