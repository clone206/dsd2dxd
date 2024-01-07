# dsd2dxd

Based on dsd2pcm by Sebastian Gesemann: [https://code.google.com/archive/p/dsd2pcm/](https://code.google.com/archive/p/dsd2pcm/)

Added shell scripts to build and test with 1kHz test tone files.

Handles planar format as well. Assumes block size (per channel) of 4096 bytes for planar, 1 byte for interleaved.

## Dependencies
- ffmpeg
- ffplay
- g++
- *nix environment

## C++ program usage
### Compling
`g++ *.c *.cpp -O3 -o dsd2pcm`
### Running
```
  Syntax: dsd2pcm <channels> <format> <bitdepth> <filter> <infile>
  channels = 1,2,3,...,9 (number of channels in DSD stream)
  format = I (interleaved) or P (planar)
  bitdepth = 16, 20, or 24 (intel byte order, output option)
  filter = X (XLD filter) or D (Original dsd2pcm filter)
  infile = Input file name, containing raw dsd with either 
  planar format and 4096 byte block size,
  or interleaved with 1 byte per channel.

  Outputs raw pcm to stdout (only supports *nix environment).
```

## Testing Examples
```bash
# Compile code; convert and play mono, planar/LSB-first, 24bit, test file,
# using XLD FIR Filter
./build_test_mono.sh P 24 X 1kHz_mono_p.dsd

# Compile code; convert and play stereo, planar/LSB-first, 16bit, test file,
# using dsd2pcm FIR filter
./build_test_stereo.sh P 16 D 1kHz_stereo_p.dsd
```

.dsd files with `_p` in the names are the equivalent of the .dsf files with the header metadata stripped off.

You can also strip off the metadata at the beginning of any dff file in a hex editor, and use it along with the correct input param (I for interleaved).

## Modified original info.txt
```
You downloaded the source code for "dsd2pcm" which is a simple little
"filter" program, that takes a DSD data stream on stdin and converts
it to a PCM stream (352.8 kHz, either 16 or 24 bits) and writes it to
stdout. The code is split into two modules:

  (1) dsd2pcm

      This is where the 8:1 decimation magic happens. It's an
      implementation of a symmetric 96-taps FIR lowpass filter
      optimized for DSD inputs. If you feed this converter with
      DSD64 you get a PCM stream at 352.8 kHz and floating point
      samples. This module is independent and can be reused. 

  (2) main.cpp (file contains the main function and handles I/O)

The first module is pure C for maximum portability. In addition,
there is a C++ wrapper header for convenient use of this module in
C++. The main application is a C++ application and makes use of the
C++ header to access the functionality of the first module.


The original code was released unter the simplified BSD license.
See LICENSE.txt for details. After modifications, released under GPL 3 license.
See LICENSE for details.


Under Linux this program is easily compiled by typing

  g++ *.c *.cpp -O3 -o dsd2pcm

provided you have GCC installed. That's why I didn't bother writing
any makefiles. :-p
```
