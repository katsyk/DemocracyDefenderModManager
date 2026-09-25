#!/usr/bin/env bash
# Smoke-test DDMM's Linux release artifacts inside a distro container.
#
# Run by .github/workflows/linux-distros.yml as:
#   docker run --privileged ... <image> bash scripts/ci/linux-smoke.sh <family> <mode> <dist-dir> <out-dir>
#
#   family  fedora | arch | debian | suse   (debian covers Ubuntu too)
#   mode    portable  -- AppImage (with/without FUSE) + tar.gz binary
#           package   -- install the .rpm/.deb with the package manager in a
#                        fresh container, then launch the installed binary
#
# Every check appends one row to <out-dir>/results.tsv:
#   <check>  PASS|FAIL|INFO  <detail>
# FAIL means a check that has to pass for the documented install path to be
# true; INFO rows record expected-to-fail baselines (e.g. AppImage before
# installing FUSE 2), so the report shows the exact error a user would see.
# The script exits non-zero if any FAIL row was written.
#
# "Started" means the frontend logged "Initialization complete." -- i.e. the
# binary loaded, GTK/WebKit came up, the webview rendered the SvelteKit app,
# and it found the (fake) Helldivers 2 install through the Steam library scan.

set -uo pipefail

family=$1
mode=$2
dist=$(realpath "$3")
out=$(realpath -m "$4")
mkdir -p "$out"
results="$out/results.tsv"
: >"$results"

LAUNCH_TIMEOUT=${LAUNCH_TIMEOUT:-90}
failed=0

log() { printf '\n=== %s ===\n' "$*"; }

record() { # check status detail
    printf '%s\t%s\t%s\n' "$1" "$2" "${3//$'\n'/ | }" >>"$results"
    printf '[%s] %s %s\n' "$2" "$1" "$3"
    [ "$2" = FAIL ] && failed=1
    return 0
}

# ---------------------------------------------------------------------------
# Package manager helpers
# ---------------------------------------------------------------------------

pm_refreshed=0
pm_install() {
    case $family in
        fedora) dnf -y install "$@" ;;
        arch)
            if [ $pm_refreshed = 0 ]; then pacman -Syu --noconfirm && pm_refreshed=1; fi
            pacman -S --noconfirm --needed "$@" ;;
        debian)
            if [ $pm_refreshed = 0 ]; then apt-get update && pm_refreshed=1; fi
            DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$@" ;;
        suse) zypper --non-interactive install --no-recommends "$@" ;;
    esac
}

pm_install_local() { # path to .rpm / .deb
    case $family in
        fedora) dnf -y install "$1" ;;
        debian)
            if [ $pm_refreshed = 0 ]; then apt-get update && pm_refreshed=1; fi
            DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$1" ;;
        suse) zypper --non-interactive install --no-recommends --allow-unsigned-rpm "$1" ;;
    esac
}

# Test harness only (virtual X server, session bus, process tools) plus the
# "any desktop already has this" baseline: Mesa and a font. Deliberately NOT
# WebKitGTK/GTK -- those are the documented runtime deps under test.
test_tools() {
    case $family in
        fedora) echo xorg-x11-server-Xvfb dbus-daemon dbus-tools procps-ng util-linux findutils file mesa-dri-drivers dejavu-sans-fonts fuse3 ;;
        arch)   echo xorg-server-xvfb dbus procps-ng util-linux findutils file mesa ttf-dejavu fuse3 ;;
        debian) echo xvfb dbus procps util-linux findutils file libgl1-mesa-dri fonts-dejavu-core fuse3 ;;
        suse)   echo xorg-x11-server-Xvfb dbus-1 procps util-linux findutils file Mesa-dri dejavu-fonts fuse3 gzip tar ;;
    esac
}

# The documented runtime dependencies (docs/getting-started/linux.md) --
# keep these two lists in sync.
runtime_deps() {
    case $family in
        fedora) echo webkit2gtk4.1 gtk3 ;;
        arch)   echo webkit2gtk-4.1 gtk3 ;;
        debian) echo libwebkit2gtk-4.1-0 ;;
        suse)   echo libwebkit2gtk-4_1-0 ;;
    esac
}

