#!/bin/sh
# Baut MinimalLibrary-<version>.spk für Synology DSM 7.2 (ein Paket für Intel/AMD und ARM64).
# Aufruf: build-spk.sh <version> <server-x86_64> <server-aarch64> <web-ordner> <ausgabe-ordner>
set -eu
VER="$1"; X64="$2"; ARM="$3"; WEB="$4"
mkdir -p "$5"; OUT="$(cd "$5" && pwd)"
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d)"
mkdir -p "$WORK/pkg/bin" "$WORK/spk" "$OUT"
cp "$X64" "$WORK/pkg/bin/minimal-library-server-x86_64"
cp "$ARM" "$WORK/pkg/bin/minimal-library-server-aarch64"
chmod 755 "$WORK/pkg/bin/"*
cp -r "$WEB" "$WORK/pkg/web"
cp -r "$HERE/ui" "$WORK/pkg/ui"
# Firewall-Ports: Pfad ist relativ zum Paket-Ziel (/var/packages/MinimalLibrary/target), nicht zu conf/
mkdir -p "$WORK/pkg/app"
cp "$HERE/MinimalLibrary.sc" "$WORK/pkg/app/MinimalLibrary.sc"
(cd "$WORK/pkg" && tar czf "$WORK/spk/package.tgz" .)
cp -r "$HERE/scripts" "$HERE/conf" "$HERE/WIZARD_UIFILES" "$WORK/spk/"
cp "$HERE/PACKAGE_ICON.PNG" "$HERE/PACKAGE_ICON_256.PNG" "$WORK/spk/"
cat > "$WORK/spk/INFO" <<INFO
package="MinimalLibrary"
version="$VER-0001"
os_min_ver="7.2-64570"
arch="noarch"
displayname="Minimal Library"
displayname_ger="Minimal Library"
description="Your music library on the NAS: organise, tag, find duplicates, playlists, play on Sonos/DLNA – in the browser. Connects with the Minimal Player Connected app."
description_ger="Deine Musikbibliothek auf dem NAS: sortieren, Tags, Duplikate, Playlisten, Wiedergabe auf Sonos/DLNA – im Browser. Verbindet sich mit der App Minimal Player Connected."
maintainer="SilentApps Minimal Labs"
maintainer_url="https://silentapps-minimal-labs.com"
distributor="SilentApps Minimal Labs"
distributor_url="https://silentapps-minimal-labs.com"
support_url="mailto:apps@silentapps-minimal-labs.com"
helpurl="https://silentapps-minimal-labs.com/app/minimal-library"
dsmuidir="ui"
dsmappname="SYNO.SDS._ThirdParty.App.MinimalLibrary"
adminport="8642"
adminprotocol="http"
adminurl="/"
startable="yes"
thirdparty="yes"
INFO
(cd "$WORK/spk" && tar cf "$OUT/MinimalLibrary-$VER.spk" INFO package.tgz scripts conf WIZARD_UIFILES PACKAGE_ICON.PNG PACKAGE_ICON_256.PNG)
rm -rf "$WORK"
echo "$OUT/MinimalLibrary-$VER.spk"
