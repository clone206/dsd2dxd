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
  Syntax: dsd2pcm <channels> <bitorder> <bitdepth> <format> <infile>
  channels = 1,2,3,...,9 (number of channels in DSD stream)
  bitorder = L (lsb first), M (msb first) (DSD stream option)
  bitdepth = 16 or 24 (intel byte order, output option)
  format = I (interleaved) or P (planar)
  infile = Input file name, containing raw dsd with either 
  planar format and 4096 byte block size,
  or interleaved with 1 byte per channel.

  Note: At 16 bits/sample a noise shaper kicks in that can preserve
  a dynamic range of 135 dB below 30 kHz.

  Outputs raw pcm to file named "out.pcm".
```

## Testing Examples
```bash
# Compile code; convert to and play mono, LSB-first, 24bit, planar test file
./build_test_mono.sh L 24 P 1kHz_mono_p.dsd

# Compile code; convert to and play stereo, LSB-first, 16bit, planar test file
./build_test_stereo.sh L 16 P 1kHz_stereo_p.dsd
```

.dsd files with `_p` in the names are the equivalent of the .dsf files with the header metadata stripped off.

You can also strip off the metadata at the beginning of any dff file in a hex editor, and use it along with the correct input params (M for MSB first, I for interleaved).

## Modified original info.txt
```
You downloaded the source code for "dsd2pcm" which is a simple little
"filter" program, that takes a DSD data stream on stdin and converts
it to a PCM stream (352.8 kHz, either 16 or 24 bits) and writes it to
out.pcm. The code is split into three modules:

  (1) dsd2pcm

      This is where the 8:1 decimation magic happens. It's an
      implementation of a symmetric 96-taps FIR lowpass filter
      optimized for DSD inputs. If you feed this converter with
      DSD64 you get a PCM stream at 352.8 kHz and floating point
      samples. This module is independent and can be reused. 

  (2) noiseshape

      A module for applying generic noise shaping filters. It's
      used for the 16-bit output mode in "main" to preserve the
      dynamic range. This module is independent and can be reused.

  (3) main.cpp (file contains the main function and handles I/O)

The first two modules are pure C for maximum portability. In addition,
there are C++ wrapper headers for convenient use of these modules in
C++. The main application is a C++ application and makes use of the
C++ headers to access the functionality of the first two modules.


The original code was released unter the simplified BSD license.
See LICENSE.txt for details. After modifications, released under GPL 3 license.
See LICENSE for details.


Under Linux this program is easily compiled by typing

  g++ *.c *.cpp -O3 -o dsd2pcm

provided you have GCC installed. That's why I didn't bother writing
any makefiles. :-p

Cheers!
Sebastian Gesemann
```