# The documented "make the AppImage mount" fix: FUSE 2.
fuse2_deps() {
    case $family in
        fedora) echo fuse fuse-libs ;;
        arch)   echo fuse2 ;;
        debian)
            if apt-cache show libfuse2t64 >/dev/null 2>&1; then echo libfuse2t64; else echo libfuse2; fi ;;
        suse)   echo fuse libfuse2 ;;
    esac
}

# ---------------------------------------------------------------------------
# Launch harness
# ---------------------------------------------------------------------------

start_xvfb() {
    Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >"$out/xvfb.log" 2>&1 &
    for _ in $(seq 1 20); do
        [ -e /tmp/.X11-unix/X99 ] && return 0
        sleep 0.5
    done
    echo "Xvfb did not come up"; cat "$out/xvfb.log"; exit 2
}

# A fresh HOME with a fake Steam library containing a fake Helldivers 2
# install that passes DDMM's detection (steam.rs: appmanifest installdir +
# tools/, data/, bin/helldivers2.exe).
make_fake_home() {
    local home=$1
    local steam="$home/.local/share/Steam"
    local game="$steam/steamapps/common/Helldivers 2"
    mkdir -p "$game/tools" "$game/data" "$game/bin"
    : >"$game/bin/helldivers2.exe"
    cat >"$steam/steamapps/appmanifest_553850.acf" <<'EOF'
"AppState"
{
	"appid"		"553850"
	"name"		"HELLDIVERS 2"
	"installdir"		"Helldivers 2"
}
EOF
}

