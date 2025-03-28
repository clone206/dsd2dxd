# dsd2dxd

Converts DSD to PCM on the command line with the following features:
- Accepts single rate (dsd64), or double rate (dsd128) DSD as input.
  - .dsf and .dff files can be read from, including metadata.
- Can output to an aiff, wav, or flac file.
  - Where possible, ID3v2 tags are copied to the destination files (when read from .dsf or .dff file).
- Can also read raw DSD bitstreams from standard in (stdin) and output raw PCM to standard out (stdout), so you can use piping/shell redirection to combine with other audio utilities on the command line.
  - Handles either planar format DSD (as found in .dsf files), or interleaved format DSD (as found in .dff files). Assumes block size (per channel) of 4096 bytes for planar, 1 byte for interleaved, unless otherwise specified with the below command line options.
- Allows you to specify the type of dither to use on output
- Output bit depth can be either 16, 20, or 24 fixed integer PCM, or 32 bit float PCM.
  - The dither will be optimized accordingly, including for 20 bit output.
- Allows you to choose between different decimation filters.

## Dependencies

- \*nix environment (linux, MacOS, etc.) with `g++` and `pkg-config` installed.
- [taglib 2.0.1](https://github.com/taglib/taglib)
  - Note that many *nix distros have dated versions of the taglib dev package, so you may need to follow the [install instructions](https://github.com/taglib/taglib/blob/master/INSTALL.md) from the above repo and use cmake/make to build and install taglib 2.0.1 from source.
  - If using [homebrew](https://brew.sh/), such as on MacOS, it should be as simple as running `brew install taglib --HEAD`. It is also possible to install homebrew on linux.
- `flac`
  - With apt on linux: `sudo apt install libflac++-dev`
  - With homebrew: `brew install flac`
- `ffmpeg` (Optional)
  - Only needed for a simple playback mechanism, such as when running the test scripts or below usage examples, or for converting/compressing the output of dsd2dxd (e.g. to an apple lossless file, a format that dsd2dxd doesn't yet support.)
  - With apt on linux: `sudo apt install ffmpeg`
  - With homebrew `brew install ffmpeg`

## C++ program usage

### Compling

`g++ *.c *.cpp -std=c++17 -O3 -o dsd2dxd $(pkg-config --libs --cflags taglib flac++)`

### Installing

`sudo install dsd2dxd /usr/local/bin/`

You can specify any directory you like as the last argument in the above install command. For example, instead of `/usr/local/bin/` you could use `/usr/bin/`. As long as the directory is in your `$PATH` it will work.

### Examples

```bash
# See options and usage ("|" means or)
dsd2dxd -h|--help
# Read from dsf file, printing extra info to stderr (-v for "verbose mode").
# Outputs to 1kHz_stereo_p.wav
dsd2dxd -o w -v 1kHz_stereo_p.dsf
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
# Recursively convert all files ending in .dsf or .DSF in the current
# directory and subdirectories, to 24 bit flac files, using the equiripple filter
# where the input files are dsd128 (falling back to the default filter for 
# dsd64), with 32:1 decimation.
dsd2dxd -t E -r 32 -b 24 -o f ./{*,**/*}.{dsf,DSF}
```

### Full Usage and Options

For many users, the majority of the below options can usually be ignored, as you will probably mostly be reading from .dsf or .dff files, which contain metadata that is read by dsd2dxd and used to set a lot of the options automatically. For that use case, the most important options are probably `-o`, `-r`, and `-l`, for setting the output type, decimation ratio, and level adjustment, respectively.


```
Usage: dsd2dxd [options] [infile(s)|-], where - means read from stdin

If reading from a file, certain command line options you provide (e.g. block size) may be overridden 
using the metadata found in that file (either a dsf or dff file).
If neither filename(s) or - is provided, standard in is assumed.
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
        available with double rate DSD input),
        C (Chebyshev. Only available with double rate DSD input)
        (default: X [single rate] or C [double rate])
    -e, --endianness
        Byte order of input. M (MSB first) or L (LSB first) (default: M)
    -s, --bs
        Block size to read/write at a time in bytes, e.g. 4096 (default: 4096)
    -d, --dither
        Which type of dither to use. T (TPDF), R (rectangular), N (Not Just Another
        Dither), F (floating point dither), or X (no dither) (default: F for 32 bit, T otherwise)
    -r, --ratio
        Decimation ratio. 8, 16, 32, or 64 (to 1) (default: 8. 64 only available with 
        double rate DSD, Chebyshev filter)
    -i, --inrate
        Input DSD data rate. 1 (dsd64) or 2 (dsd128) (default: 1. 2 only available with 
        Decimation ratio of 16, 32, or 64)
    -o, --output
        Output type. S (stdout), A (aif), W (wave), or F (flac)
        (default: S. Note that W, A, or F outputs to either 
        <basename>.[wav|aif|flac] in current directory,
        where <basename> is the input filename 
        without the extension, or outfile.[wav|aif|flac] if reading from stdin.)
    -l, --level
        Volume level adjustment in dB. If a negative number is needed use the --level= 
        format (with no space after the "="). (default: 0).
    -v, --verbose
        Print diagnostic messages to standard error while converting.
```

## Testing Examples

dsd2dxd includes shell scripts to compile and test with 1kHz test tone files. 

```bash
# Compile code; convert and play mono, planar/LSB-first, 24bit, test file w 4dB boost
./build_test_mono.sh P 24 L 4 1kHz_mono_p.dsd

# Compile code; convert and play stereo, planar/LSB-first, 16bit, test file w 4dB cut
./build_test_stereo.sh P 16 L -4 1kHz_stereo_p.dsd

# Compile code; convert and play stereo, planar/LSB-first, 32bit float, test file with no volume adj
./build_test_stereo_flt.sh P L 0 1kHz_stereo_p.dsd
```

.dsd files found here with `_p` in the names are the equivalent of the corresponding .dsf files with the header metadata stripped off. This means they have a block size of `4096` and are planar format.

## Tips & More Info

The decimation filters for dsd128 were created from scratch using extensive listening tests. This tool aims to have audiophile-worthy conversion quality while also being useful in a recording engineering context, where converting between dsd and dxd may be necessary. Some of the filters for dsd64 were copied over from XLD, and the original dsd2pcm filter is an option as well.

For a natural sound with slight rolloff, try the chebyshev filters when using dsd128 (the default when inputting dsd128). For a slightly more "airy" sound when using dsd128, try the equiripple filters, especially if going to 176.4 kHz (32:1 decimation).

For dsd64, if you like the sound of XLD then feel free to use those filters here (default for dsd64), but personally I think the XLD filter for 88.2kHz output (32:1 decimation) is not great and should possibly be avoided depending on the source material. Better to go to 176.4 kHz (16:1 decimation) when using the XLD filter, if the final format will be 88.2kHz or below, or if playing back on a NOS DAC. If either of those are false, you may encounter harshness on playback as the 176.4kHz file is delta sigma modulated by the DAC. In that case, you may be better off settling with 32:1 decimation until I can create a new filter for that ratio. Unlike the actual XLD app, you can apply dither with dsd2dxd, even when using the XLD filters.

There are a few dither options, including the Airwindows [Not Just Another Dither](https://www.airwindows.com/not-just-another-dither/), and [Dither Float](https://www.airwindows.com/ditherfloat/). The former is not truly random and uses weighting based on Benford Real Numbers, and the latter is for use when outputting to 32 bit float. dsd2dxd uses double precision calculations internally so technically outputting to 32 bit float represents a loss of precision, hence the Dither Float option.

You can also turn the dither off completely if that's your thing.

### Notes on decimation ratios

Using the `-r|--ratio` option, you set the effective output sample rate. The below tables show the output sample rate for each of dsd2dxd's allowed input rates and decimation ratios.

#### DSD 64 (Single rate/2.8 mHz)
```
-r|--ratio  Output Rate
-------------------------
8           352.8 kHz
16          176.4 kHz
32           88.2 kHz
```

#### DSD 128 (Double rate/5.6 mHz)
```
-r|--ratio  Output Rate
-------------------------
16          352.8 kHz
32          176.4 kHz
64           88.2 kHz

```

## Acknowledgements

Based on dsd2pcm by Sebastian Gesemann: [https://code.google.com/archive/p/dsd2pcm/](https://code.google.com/archive/p/dsd2pcm/). Added many enhancements over the original dsd2pcm as detailed above.

Also influenced by/borrowed from [dsf2flac](https://github.com/hank/dsf2flac), [XLD](https://tmkk.undo.jp/xld/index_e.html), and [Airwindows](https://www.airwindows.com).

Commandline options parsing uses [argagg](https://github.com/vietjtnguyen/argagg). Wav and aiff output uses [AudioFile](https://github.com/adamstark/AudioFile). .dsf and .dff file metadata reading is done via a modified version of [dsdunpack](https://github.com/michaelburton/dsdunpack).

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

  g++ *.c *.cpp -std=c++17 -O3 -o dsd2dxd $(pkg-config --libs --cflags taglib flac++)

provided you have GCC installed. That's why I didn't bother writing
any makefiles. :-p
```
## Contributing

Contributions from experienced  developers welcome! Just keep the code clean and try to follow the formatting patterns already in place (e.g. space indentation, avoiding overly long lines of code.) I personally use the Microsoft C/C++ extensions in vscode, which include a formatter. Using the same formatter may save some headaches.

Make sure to do some testing, including with the included test scripts and test tone DSD files.

If you'd like to create new filters for dsd2dxd, you'll need to make sure they're symmetric. The filter coefficients stored in the dsd2pcm C header only include the 2nd half of the taps of each symmetric decimation filter. Thus far the approach taken in the filter design has been to prefer gradual rolloffs and to allow small amounts of aliasing. This author doesn't put much stock in the importance of ultrasonic frequencies for enjoyable sound reproduction.

In summary, I've tried to keep things as flat as possible out to 20kHz, gradually rolling off after that, with the transition band edging slightly past the Nyquist frequency, and keeping the number of taps to a minimum.

For general info on contributing see [https://docs.github.com/en/get-started/exploring-projects-on-github/contributing-to-a-project](https://docs.github.com/en/get-started/exploring-projects-on-github/contributing-to-a-project). Basically, fork this repository, create a feature branch from main, and submit a pull request.