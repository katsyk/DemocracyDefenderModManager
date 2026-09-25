---
title: Installing on Linux
---

# Installing on Linux

DDMM ships four Linux downloads, all for 64-bit x86 PCs (`x86_64` / `amd64`). They all contain the same app. The
only difference is how it gets onto your system and what it expects your system to already have.

Every command on this page is tested automatically on each change to DDMM's packaging, inside clean Fedora, Arch,
Ubuntu, Debian and openSUSE systems (see [How this is tested](#how-this-is-tested)).

## Which file do I pick?

| Your distribution | Pick | Why |
| --- | --- | --- |
| Fedora, Nobara, openSUSE | **`.rpm`** | Your package manager installs everything DDMM needs, and adds a menu entry. |
| Ubuntu, Debian, Linux Mint, Pop!_OS, Zorin, elementary | **`.deb`** | Same, for `apt`-based systems. |
| Arch, CachyOS, EndeavourOS, Manjaro | **`.tar.gz`** (plus one package) | There's no Arch package yet (an AUR package may come later). |
| Anything else, or you don't want to install anything system-wide | **AppImage** | Carries WebKitGTK inside it and runs from anywhere, but relies on your desktop's graphics and font libraries. |

When in doubt, use the `.rpm`, `.deb` or `.tar.gz`. They use your distribution's own WebKitGTK, which is built for
your graphics drivers. The AppImage is the fallback.

The files on the [Releases page](https://github.com/katsyk/DemocracyDefenderModManager/releases) are named:

- `DDMM-<version>-linux-x86_64.rpm`
- `DDMM-<version>-linux-amd64.deb`
- `DDMM-<version>-linux-x86_64.AppImage`
- `DDMM-<version>-linux-x64.tar.gz`

DDMM needs glibc 2.35 or newer and WebKitGTK 4.1. That covers Ubuntu 22.04, Debian 12 and anything more recent,
and any current Fedora, Arch or openSUSE Tumbleweed.

## .rpm (Fedora, openSUSE)

=== "Fedora / RHEL-like"

    ```sh
    sudo dnf install ./DDMM-*-linux-x86_64.rpm
    ```

=== "openSUSE"

    ```sh
    sudo zypper install --allow-unsigned-rpm ./DDMM-*-linux-x86_64.rpm
    ```

The `./` matters: it tells the package manager to install the file you downloaded, and to fetch WebKitGTK and GTK
from your distribution's repositories if they're missing. Then start **Democracy Defender Mod Manager** from your
app menu, or run `ddmm` in a terminal.

The package isn't signed yet, which is why openSUSE needs `--allow-unsigned-rpm`. To remove it later:
`sudo dnf remove democracy-defender-mod-manager` (or `sudo zypper remove democracy-defender-mod-manager`).

On an immutable Fedora (Silverblue, Kinoite, Bazzite, Aurora), the AppImage is the simplest choice.
`rpm-ostree install ./DDMM-*-linux-x86_64.rpm` followed by a reboot should also work, but the automated tests
don't cover it.

## .deb (Debian, Ubuntu, Mint)

```sh
sudo apt install ./DDMM-*-linux-amd64.deb
```

Then start **Democracy Defender Mod Manager** from your app menu, or run `ddmm` in a terminal. To remove it:
`sudo apt remove democracy-defender-mod-manager`.

## .tar.gz (Arch, CachyOS, or anything else)

The tarball is just the `ddmm` program and a quick-start text file. It uses your system's WebKitGTK, so install
that first:

=== "Arch / CachyOS / Manjaro"

    ```sh
    sudo pacman -S --needed webkit2gtk-4.1
    ```

=== "Fedora"

    ```sh
    sudo dnf install webkit2gtk4.1
    ```

=== "Debian / Ubuntu"

    ```sh
    sudo apt install libwebkit2gtk-4.1-0
    ```

=== "openSUSE"

    ```sh
    sudo zypper install libwebkit2gtk-4_1-0
    ```

Each of these pulls in GTK 3 and everything else DDMM links against. Then extract and run it:

```sh
mkdir -p ~/Applications/DDMM
tar -xzf DDMM-*-linux-x64.tar.gz -C ~/Applications/DDMM
~/Applications/DDMM/ddmm
```

If it says `error while loading shared libraries: libwebkit2gtk-4.1.so.0` (or `libgdk-3.so.0`,
`libsoup-3.0.so.0`, ...), the package above isn't installed.

## AppImage

```sh
chmod +x DDMM-*-linux-x86_64.AppImage
./DDMM-*-linux-x86_64.AppImage
```

The AppImage carries WebKitGTK and most of GTK inside it. Like every AppImage, it deliberately does *not* carry
your graphics driver libraries (EGL, OpenGL ES, GBM), and it uses your system's fontconfig, HarfBuzz, FriBidi,
Wayland and X11 libraries. Bundled copies of those are exactly what breaks on newer systems. Any desktop with
GTK 3 and Mesa already has all of them. On a very minimal install (only a window manager, no desktop
environment), an error like `error while loading shared libraries: libfontconfig.so.1` (or
`libwayland-server.so.0`), or `Couldn't open libGLESv2.so.2`, means one is missing. Install them with:

=== "Arch / CachyOS"

    ```sh
    sudo pacman -S --needed gtk3 mesa libglvnd
    ```

=== "Fedora"

    ```sh
    sudo dnf install gtk3 mesa-libEGL libglvnd-gles mesa-libgbm libwayland-server
    ```

=== "Debian / Ubuntu"

    ```sh
    sudo apt install libgtk-3-0 libegl1 libgles2 libgbm1 libwayland-server0
    ```

=== "openSUSE"

    ```sh
    sudo zypper install libgtk-3-0 Mesa-libEGL1 Mesa-libGLESv2-2 libgbm1 libwayland-server0
    ```

Those are the packages the automated tests install before running the AppImage.

Releases up to and including 2.0.0-rc.5 bundled old Wayland/X11 client libraries in the AppImage. On current
Mesa (Fedora 44, Arch) that makes the window stay blank while the terminal shows
`Could not create default EGL display: EGL_BAD_PARAMETER. Aborting...`. Later releases fix this. For rc.5 itself,
use the `.tar.gz` instead (or the `.rpm` from a later release).

### FUSE

An AppImage normally mounts itself with FUSE. DDMM's AppImage uses the current AppImage runtime, which only needs
the `fusermount3` helper from the `fuse3` package (installed by default on Fedora, Ubuntu and most desktops).
You do **not** need the old `fuse` / `fuse2` / `libfuse2` package that older guides mention.

If FUSE isn't available at all, the AppImage prints `Error: No suitable fusermount binary found on the $PATH` and
then unpacks itself to a temporary folder and runs from there instead. That works, it just starts a little slower.
To skip the FUSE attempt entirely, run it with:

```sh
./DDMM-*-linux-x86_64.AppImage --appimage-extract-and-run
```

To install FUSE anyway: `sudo dnf install fuse3` (Fedora), `sudo pacman -S fuse3` (Arch),
`sudo apt install fuse3` (Debian/Ubuntu), `sudo zypper install fuse3` (openSUSE).

## Blank or white window

If DDMM opens but the window stays white, grey or black, or the terminal shows `EGL` / `DMABUF` / `GBM` errors,
that's a WebKitGTK graphics issue, most often with NVIDIA's driver or under Wayland. Start DDMM with the DMA-BUF
renderer turned off:

```sh
WEBKIT_DISABLE_DMABUF_RENDERER=1 ddmm                                   # .rpm / .deb
WEBKIT_DISABLE_DMABUF_RENDERER=1 ./DDMM-*-linux-x86_64.AppImage         # AppImage
WEBKIT_DISABLE_DMABUF_RENDERER=1 ~/Applications/DDMM/ddmm               # tar.gz
```

If that doesn't help, also try `WEBKIT_DISABLE_COMPOSITING_MODE=1`, or forcing X11 with `GDK_BACKEND=x11`. When
one works, add it to the `Exec=` line of your menu entry, or put it in a small launcher script.

## Steam installed as a Flatpak

DDMM finds Helldivers 2 in the Flatpak Steam location
(`~/.var/app/com.valvesoftware.Steam/.local/share/Steam`) as well as the usual `~/.steam/steam` and
`~/.local/share/Steam`, including extra Steam libraries on other drives. Run DDMM itself natively (from the
`.rpm`, `.deb`, tar.gz or AppImage), not inside the Steam Flatpak.

If your game lives on a drive that Flatpak Steam was given access to but DDMM doesn't detect it, set the folder
yourself in **Settings**. It's the one that contains `bin/`, `data/` and `tools/`. See
[First-time setup](setup.md).

## Seeing errors: run it from a terminal

If DDMM doesn't start, or starts and closes, run it from a terminal. The terminal shows the same output as the
[log file](../help/logs.md), plus any error from the system before DDMM's own logging starts (a missing library,
for example):

```sh
ddmm                                   # installed from .rpm / .deb
./DDMM-*-linux-x86_64.AppImage         # AppImage
~/Applications/DDMM/ddmm               # tar.gz, wherever you extracted it
```

When [reporting a bug](../help/bugs.md), include that output together with:

```sh
cat /etc/os-release
echo "$XDG_SESSION_TYPE"
lspci -k | grep -EA3 'VGA|3D|Display'
```

These show your distribution, whether you're on Wayland or X11, and your GPU and driver. For the tar.gz, also
include the output of `ldd ~/Applications/DDMM/ddmm | grep 'not found'`.

## How this is tested

The `Linux distros` workflow in DDMM's repository (`.github/workflows/linux-distros.yml`) builds all four
downloads. For each distribution (Fedora 44, Arch, Ubuntu 22.04 and 24.04, Debian 12 and 13, openSUSE
Tumbleweed), it starts a clean container and installs only the packages listed on this page. It then checks that
every library resolves and launches DDMM on a virtual display until the app reports it has finished starting. The
`.rpm` and `.deb` are installed with `dnf` / `zypper` / `apt` in a fresh container that has nothing else installed.
