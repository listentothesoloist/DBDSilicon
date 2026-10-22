# Runtime fixes

Dead by Daylight does not start, or draws wrong colors, on the Linux engine of the Voile release DBDSilicon pins
([v26.3.0-15](https://github.com/Zagrios/voile/releases/tag/v26.3.0-15)). `runtime-fixes.tar.xz` holds the files that
fix it, laid out as the runtime's `linux` folder. The app bundles the archive, and before each launch it puts the
files over the runtime, once per archive (see `crates/game-runtime/src/wine/linux_engine/fixes.rs`). Each launch also
gives the prefix the fixed `lsteamclient.dll`, and writes the guest's `resolv.conf` with the DNS servers that answer
a query within 0.7 s: the Mac's own first, then Cloudflare, Google and Quad9, fastest first. Voile's default,
Cloudflare alone, cannot be reached on some networks and VPNs. When none answers, all of them are kept.

`SHA256SUMS` lists each file's hash. `make-archive.sh` packs the archive again from a fixed runtime.

## The files and their source

| File | What it fixes | Source |
| --- | --- | --- |
| `engine/fexdarwin-run` | Unreal Engine 5 starts more than 300 threads, the kernel emulation's limit, and its audio then fails. Also cheaper `/proc/<pid>/status` reads, a back-off for threads spinning on `sched_yield`, and FEX's JIT caches kept whole. | [voile-lkd](https://github.com/Zagrios/voile-lkd) `32d5345`, plus commit `993f18f` on branch `dbd-thread-limit` |
| `engine/wsd` | Shows 10-bit swapchains, Unreal Engine 5's default, in their true colors. | Same commit, `wsd/wsd.m` |
| `windows/d3d11.dll`, `dxgi.dll`, `d3d12.dll` | `OMSetRenderTargetsAndUnorderedAccessViews` with `D3D11_KEEP_*` or NULL arrays, which made the game report "Out of video memory". The 10-bit format, and fewer round trips to the render server. | Neptune `3d42ee2`, plus commit `6e819d0` on branch `dbd-keep-rtv` |
| `graphics/lib/libvirglrenderer.1.dylib`, `graphics/libexec/virgl_render_server` | Counts query versions at `End`, for the guest's query fast path. | Same commit, `virglrenderer/src/neptune/npt_feedback.c` |
| `wine/lib/wine/x86_64-unix/nptunix.so` | Passes the swapchain's format through to wsd instead of forcing 8-bit. | Voile's build, patched by `patch_nptunix.py` |
| `wine/lib/wine/x86_64-windows/lsteamclient.dll` | Valve's Linux Steam client crashes in `ISteamInput::Init` under the kernel emulation. The call now reports failure, as Steamworks allows, and the game runs without Steam Input. | Voile's build of Proton's lsteamclient, patched by `patch_lsteamclient.py` |

The binaries are signed ad hoc, as Voile's are.

## When Voile changes

The fixes fit Voile v26.3.0-15 only. Before DBDSilicon pins a newer release, rebuild each file against it, or drop
the ones the release already fixes, then run `make-archive.sh` on the fixed runtime.
