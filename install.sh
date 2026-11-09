#!/bin/sh
set -e

REPO="erenmentes/zetsu"
BIN_DIR="$HOME/.local/bin"
BIN="$BIN_DIR/zetsu"

case "$(uname -s)" in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) echo "Unsupported system: $(uname -s)"; exit 1 ;;
esac

case "$(uname -m)" in
    x86_64|amd64) arch=x86_64 ;;
    arm64|aarch64) arch=arm64 ;;
    *) echo "Unsupported architecture: $(uname -m)"; exit 1 ;;
esac

if [ "$os" = linux ] && [ "$arch" = arm64 ]; then
    echo "There is no prebuilt binary for Linux arm64 yet. Build from source:"
    echo "  cargo install --git https://github.com/$REPO"
    exit 1
fi

dl() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    else
        wget -qO "$2" "$1"
    fi
}

mkdir -p "$BIN_DIR"
echo "Downloading zetsu..."
dl "https://github.com/$REPO/releases/latest/download/zetsu-$os-$arch" "$BIN.tmp"
chmod +x "$BIN.tmp"
mv "$BIN.tmp" "$BIN"

ICON_URL="https://raw.githubusercontent.com/$REPO/main/assets/zetsu-512.png"

if [ "$os" = linux ]; then
    ICON_DIR="$HOME/.local/share/icons/hicolor/512x512/apps"
    APP_DIR="$HOME/.local/share/applications"
    mkdir -p "$ICON_DIR" "$APP_DIR"
    dl "$ICON_URL" "$ICON_DIR/zetsu.png" || true

    cat > "$APP_DIR/zetsu.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Zetsu IDE
Comment=Minimal terminal IDE
Exec=$BIN %F
Icon=$ICON_DIR/zetsu.png
Terminal=true
Categories=Development;TextEditor;
DESKTOP
    chmod +x "$APP_DIR/zetsu.desktop"
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true

    DESK="$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")"
    if [ -d "$DESK" ]; then
        cp "$APP_DIR/zetsu.desktop" "$DESK/zetsu.desktop"
        chmod +x "$DESK/zetsu.desktop"
        command -v gio >/dev/null 2>&1 && gio set "$DESK/zetsu.desktop" metadata::trusted true 2>/dev/null || true
    fi
fi

if [ "$os" = macos ]; then
    xattr -d com.apple.quarantine "$BIN" 2>/dev/null || true

    APP="$HOME/Applications/Zetsu IDE.app"
    rm -rf "$APP"
    mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

    TERM_APP="Terminal"
    [ -d "/Applications/iTerm.app" ] && TERM_APP="iTerm"
    cat > "$APP/Contents/MacOS/zetsu-launcher" <<LAUNCH
#!/bin/sh
open -a "$TERM_APP" "$BIN"
LAUNCH
    chmod +x "$APP/Contents/MacOS/zetsu-launcher"

    cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Zetsu IDE</string>
    <key>CFBundleDisplayName</key><string>Zetsu IDE</string>
    <key>CFBundleIdentifier</key><string>dev.zetsu.ide</string>
    <key>CFBundleExecutable</key><string>zetsu-launcher</string>
    <key>CFBundleIconFile</key><string>zetsu</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>1</string>
</dict>
</plist>
PLIST

    TMP="$(mktemp -d)"
    if dl "$ICON_URL" "$TMP/icon.png"; then
        SET="$TMP/zetsu.iconset"
        mkdir -p "$SET"
        for s in 16 32 128 256 512; do
            sips -z $s $s "$TMP/icon.png" --out "$SET/icon_${s}x${s}.png" >/dev/null
            d=$((s * 2))
            sips -z $d $d "$TMP/icon.png" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
        done
        iconutil -c icns "$SET" -o "$APP/Contents/Resources/zetsu.icns" || true
    fi
    rm -rf "$TMP"
    touch "$APP"

    if [ -d "$HOME/Desktop" ]; then
        rm -f "$HOME/Desktop/Zetsu IDE.app"
        ln -s "$APP" "$HOME/Desktop/Zetsu IDE.app"
    fi
fi

echo ""
echo "zetsu installed: $BIN"
case ":$PATH:" in
    *":$BIN_DIR:"*) echo "Run it with: zetsu" ;;
    *)
        echo "Add this line to your ~/.bashrc or ~/.zshrc to use the zetsu command:"
        echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
        ;;
esac
