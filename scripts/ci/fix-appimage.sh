#!/usr/bin/env bash
# Strip the host display-stack libraries out of Tauri's AppImage and repack it.
#
# Usage: scripts/ci/fix-appimage.sh <path/to/app.AppImage>   (rewritten in place)
#
# Why: linuxdeploy (used by Tauri's AppImage bundler) copies the build host's
# libwayland-*, libxkbcommon, libxcb-{randr,render,shm}, libXau and libXdmcp
# into usr/lib. The host's Mesa then loads against those old copies and
# WebKitGTK's web process dies with
#     Could not create default EGL display: EGL_BAD_PARAMETER. Aborting...
# so the window stays blank/white and never finishes loading. Seen in our
# linux-distros CI on fedora:44 (also under X11, and
# WEBKIT_DISABLE_DMABUF_RENDERER=1 does not help); upstream:
# https://github.com/tauri-apps/tauri/issues/15976. The AppImage community
# excludelist already says these must come from the host, and every desktop
# with GTK 3 has them.
#
# The repack reuses the AppImage's own runtime (the bytes before the
# squashfs), so the result starts exactly the same way as Tauri's output.

set -euo pipefail

appimage=$(realpath "$1")
APPIMAGETOOL_URL=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0

EXCLUDE=(
    libwayland-client.so.0
    libwayland-cursor.so.0
    libwayland-egl.so.1
    libwayland-server.so.0
    libxkbcommon.so.0
    libxcb-randr.so.0
    libxcb-render.so.0
    libxcb-shm.so.0
    libXau.so.6
    libXdmcp.so.6
)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"

curl -fsSL -o appimagetool "$APPIMAGETOOL_URL"
echo "$APPIMAGETOOL_SHA256  appimagetool" | sha256sum -c -
chmod +x appimagetool "$appimage"

offset=$(APPIMAGE_EXTRACT_AND_RUN=1 "$appimage" --appimage-offset)
head -c "$offset" "$appimage" >runtime
APPIMAGE_EXTRACT_AND_RUN=1 "$appimage" --appimage-extract >/dev/null

removed=0
for lib in "${EXCLUDE[@]}"; do
    if [ -e "squashfs-root/usr/lib/$lib" ]; then
        rm -f "squashfs-root/usr/lib/$lib"
        echo "removed usr/lib/$lib"
        removed=$((removed + 1))
    fi
done
if [ "$removed" -eq 0 ]; then
    echo "nothing to remove (did linuxdeploy's excludelist change?); leaving the AppImage as is"
    exit 0
fi

ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 ./appimagetool --no-appstream \
    --runtime-file runtime squashfs-root repacked.AppImage
chmod +x repacked.AppImage
mv repacked.AppImage "$appimage"
echo "repacked $appimage ($removed libraries removed)"
