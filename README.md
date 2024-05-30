# dsd2dxd

Based on dsd2pcm by Sebastian Gesemann: [https://code.google.com/archive/p/dsd2pcm/](https://code.google.com/archive/p/dsd2pcm/)

Added shell scripts to build and test with 1kHz test tone files.

Handles planar format as well. Assumes block size (per channel) of 4096 bytes for planar, 1 byte for interleaved.

## Dependencies

- ffmpeg
- ffplay
- g++
- \*nix environment

## C++ program usage

### Compling

`g++ *.c *.cpp -std=c++11 -O3 -o dsd2dxd`

### Running

## Examples

```bash
# See all options
./dsd2dxd -h|--help
# Example of using with an input and output file
./dsd2dxd [options] < infile.dsd > outfile.pcm
# Example of piping output to ffplay (planar format, lsb first)
./dsd2dxd -f P -e L < infile.dsd | ffplay -f s24le -ar 352.8k -ac 2 -i -
# Example of piping output to ffmpeg to save to file
# (Planar, LSB-first, "Not Just Another" dither, 16:1 decimation on dsd64 input file, quantized to 20 bits)
./dsd2dxd -f P -e L -d N -r 16 -b 20 < infile.dsd | ffmpeg -y -f s24le -ar 176.4k -ac 2 -i - -c:a pcm_s24le outfile.wav
# Using dsdextractor with a dsf file as input, 20 bit, 32:1 ratio, njad dither
./dsdextractor input.dsf | ./dsd2dxd -d N -b 20 -r 32 | ffplay -f s24le -ar 88.2k -ac 2 -i -
```

## Testing Examples

```bash
# Compile code; convert and play mono, planar/LSB-first, 24bit, test file
./build_test_mono.sh P 24 L 1kHz_mono_p.dsd

# Compile code; convert and play stereo, planar/LSB-first, 16bit, test file
./build_test_stereo.sh P 16 L 1kHz_stereo_p.dsd

# Compile code; convert and play stereo, planar/LSB-first, 32bit float, test file
./build_test_stereo_flt.sh P L 1kHz_stereo_p.dsd
```

.dsd files found here with `_p` in the names are the equivalent of the corresponding .dsf files with the header metadata stripped off.

See [dsdextractor](https://github.com/clone206/dsdextractor) repo for a tool that can read .dsf audio data to stdout as used in above usage example, for piping to dsd2dxd.

You can also strip off the metadata at the beginning of any dff file in a hex editor, and use it directly with the ./dsd2dxd command, with all default options.

## Options

```
    -h, --help
        shows this help message
    -c, --channels
        Number of channels (default: 2)
    -f, --fmt
        I (interleaved) or P (planar) (DSD stream option) (default: I)
    -b, --bitdepth
        16, 20, 24, or 32 (float) (intel byte order, output option) (default: 24)
    -t, --filttype
        X (XLD filter), D (Original dsd2pcm filter. Only available with 8:1 decimation ratio),
        E (Equiripple. Only available with double rate DSD input), C (Chebyshev. Only available with double rate DSD input)
        (default: X [single rate] or C [double rate])
    -e, --endianness
        Byte order of input. M (MSB first) or L (LSB first) (default: M)
    -s, --bs
        Block size to read/write at a time in bytes, e.g. 4096 (default: 4096)
    -d, --dither
        Which type of dither to use. T (TPDF), N (Not Just Another Dither), or X (no dither) (default: T)
    -r, --ratio
        Decimation ratio. 8, 16, 32, or 64 (to 1) (default: 8. 64 only available with double rate DSD, Chebyshev filter)
    -i, --inrate
        Input DSD data rate. 1 (dsd64) or 2 (dsd128) (default: 1. 2 only available with Decimation ratio of 16, 32, oe 64)
    -o --output
        Output type. S (stdout), or W (wave) (default: S. Note that W outputs to "outfile.wav" in current directory)
```

## Modified original info.txt

```
You downloaded the source code for "dsd2pcm" which is a simple little
"filter" program, that takes a DSD data stream on stdin and converts
it to a PCM stream (either 16, 20 or 24 bits) and writes it to
stdout. The code is split into two modules:

  (1) dsd2pcm

      This is where the decimation magic happens. It's an
      implementation of a FIR lowpass filter
      optimized for DSD inputs. If you feed this converter with
      DSD you get a PCM stream and double precision floating point
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

  g++ *.c *.cpp -std=c++17 -O3 -o dsd2dxd

provided you have GCC installed. That's why I didn't bother writing
any makefiles. :-p
```
