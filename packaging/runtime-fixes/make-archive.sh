#!/bin/bash
# Packs DBDSilicon's fixes from a fixed Linux engine runtime into runtime-fixes.tar.xz, which the app bundles and
# puts over Voile's runtime before each launch. Usage: make-archive.sh <path to Voile/v26.3.0-15/linux>
set -euo pipefail

runtime=${1:?usage: make-archive.sh <path to the runtime linux folder>}
here=$(cd "$(dirname "$0")" && pwd)
files=(
    engine/fexdarwin-run
    engine/wsd
    graphics/libexec/virgl_render_server
    graphics/lib/libvirglrenderer.1.dylib
    windows/d3d11.dll
    windows/dxgi.dll
    windows/d3d12.dll
    wine/lib/wine/x86_64-unix/nptunix.so
    wine/lib/wine/x86_64-windows/lsteamclient.dll
)
for file in "${files[@]}"; do
    [[ -f "$runtime/$file" ]] || { echo "missing: $runtime/$file" >&2; exit 1; }
done
COPYFILE_DISABLE=1 tar -cJf "$here/runtime-fixes.tar.xz" -C "$runtime" \
    --no-mac-metadata --no-xattrs --uid 0 --gid 0 --uname root --gname wheel "${files[@]}"
(cd "$runtime" && shasum -a 256 "${files[@]}") > "$here/SHA256SUMS"
ls -lh "$here/runtime-fixes.tar.xz"