# launch <check> <required|info> <env...> -- <command...>
# Starts the command in its own session with a fake HOME, waits for
# "Initialization complete." in DDMM's log, then kills the whole session.
launch() {
    local check=$1 level=$2; shift 2
    local envs=()
    while [ "$1" != -- ]; do envs+=("$1"); shift; done
    shift

    local home rt dir
    home=$(mktemp -d /tmp/home-XXXXXX)
    rt=$(mktemp -d /tmp/xdg-XXXXXX); chmod 700 "$rt"
    dir="$out/$check"; mkdir -p "$dir"
    make_fake_home "$home"

    log "launch $check: $*"
    (cd "$home" && exec setsid env HOME="$home" XDG_RUNTIME_DIR="$rt" DISPLAY=:99 \
        NO_AT_BRIDGE=1 "${envs[@]}" dbus-run-session -- "$@") >"$dir/stdout.txt" 2>&1 &
    local pid=$! started=0 waited=0
    local logs="$home/.local/share/io.github.katsyk.ddmm/logs"

    while [ $waited -lt "$LAUNCH_TIMEOUT" ]; do
        if grep -qs "Initialization complete." "$logs"/*.log; then started=1; break; fi
        kill -0 $pid 2>/dev/null || break
        sleep 1; waited=$((waited + 1))
    done

    local alive=0
    kill -0 $pid 2>/dev/null && alive=1
    kill -TERM -- -$pid 2>/dev/null; sleep 2; kill -KILL -- -$pid 2>/dev/null
    wait $pid 2>/dev/null
    cp -r "$logs" "$dir/ddmm-logs" 2>/dev/null

    local detail
    if [ $started = 1 ]; then
        detail="Initialization complete. after ${waited}s"
        [ ${#envs[@]} -gt 0 ] && detail="$detail (env: ${envs[*]})"
        record "$check" PASS "$detail"
    else
        if [ $alive = 1 ]; then detail="still running after ${LAUNCH_TIMEOUT}s without 'Initialization complete.'"
        else detail="exited early"; fi
        detail="$detail; $(grep -iE 'error|not found|cannot|failed|fuse|egl|panic|abort' "$dir/stdout.txt" \
            | grep -v 'Initialization' | head -n 6 | cut -c1-240)"
        [ ${#envs[@]} -gt 0 ] && detail="(env: ${envs[*]}) $detail"
        if [ "$level" = required ]; then record "$check" FAIL "$detail"; else record "$check" INFO "$detail"; fi
    fi
    [ $started = 1 ]
}

# Launch, and if it fails, retry once with the known WebKitGTK rendering
# workaround so the report says whether that is the fix.
launch_with_fallback() {
    local check=$1 level=$2; shift 2
    if ! launch "$check" "$level" -- "$@"; then
        launch "$check+dmabuf-off" info WEBKIT_DISABLE_DMABUF_RENDERER=1 -- "$@" || true
    fi
}

# ldd_check <check> <required|info> <binary>
ldd_check() {
    local check=$1 level=$2 bin=$3
    local report="$out/$check.ldd.txt"
    if command -v ldd >/dev/null; then ldd "$bin" >"$report" 2>&1
    else LD_TRACE_LOADED_OBJECTS=1 "$bin" >"$report" 2>&1; fi
    local missing
    missing=$(grep 'not found' "$report" | awk '{print $1}' | tr '\n' ' ')
    if [ -z "$missing" ]; then
        record "$check" PASS "all $(grep -c '=>' "$report") shared libraries resolve"
    elif [ "$level" = required ]; then
        record "$check" FAIL "not found: $missing"
    else
        record "$check" INFO "not found: $missing"
    fi
}

# ---------------------------------------------------------------------------

grep -E '^(PRETTY_NAME|VERSION_ID)=' /etc/os-release | tee "$out/os-release.txt"

log "install test harness"
# shellcheck disable=SC2046
pm_install $(test_tools) >"$out/install-test-tools.txt" 2>&1 \
    || { tail -n 30 "$out/install-test-tools.txt"; echo "harness install failed"; exit 2; }
start_xvfb

appimage=$(find "$dist" -name 'DDMM-*-linux-x86_64.AppImage' | head -n1)
tarball=$(find "$dist" -name 'DDMM-*-linux-x64.tar.gz' | head -n1)
deb=$(find "$dist" -name 'DDMM-*-linux-amd64.deb' | head -n1)
rpm=$(find "$dist" -name 'DDMM-*-linux-x86_64.rpm' | head -n1)

case $mode in
portable)
    # --- AppImage, before installing anything app-specific ------------------
    cp "$appimage" /tmp/ddmm.AppImage && chmod +x /tmp/ddmm.AppImage
    launch_with_fallback appimage-fuse-no-fuse2 info /tmp/ddmm.AppImage
    launch_with_fallback appimage-extract-and-run required /tmp/ddmm.AppImage --appimage-extract-and-run

    log "install FUSE 2: $(fuse2_deps)"
    # shellcheck disable=SC2046
    if pm_install $(fuse2_deps) >"$out/install-fuse2.txt" 2>&1; then
        launch_with_fallback appimage-fuse-with-fuse2 required /tmp/ddmm.AppImage
    else
        record appimage-fuse-with-fuse2 FAIL "installing $(fuse2_deps) failed: $(tail -n 3 "$out/install-fuse2.txt")"
    fi

    # --- tar.gz binary --------------------------------------------------------
    mkdir -p /tmp/tar && tar -C /tmp/tar -xzf "$tarball"
    ldd_check tar-ldd-before-deps info /tmp/tar/ddmm
    launch tar-launch-before-deps info -- /tmp/tar/ddmm || true

    log "install documented runtime deps: $(runtime_deps)"
    # shellcheck disable=SC2046
    if pm_install $(runtime_deps) >"$out/install-runtime-deps.txt" 2>&1; then
        ldd_check tar-ldd required /tmp/tar/ddmm
        launch_with_fallback tar-launch required /tmp/tar/ddmm
    else
        record tar-launch FAIL "installing $(runtime_deps) failed: $(tail -n 3 "$out/install-runtime-deps.txt")"
    fi
    ;;
package)
    case $family in
        fedora|suse) pkg=$rpm ;;
        debian) pkg=$deb ;;
        *) echo "no native package for $family"; exit 2 ;;
    esac
    log "install $(basename "$pkg") with the package manager (fresh container)"
    if pm_install_local "$pkg" >"$out/install-package.txt" 2>&1; then
        record package-install PASS "$(basename "$pkg")"
        bin=$(command -v ddmm || echo /usr/bin/ddmm)
        ldd_check package-ldd required "$bin"
        desktop=$(grep -l '^Exec=ddmm' /usr/share/applications/*.desktop 2>/dev/null | head -n1)
        if [ -n "$desktop" ]; then record package-desktop-entry PASS "$desktop"
        else record package-desktop-entry FAIL "no /usr/share/applications/*.desktop with Exec=ddmm"; fi
        launch_with_fallback package-launch required "$bin"
    else
        record package-install FAIL "$(tail -n 8 "$out/install-package.txt")"
    fi
    ;;
esac

log "results"
column -t -s $'\t' "$results" 2>/dev/null || cat "$results"
exit $failed
