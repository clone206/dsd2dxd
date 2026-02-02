#! /usr/bin/env python3

import argparse

from filter_lib import *

parser = argparse.ArgumentParser(description="Design an equiripple FIR filter and emit Rust taps")
parser.add_argument("--fs", type=float, required=True, help="Sampling rate in Hz")
parser.add_argument("--fsb", type=float, required=True, help="Stopband edge in Hz")
parser.add_argument("--fpb", type=float, required=True, help="Passband edge in Hz")
args = parser.parse_args()

Fs      = args.fs
Fpb     = args.fpb
Fsb     = args.fsb
Apb     = 0.01
Asb     = 144

N = fir_find_optimal_N(Fs, Fpb, Fsb, Apb, Asb)

(h, w, H, Rpb, Rsb, Hpb_min, Hpb_max, Hsb_max) =  fir_calc_filter(Fs, Fpb, Fsb, Apb, Asb, N)

ht_len = len(h)//2
print(f'pub const HTAPS_EQ: [f64; {ht_len}] = [')
print(*h[ht_len:], sep=',\n')
print('];')
print(f'Filter order: {N}, Filter half-length: {ht_len}')

plt.figure(figsize=(10,5))
plt.subplot(111)
plot_freq_response(w, H, Fs, Fpb, Fsb, Hpb_min, Hpb_max, Hsb_max)
plt.tight_layout()
plt.show()

