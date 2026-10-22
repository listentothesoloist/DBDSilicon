"""Lets Dead by Daylight start on Obelisk's Linux engine when Steam Input cannot initialize there.

Valve's Linux steamclient.so crashes inside ISteamInput::Init under the engine, and Proton's lsteamclient.dll turns
any crash of a Steam call into a crash of the game (a read at 0xbeef). This patch makes that one call report failure
instead, which Steamworks allows: Init() returns false and the game runs without Steam Input.

Usage: python3 -I patch_lsteamclient.py <lsteamclient.dll>...   (keeps <file>.orig; restore it to undo)
"""
import hashlib
import os
import shutil
import struct
import sys

EXPECTED = "6a95953712fbd6f21493ca7698881457b71a4efa443b783b3a9685d8203ff99c"  # Voile v26.3.0-15
PATCHES = [  # (virtual address, original bytes, replacement)
    (0x180007A22, bytes.fromhex("8b0c25efbe0000890c25efbe0000"), b"\x90" * 14),  # steamclient_call: no 0xbeef trap
    (0x18006456F, bytes.fromhex("750a"), b"\x90\x90"),  # SteamInput006_Init: a failed call returns false
]

for path in sys.argv[1:]:
    data = bytearray(open(path, "rb").read())
    if hashlib.sha256(data).hexdigest() != EXPECTED:
        print("skipped, not the expected build:", path)
        continue
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    sections = struct.unpack_from("<H", data, pe + 6)[0]
    optional = struct.unpack_from("<H", data, pe + 20)[0]
    base = struct.unpack_from("<Q", data, pe + 24 + 24)[0]
    table = [struct.unpack_from("<8sIIII", data, pe + 24 + optional + 40 * i) for i in range(sections)]

    def offset(va):
        rva = va - base
        for _, vsize, start, rsize, raw in table:
            if start <= rva < start + max(vsize, rsize):
                return raw + rva - start
        raise SystemExit(f"{va:#x} is outside every section")

    for va, old, new in PATCHES:
        at = offset(va)
        if data[at : at + len(old)] != old:
            raise SystemExit(f"unexpected bytes at {va:#x} in {path}")
        data[at : at + len(old)] = new
    if not os.path.exists(path + ".orig"):
        shutil.copy2(path, path + ".orig")
    open(path, "wb").write(data)
    print("patched:", path)
