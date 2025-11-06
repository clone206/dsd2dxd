# dsd2dxd

Converts DSD to PCM on the command line with the following features:
- Accepts single rate (dsd64), double rate (dsd128), or quad rate (dsd256) DSD as input.
  - .dsf and .dff files can be read from, including metadata.
- Can output to an aiff, wav, or flac file.
  - Where possible, ID3v2 tags are copied to the destination files (when read from .dsf or .dff file that has them).
- Can also read raw DSD bitstreams from standard in (stdin) and output raw PCM to standard out (stdout), so you can use piping/shell redirection to combine with other audio utilities on the command line.
  - Handles either planar format DSD (as found in .dsf files), or interleaved format DSD (as found in .dff files). Assumes block size (per channel) of 4096 bytes for planar, 1 byte for interleaved, unless otherwise specified with the below command line options.
- Allows you to specify the type of dither to use on output
- Output bit depth can be either 16, 20, or 24 fixed integer PCM, or 32 bit float PCM.
  - The dither will be optimized accordingly, including for 20 bit output.
- Allows you to choose between different decimation filters.

## Build Dependencies

- [Rust/Cargo](https://rust-lang.org/tools/install/)
  - On *nix systems like MacOS & Linux, this can be as easy as running the following on the command line: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`. Feel free to accept the defaults when the installer prompts you. Keep an eye out for the prompt to reload your environment in the terminal. This will allow you to update your path with your new rust user directory.
- `ffmpeg` (Optional)
  - Only needed for a simple playback mechanism, such as when running the test scripts or below usage examples, or for converting/compressing the output of dsd2dxd (e.g. to an apple lossless file, a format that dsd2dxd doesn't yet support.)
  - With apt on linux: `sudo apt install ffmpeg`
  - With homebrew `brew install ffmpeg`

## Program usage

### Compling and Installing

At the root of the cloned repository:

```
git submodule update --init
cargo install --path .
```

This should install dsd2dxd into a directory that was automatically added to your `$PATH` when you installed Rust/Cargo. **NOTE:** If you installed an older version of dsd2dxd previously, make sure to remove it so it doesn't take precedence over this new installation (e.g. `sudo rm -f /usr/local/bin/dsd2dxd`). When in doubt on a *nix system, type `which dsd2dxd` to see which binary is currently being used.

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
dsd2dxd -f P -e L < 1kHz_stereo_p.dsd | ffplay -f s24le -ar 352.8k -ch_layout stereo -i -
# Example of piping output to ffmpeg to save to an apple lossless file
# (Planar, LSB-first, "Not Just Another" dither, 176.4K output from 
# dsd64 input file)
dsd2dxd -f P -e L -d N -r 176400 < 1kHz_stereo_p.dsd | ffmpeg -y -f s24le -ar 176.4k -ch_layout stereo -i - -c:a alac outfile.m4a
# Generalized example of using with an input and output file,
# via stdin/stdout
dsd2dxd [options] < infile.dsd > outfile.pcm
# Convert all files ending in .dsf or .DSF in all subdirectories 
# of current directory, to 24 bit flac files, with 88.2K output.
# If using a bash shell, you can ensure the below is recursive by running
# "shopt -s globstar" first. Outputs to the directory specified with -p, 
# instead of the default of outputting to the same directory as each input file.
dsd2dxd -r 88200 -o f -p ../some/other/directory **/*.{dsf,DSF}
# Advanced: Convert all .dsf, .dff, and .dsd files in the current directory, 
# recursively, as well as a raw dsd file via stdin, to wav files, and place
# the converted files into the ./test directory. The ./test directory must
# already exist, but any needed subdirectories will be created within. 
# The result of converting from stdin (-) will be a file named output.wav.
# You can also mix globs (wildcards) like the ones above with directories
# and stdin as inputs. The shell will just expand the globs to a list of 
# files, and dsd2dxd will also expand the directories to a list of files.
dsd2dxd -R -o w -p test . - < raw_dsd_file
```

### Full Usage and Options

For many users, the majority of the below options can usually be ignored, as you will probably mostly be reading from .dsf or .dff files, which contain metadata that is read by dsd2dxd and used to set a lot of the options automatically. For that use case, the most important options are probably `-o`, `-r`, and `-p`, for setting the output type, output sample rate, and output folder path, respectively.

```
Usage: dsd2dxd [options] [infile/folder(s)|-], where "-" means read from 
standard in (stdin).

If reading from a file, certain command line options you provide 
(e.g. block size) may be overridden using the metadata found in 
that file (either a dsf or dff file). If neither file/folder path(s) 
or - is provided, standard in is assumed. 
Multiple file/folder paths can be provided and the input-related options 
specified will be applied to each, except where overridden by each 
file's metadata. When -R/--recurse is supplied, folders will be recursively
scanned for files ending in .dsf, .dff, or .dsd, where .dsd files are assumed
to be raw dsd bitstreams. Without -R, directories are not traversed; provide
explicit file paths if you don’t want recursion.

Options:
  -p, --path <PATH>
          Output directory path for converted PCM files.
          Directory must already exist but any subdirectories
          will be created as needed. Artwork files will be copied to 
          the output directories. [default: same as input file]
  -c, --channels <CHANNELS>
          Number of channels [default: 2]
  -f, --fmt <FORMAT>
          DSD data format: Interleaved (I) or Planar (P)
          [default: I]
  -b, --bitdepth <BIT_DEPTH>
          Output bit depth: 16, 20, 24 (fixed integer), or 32
          (float) [default: 24]
  -t, --filttype <FILTER_TYPE>
          Filter type: E (Equiripple), X (XLD. Only available
          with DSD64 input and 88200, 176400, and 352800
          outputs), D (Original dsd2pcm. Only available with
          DSD64 input and 352800 output), C (Chebyshev. Only
          available with DSD128 input and 88200, 176400, and
          352800 outputs) [default: E]
  -e, --endianness <ENDIANNESS>
          DSD data endianness: M (most significant bit
          first), or L (least significant bit first)
          [default: M]
  -s, --bs <BLOCK_SIZE>
          DSD block size in bytes. Only set this if you know
          what you're doing [default: 4096]
  -d, --dither <DITHER_TYPE>
          Dither type: T (TPDF), R (rectangular), F (float),
          X (none) [default: F for 32 bit, T otherwise]
  -r, --rate <OUTPUT_RATE>
          Output sample rate in Hz. Can be 88200, 96000,
          176400, 192000, 352800, 384000. Note that
          conversion to multiples of 44100 are faster than
          48000 multiples [default: 352800]
  -i, --inrate <INPUT_RATE>
          Input DSD rate: 1 (DSD64), 2 (DSD128), or 4
          (DSD256) [default: 1]
  -o, --output <OUTPUT>
          Output type: S (stdout), A (aif), W (wave), F
          (flac) Note that W, A, or F outputs to either
          <basename>.[wav|aif|flac] in current directory,
          where <basename> is the input filename without the
          extension, or output.[wav|aif|flac] (if reading
          from stdin.) [default: S]
  -l, --level <LEVEL>
          Volume level adjustment in dB. Can be negative with
          the long form, e.g. --level=-3 [default: 0.0]
  -v, --verbose
          Print diagnostic messages
  -a, --append
          Append abbreviated output rate to filename (e.g.,
          _96K, _88_2K). Also appends " [<OUTPUT_RATE>]" to
          the album tag of the output file if present
  -R, --recurse
          Recurse into directories when the supplied input paths include folders
  -h, --help
          Print help
  -V, --version
          Print version
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

Many of dsd2dxd's decimation filters were created from scratch using extensive listening tests. This tool aims to have audiophile-worthy conversion quality while also being useful in a recording engineering context, where converting between dsd and dxd may be necessary. Some of the filters for dsd64 were copied over from XLD, and the original dsd2pcm filter is an option as well.

For a best all-round sound that works in the most contexts (e.g. a smartphone, wireless earbuds, audiophile DAC with speakers, etc), I recommend the following for each of the supported DSD rates, always using the default equiripple filters:

```
DSD64  -> 96kHz    (-r 96000)
DSD128 -> 176.4kHz (-r 176400)
DSD256 -> 192kHz   (-r 192000)
```

Converting to uneven factors (96kHz multiples) is nothing to be afraid of with dsd2dxd. The conversions may run a little more slowly, but that's in part due to the fact that no shortcuts were taken. All of the filtering operations happen at 64 bit float, just like when converting to 88.2kHz multiples. And in some ways the 96kHz multiples are better because they use cascaded FIR filters, each with a gentle EQ curve, rather than one filter with a more severe curve (smaller fraction of the pre-decimated sample rates).

For a natural sound with slight rolloff, try switching to the chebyshev filters when using dsd128 (the default is normally equiripple). For a slightly more "airy" sound when using dsd128, stay with the equiripple filters, especially if going to 176.4 kHz.

For dsd64, if you like the sound of XLD then feel free to use those filters here (not the default), but personally I think the XLD filter for 88.2kHz output is not great and should possibly be avoided depending on the source material. Better to go to 176.4 kHz when sticking with the XLD filter, if the output will later be resampled to 88.2kHz or below, or if playing back the 176.4k file on a NOS DAC. If either of those are false, you may encounter harshness on playback as the 176.4kHz file is delta sigma modulated by the DAC. Note that unlike the actual XLD app, you can apply dither with dsd2dxd, even when using the XLD filters.

There are a few dither options, including the Airwindows [Dither Float](https://www.airwindows.com/ditherfloat/), which is for use when outputting to 32 bit float. dsd2dxd uses double precision calculations internally so technically outputting to 32 bit float represents a loss of precision, hence the Dither Float option.

You can also turn the dither off completely if that's your thing.

## Acknowledgements

Based on dsd2pcm by Sebastian Gesemann: [https://code.google.com/archive/p/dsd2pcm/](https://code.google.com/archive/p/dsd2pcm/). Added many enhancements over the original dsd2pcm as detailed above.

Also influenced by/borrowed from [dsf2flac](https://github.com/hank/dsf2flac), [XLD](https://tmkk.undo.jp/xld/index_e.html), and [Airwindows](https://www.airwindows.com).

## Contributing

Contributions from experienced  developers welcome! Just keep the code clean and try to follow the formatting patterns already in place (e.g. space indentation, avoiding overly long lines of code.) I personally use vscode, which includes a formatter. Using the same formatter may save some headaches.

Make sure to do some testing, including with the included test scripts and test tone DSD files.

If you'd like to create new filters for dsd2dxd, you'll need to make sure they have an even number of taps (odd filter order). The filter coefficients stored in the code only include the 2nd half of the taps of each symmetric decimation filter. Thus far the approach taken in the filter design has been to prefer gradual rolloffs and to allow small amounts of aliasing. This author doesn't put much stock in the importance of ultrasonic frequencies for enjoyable sound reproduction.

In summary, I've tried to keep things as flat as possible out to 20kHz-22kHz, gradually rolling off after that, with the transition band edging slightly past the Nyquist frequency, and keeping the number of taps to a minimum.

For general info on contributing see [https://docs.github.com/en/get-started/exploring-projects-on-github/contributing-to-a-project](https://docs.github.com/en/get-started/exploring-projects-on-github/contributing-to-a-project). Basically, fork this repository, create a feature branch from main, and submit a pull request.
