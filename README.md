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
`g++ *.c *.cpp -std=c++11 -O3 -o dsd2dxd`
### Running
```bash
# See all options
./dsd2dxd -h|--help
# Example of using with an input and output file
./dsd2dxd [options] < infile.dsd > outfile.pcm
# Example of piping output to ffplay (planar format, lsb first)
./dsd2dxd -f P -e L < infile.dsd | ffplay -f s24le -ar 352.8k -ac 2 -i -
```

## Testing Examples
```bash
# Compile code; convert and play mono, planar/LSB-first, 24bit, test file
./build_test_mono.sh P 24 L 1kHz_mono_p.dsd

# Compile code; convert and play stereo, planar/LSB-first, 16bit, test file
./build_test_stereo.sh P 16 L 1kHz_stereo_p.dsd
```

.dsd files found here with `_p` in the names are the equivalent of the corresponding .dsf files with the header metadata stripped off.

You can also strip off the metadata at the beginning of any dff file in a hex editor, and use it directly with the ./dsd2dxd command, with all default options.

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

  g++ *.c *.cpp -std=c++11 -O3 -o dsd2dxd

provided you have GCC installed. That's why I didn't bother writing
any makefiles. :-p
```
