#!/usr/bin/env bash
# Repacks an AppImage without some of the libraries linuxdeploy bundled, so the
# system's own copies are used instead.
#
# libxkbcommon.so.0: the bundled copy comes from the build machine (Ubuntu
# 22.04, xkbcommon 1.4) and can't read the keyboard layouts that current Wayland
# desktops send (KDE Plasma on Arch / CachyOS). winit then has no keymap and
# panics on the first key press. Every desktop that runs GTK has its own copy.
#
# libwayland-*: the bundled 1.20 lacks functions (wl_display_create_queue_with_name,
# 1.23) that Arch / CachyOS Mesa needs and binds at load (-z now), so the
# system's EGL and Vulkan drivers fail to load next to it: the login window's
# web process aborts ("Could not create default EGL display") and stays blank.
# GTK depends on these on every distro, so the system always has them.
#
# usage: appimage-drop-libs.sh <file.AppImage> <library>...
set -euo pipefail

app=$(realpath "$1")
shift
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# an AppImage is the runtime (an ELF) followed by a SquashFS image of the app
offset=$("$app" --appimage-offset)
head -c "$offset" "$app" > "$work/runtime"
(cd "$work" && "$app" --appimage-extract > /dev/null)
root="$work/squashfs-root"

for lib in "$@"; do
  if [ ! -e "$root/usr/lib/$lib" ]; then
    echo "$lib is not bundled in $app" >&2
    exit 1
  fi
  rm "$root/usr/lib/$lib"
  echo "dropped $lib"
done

# what's left must still load, now partly from the system
deps=$(ldd "$root/usr/bin/fastbet")
if grep "not found" <<< "$deps"; then
  exit 1
fi
for lib in "$@"; do
  grep "$lib" <<< "$deps" || true
done

mksquashfs "$root" "$work/image" -root-owned -noappend -comp zstd -no-progress > /dev/null
cat "$work/runtime" "$work/image" > "$app"
chmod +x "$app"

# the repacked file must still open as an AppImage, with the app inside
mkdir "$work/check"
(cd "$work/check" && "$app" --appimage-extract > /dev/null)
test -x "$work/check/squashfs-root/usr/bin/fastbet"
for lib in "$@"; do
  if [ -e "$work/check/squashfs-root/usr/lib/$lib" ]; then
    echo "$lib is still in $app" >&2
    exit 1
  fi
done
echo "repacked $app"
