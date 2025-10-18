#!/usr/bin/env python3
# Make a planar, LSB-first raw DSD file with a zero-mean toggling baseline.
# Single-channel (mono) with a single "impulse" (broken toggle) at the middle bit.
#
# Convert with:
#   ./target/debug/dsd2dxd -v -f P -e L -c 1 -s 4096 -i 1 -r 32 -b 32 -o W impulse_mono_toggle.dsd

BLOCK_SIZE = 4096                # bytes per channel per frame (match -s)
FRAMES = 1                      # choose so BYTES_PER_CH = BLOCK_SIZE * FRAMES
BYTES_PER_CH = BLOCK_SIZE * FRAMES
OUT = "impulse_mono_toggle.dsd"

def build_channel(bytes_per_ch: int, impulse_bit: int | None) -> bytearray:
    nbits = bytes_per_ch * 8
    out = bytearray(bytes_per_ch)
    prev = 0
    for n in range(nbits):
        # Toggling baseline: b(n) = n & 1
        bit = (n & 1)
        if impulse_bit is not None and n == impulse_bit:
            # Break the toggle: repeat previous bit -> creates a unit impulse
            bit = prev
        # Pack LSB-first
        byte_idx = n >> 3
        bit_idx = n & 7
        if bit:
            out[byte_idx] |= (1 << bit_idx)
        prev = bit
    return out

nbits = BYTES_PER_CH * 8
impulse_at = nbits // 2  # middle

mono = build_channel(BYTES_PER_CH, impulse_at)

with open(OUT, "wb") as f:
    # Write as planar frames: only one channel (mono)
    for off in range(0, BYTES_PER_CH, BLOCK_SIZE):
        end = off + BLOCK_SIZE
        f.write(mono[off:end])

print(f"Wrote {OUT}: mono LSB-first, bytes/ch={BYTES_PER_CH}, block_size={BLOCK_SIZE}, impulse at bit {impulse_at}")