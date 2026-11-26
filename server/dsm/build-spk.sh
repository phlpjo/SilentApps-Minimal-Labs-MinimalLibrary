#!/bin/sh
# Baut MinimalLibrary-<version>.spk für Synology DSM 7.2 (ein Paket für Intel/AMD und ARM64).
# Aufruf: build-spk.sh <version> <server-x86_64> <server-aarch64> <web-ordner> <ausgabe-ordner> [stream]
# Mit „stream“ entsteht die schlanke Variante MinimalLibraryStream (nur Streaming fürs Handy, eigene kleine Seite).
set -eu
VER="$1"; X64="$2"; ARM="$3"; WEB="$4"; VARIANT="${6:-full}"
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
cp -r "$HERE/scripts" "$HERE/conf" "$HERE/WIZARD_UIFILES" "$WORK/spk/"
cp "$HERE/PACKAGE_ICON.PNG" "$HERE/PACKAGE_ICON_256.PNG" "$WORK/spk/"
PKG="MinimalLibrary"; NAME="Minimal Library"; PORT="8642"
DESC="Your music library on the NAS: organise, tag, find duplicates, playlists, play on Sonos/DLNA – in the browser. Connects with the Minimal Player Connected app."
DESC_GER="Deine Musikbibliothek auf dem NAS: sortieren, Tags, Duplikate, Playlisten, Wiedergabe auf Sonos/DLNA – im Browser. Verbindet sich mit der App Minimal Player Connected."
if [ "$VARIANT" = "stream" ]; then
  PKG="MinimalLibraryStream"; NAME="Minimal Library Stream"; PORT="8643"
  DESC="Stream your music from the NAS to the Minimal Player Connected app – at home or over VPN. Lightweight: reads your music folders, no analysis, a small page for pairing."
  DESC_GER="Deine Musik vom NAS in die App Minimal Player Connected streamen – zu Hause oder per VPN. Schlank: liest die Musikordner ein, keine Analyse, eine kleine Seite zum Koppeln."
  # Paketname, Port und Modus in allen Paketdateien umstellen
  for f in "$WORK/spk/scripts/"* "$WORK/spk/conf/"* "$WORK/spk/WIZARD_UIFILES/"* "$WORK/pkg/ui/config" "$WORK/pkg/app/MinimalLibrary.sc"; do
    sed -i -e 's/MinimalLibrary/MinimalLibraryStream/g' -e 's/8642/8643/g' -e 's/Minimal Library (Browser)/Minimal Library Stream (Seite)/g' "$f"
  done
  sed -i -e 's/"title": "Minimal Library"/"title": "Minimal Library Stream"/' "$WORK/pkg/ui/config"
  sed -i -e 's/ML_MUSIC="\$MUSIC"/ML_MUSIC="$MUSIC" ML_MODE=stream ML_PORT=8643/' "$WORK/spk/scripts/start-stop-status"
  mv "$WORK/pkg/app/MinimalLibrary.sc" "$WORK/pkg/app/MinimalLibraryStream.sc"
fi
(cd "$WORK/pkg" && tar czf "$WORK/spk/package.tgz" .)
cat > "$WORK/spk/INFO" <<INFO
package="$PKG"
version="$VER-0001"
os_min_ver="7.2-64570"
arch="noarch"
displayname="$NAME"
displayname_ger="$NAME"
description="$DESC"
description_ger="$DESC_GER"
maintainer="SilentApps Minimal Labs"
maintainer_url="https://silentapps-minimal-labs.com"
distributor="SilentApps Minimal Labs"
distributor_url="https://silentapps-minimal-labs.com"
support_url="mailto:apps@silentapps-minimal-labs.com"
helpurl="https://silentapps-minimal-labs.com/app/minimal-library"
dsmuidir="ui"
dsmappname="SYNO.SDS._ThirdParty.App.$PKG"
adminport="$PORT"
adminprotocol="http"
adminurl="/"
startable="yes"
thirdparty="yes"
INFO
(cd "$WORK/spk" && tar cf "$OUT/$PKG-$VER.spk" INFO package.tgz scripts conf WIZARD_UIFILES PACKAGE_ICON.PNG PACKAGE_ICON_256.PNG)
rm -rf "$WORK"
echo "$OUT/$PKG-$VER.spk"
