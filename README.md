# dsd2dxd

Converts DSD to PCM on the command line.

Based on dsd2pcm by Sebastian Gesemann: [https://code.google.com/archive/p/dsd2pcm/](https://code.google.com/archive/p/dsd2pcm/). Also influenced by/borrowed from [dsf2flac](https://github.com/hank/dsf2flac), [XLD](https://tmkk.undo.jp/xld/index_e.html), and [Airwindows](https://www.airwindows.com).

Added many enhancements over the original dsd2pcm, and shell scripts to build and test with 1kHz test tone files. 32 bit float output is now also an option, as well as dsd128 input. And aside from outputting to standard out, you can also output to an aiff, wav, or flac file. Where possible, ID3v2 tags are copied to the destination files.

`dsd2dxd` handles either planar format DSD (as found in .dsf files), or interleaved format DSD (as found in .dff files). Assumes block size (per channel) of 4096 bytes for planar, 1 byte for interleaved.

## Dependencies

- \*nix environment
- `g++`
- [taglib 2.0.1](https://github.com/taglib/taglib)
  - Note that many distros have dated versions of the taglib dev package, so you may need to follow the install instructions at the above repo and use cmake/make to build and install taglib 2.0.1 from source.
    - If using homebrew, such as on MacOS, it may be as simple as running `brew install taglib --HEAD`. It is also possible to run homebrew in linux.
- `flac`
  - This can be installed with homebrew as well, e.g. `brew install flac`
- `ffmpeg` (only needed for a simple playback mechanism, such as when running the test scripts or below usage examples, or for compressing the output of `dsd2dxd`, e.g. to flac)

## C++ program usage

### Compling

`g++ *.c *.cpp -std=c++17 -O3 -o dsd2dxd $(pkg-config --libs --cflags taglib flac++)`

### Installing

`sudo install dsd2dxd /usr/local/bin/`

You can specify any directory you like as the last argument in the above install command. For example, instead of `/usr/local/bin/` you could use `/usr/bin/`. As long as the directory is in your `$PATH` it will work.

### Running

## Examples

```bash
# See options and usage
dsd2dxd -h|--help
# Read from dsf file, printing extra info to stderr (-l for "loud mode").
# Outputs to 1kHz_stereo_p.wav
dsd2dxd -o w -l 1kHz_stereo_p.dsf
# Process all .dsf files in current directory, saving to aiff files
dsd2dxd -o a *.dsf
# Quick and dirty way to process all dff and dsf files in current
# directory, saving to wav files
dsd2dxd -o w *.d?f
# Example of reading raw dsd (planar format, lsb first) into stdin,
# piping output to ffplay
dsd2dxd -f P -e L < 1kHz_stereo_p.dsd | ffplay -f s24le -ar 352.8k -ac 2 -i -
# Example of piping output to ffmpeg to save to a flac file
# (Planar, LSB-first, "Not Just Another" dither, 16:1 decimation on dsd64 input file, quantized to 20 bits)
dsd2dxd -f P -e L -d N -r 16 -b 20 < 1kHz_stereo_p.dsd | ffmpeg -y -f s24le -ar 176.4k -ac 2 -i - -c:a flac outfile.flac
# Generalized example of using with an input and output file,
# via stdin/stdout
dsd2dxd [options] < infile.dsd > outfile.pcm
```

## Testing Examples

```bash
# Compile code; convert and play mono, planar/LSB-first, 24bit, test file w 4dB boost
./build_test_mono.sh P 24 L 4 1kHz_mono_p.dsd

# Compile code; convert and play stereo, planar/LSB-first, 16bit, test file w 4dB cut
./build_test_stereo.sh P 16 L -4 1kHz_stereo_p.dsd

# Compile code; convert and play stereo, planar/LSB-first, 32bit float, test file with no volume adj
./build_test_stereo_flt.sh P L 0 1kHz_stereo_p.dsd
```

.dsd files found here with `_p` in the names are the equivalent of the corresponding .dsf files with the header metadata stripped off. This means they have a block size of `4096` and are planar format.

## More Info

The decimation filters for dsd128 were created from scratch using extensive listening tests. This tool aims to have audiophile-worthy conversion quality while also being useful in a recording engineering context, where converting between dsd and dxd may be necessary. Some of the filters for dsd64 were copied over from XLD, and the original dsd2pcm filter is an option as well.

For a natural sound with slight rolloff but good time-domain performance, try the chebyshev filters when using dsd128 (this is the default). For a more "correct" sound (with respect to the frequency domain) when using dsd128, try the equiripple filters, especially if going to 176.4 kHz (32:1 decimation).

For dsd64, if you like the sound of XLD then feel free to use those filters here (default for dsd64), but personally I think the XLD filter for 88.2kHz output (32:1 decimation) is not great and should possibly be avoided depending on the source material. Better to go to 176.4 kHz (16:1 decimation) when using the XLD filter.

There are also a few dither options, including the Airwindows "Not Just Another Dither", and "Dither Float". The former is not truly random and uses weighting based on Benford Real Numbers, and the latter is for use when outputting to 32 bit float. `dsd2dxd` uses double precision calculations internally so technically outputting to 32 bit float represents a loss of precision, hence the Dither Float option.

## Full Usage and Options

```
dsd2dxd filter (DSD --> PCM).
Reads from stdin or file and writes to stdout or file in a *nix environment.

Usage: dsd2dxd [options] [infile(s)|-], where - means read from stdin

If reading from a file, certain command line options you provide (e.g. block size) may be overridden
using the metadata found in that file (either a dsf or dff file).
If neither filename(s) or - is provided, stdin is assumed.
Multiple filenames can be provided and the input-related options specified will be applied to each,
except where overridden by each file's metadata.
    -h, --help
        shows this help message
    -c, --channels
        Number of channels (default: 2)
    -f, --fmt
        I (interleaved) or P (planar) (DSD stream option) (default: I)
    -b, --bitdepth
        16, 20, 24, or 32 (float) (intel byte order, output option) (default: 24)
    -t, --filttype
        X (XLD filter), D (Original dsd2pcm filter. Only
        available with 8:1 decimation ratio),
        E (Equiripple. Only
        available with double rate DSD input), C (Chebyshev. Only available with double rate DSD input)
        (default: X [single rate] or C [double rate])
    -e, --endianness
        Byte order of input. M (MSB first) or L (LSB first) (default: M)
    -s, --bs
        Block size to read/write at a time in bytes, e.g. 4096 (default: 4096)
    -d, --dither
        Which type of dither to use. T (TPDF), N (Not Just Another Dither), F (floating
        point dither), or X (no dither) (default: F for 32 bit, T otherwise)
    -r, --ratio
        Decimation ratio. 8, 16, 32, or 64 (to 1) (default: 8. 64 only available with
        double rate DSD, Chebyshev filter)
    -i, --inrate
        Input DSD data rate. 1 (dsd64) or 2 (dsd128) (default: 1. 2 only available with
        Decimation ratio of 16, 32, or 64)
    -o, --output
        Output type. S (stdout), A (aif), W (wave), or F (flac) (default: S. Note that W or A outputs to either
        <basename>.[wav|aif] in current directory, where <basename> is the input filename
        without the extension, or outfile.[wav|aif] if reading from stdin.)
    -v, --volume
        Volume adjustment in dB. If a negative number is needed use the --volume=
        format. (default: 0).
    -l, --loud
        Print diagnostic messages to stderr
```

## Modified original info.txt

```
You downloaded the source code for "dsd2pcm" which is a simple little
"filter" program, that takes a DSD data stream and converts
it to a PCM stream (either 16, 20, or 24 fixed bits; or 32 float bits) and writes it to
stdout or a file. The code is split into two modules:

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

  g++ *.c *.cpp -std=c++17 -O3 -o dsd2dxd $(pkg-config --libs --cflags taglib)

provided you have GCC installed. That's why I didn't bother writing
any makefiles. :-p
```
