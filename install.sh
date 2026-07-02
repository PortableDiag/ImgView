#!/usr/bin/env bash
# Register ImgView as a desktop app + image handler for the current user.
# Builds the binary first if needed. No root required.
#
#   ./install.sh            # install/refresh
#   ./install.sh --default  # also make ImgView the DEFAULT image viewer
#   ./install.sh --uninstall
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APPS_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/256x256/apps"
DESKTOP="$APPS_DIR/imgview.desktop"
ICON="$ICON_DIR/imgview.png"
BIN="$DIR/imgview"

MIMES="image/jpeg;image/png;image/gif;image/bmp;image/webp;image/tiff;image/x-portable-pixmap;image/x-icon;image/avif;"

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$DESKTOP" "$ICON"
    update-desktop-database "$APPS_DIR" 2>/dev/null || true
    echo "Removed ImgView desktop entry + icon."
    exit 0
fi

[[ -x "$BIN" ]] || "$DIR/build.sh"

mkdir -p "$APPS_DIR" "$ICON_DIR"

# Draw the same sky/sun/mountains icon at 256px via ImageMagick if available,
# otherwise fall back to a solid-color placeholder.
if command -v convert >/dev/null 2>&1; then
    convert -size 256x256 xc:'#4a90d9' \
        -fill '#ffd34e' -draw 'circle 70,70 70,110' \
        -fill '#2f7d4f' -draw 'polygon 0,256 110,120 190,256' \
        -fill '#3a9d63' -draw 'polygon 100,256 175,150 256,256' \
        "$ICON"
else
    printf '' > "$ICON"  # placeholder; the in-window icon is generated at runtime
fi
echo "Icon  -> $ICON"

cat > "$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=ImgView
GenericName=Image Viewer
Comment=Lightweight Picasa-style image viewer
Exec="$BIN" %f
Icon=$ICON
Terminal=false
Categories=Graphics;Viewer;Photography;
MimeType=$MIMES
StartupNotify=true
EOF
chmod +x "$DESKTOP"
echo "Entry -> $DESKTOP"

update-desktop-database "$APPS_DIR" 2>/dev/null || true
gtk-update-icon-cache "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

if [[ "${1:-}" == "--default" ]]; then
    IFS=';' read -ra M <<< "$MIMES"
    for m in "${M[@]}"; do
        [[ -n "$m" ]] && xdg-mime default imgview.desktop "$m" 2>/dev/null || true
    done
    echo "Set ImgView as the DEFAULT viewer for images."
fi

echo
echo "Done. Right-click any image -> 'Open With' -> ImgView."
