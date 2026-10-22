"""Lets Obelisk's Linux engine show 10-bit swapchains (Unreal Engine 5's default back buffer) in their true colors.

nptunix.so turns a swapchain's virgl format into wsd's: R8G8B8A8 (67) or else B8G8R8A8, so an R10G10B10A2 frame
was shown as 8-bit BGRA. This patch passes the virgl format through unchanged; the rebuilt wsd maps it to Metal
(1, 2: BGRA8, 67: RGBA8, 8: RGB10A2). Use it with that wsd and the rebuilt Neptune DLLs, and revert all three together.

Usage: python3 -I patch_nptunix.py <nptunix.so>   (keeps <file>.orig; restore it to undo)
"""
import hashlib
import os
import shutil
import sys

EXPECTED = "1610899ee26eca7fb9a325f7773282f3f8db4ba99a75a15604876824df67ed01"  # Voile v26.3.0-15
OFFSET = 0x236B  # npt_do_wsi_init: a.format = (p->virgl_format == 67) + 1
ORIGINAL = bytes.fromhex("31ff83782843400f94c783c701")
REPLACEMENT = bytes.fromhex("8b7828") + b"\x90" * 10  # mov edi, [rax+0x28]: the virgl format itself

for path in sys.argv[1:]:
    data = bytearray(open(path, "rb").read())
    if hashlib.sha256(data).hexdigest() != EXPECTED:
        print("skipped, not the expected build:", path)
        continue
    if data[OFFSET : OFFSET + len(ORIGINAL)] != ORIGINAL:
        raise SystemExit(f"unexpected bytes in {path}")
    data[OFFSET : OFFSET + len(ORIGINAL)] = REPLACEMENT
    if not os.path.exists(path + ".orig"):
        shutil.copy2(path, path + ".orig")
    open(path, "wb").write(data)
    print("patched:", path)
