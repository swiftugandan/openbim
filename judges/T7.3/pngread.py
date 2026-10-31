# Minimal, dependency-free PNG decoder (zlib + numpy) for the T7.3 judge.
# Supports non-interlaced bit depth 8 or 16, colour types 0 (grey), 2 (RGB), 3 (palette, 8-bit),
# 4 (grey+alpha) and 6 (RGBA). Returns an (H, W, 3) uint8 RGB array (alpha is dropped).
import struct
import zlib

import numpy as np

_CHANNELS = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}


def _paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    return b if pb <= pc else c


def read_png(path):
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: not a PNG file")
    pos, idat, palette, hdr = 8, [], None, None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos:pos + 4])
        ctype = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + length]
        pos += 12 + length
        if ctype == b"IHDR":
            hdr = struct.unpack(">IIBBBBB", body)
        elif ctype == b"PLTE":
            palette = np.frombuffer(body, dtype=np.uint8).reshape(-1, 3)
        elif ctype == b"IDAT":
            idat.append(body)
        elif ctype == b"IEND":
            break
    if hdr is None:
        raise ValueError(f"{path}: missing IHDR")
    w, h, depth, color, _comp, _filt, interlace = hdr
    if interlace != 0:
        raise ValueError(f"{path}: interlaced PNG not supported by the judge")
    if color not in _CHANNELS or depth not in (8, 16) or (color == 3 and depth != 8):
        raise ValueError(f"{path}: unsupported PNG colour type {color} / depth {depth}")
    ch = _CHANNELS[color]
    bpp = ch * depth // 8
    stride = w * bpp
    raw = zlib.decompress(b"".join(idat))
    if len(raw) != h * (stride + 1):
        raise ValueError(f"{path}: bad image data length")
    out = bytearray(h * stride)
    prev = bytearray(stride)
    for y in range(h):
        ft = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        if ft == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 0xFF
        elif ft == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif ft == 3:
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 0xFF
        elif ft == 4:
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                c = prev[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + _paeth(a, prev[i], c)) & 0xFF
        elif ft != 0:
            raise ValueError(f"{path}: bad filter type {ft}")
        out[y * stride:(y + 1) * stride] = line
        prev = line
    arr = np.frombuffer(bytes(out), dtype=np.uint8)
    if depth == 16:
        arr = arr.reshape(-1, 2)[:, 0]  # keep the high byte
    arr = arr.reshape(h, w, ch)
    if color == 3:
        if palette is None:
            raise ValueError(f"{path}: palette image without PLTE")
        return palette[arr[:, :, 0]]
    if color in (0, 4):
        return np.repeat(arr[:, :, :1], 3, axis=2)
    return np.ascontiguousarray(arr[:, :, :3])
