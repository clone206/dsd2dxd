# dsd2dxd

Based on dsd2pcm by Sebastian Gesemann.

Added shell scripts to build and test with 1kHz test tone files.

20240102: Only mono working fully. Some artifacts in stereo. Possibly due to block/stride size issue in code.

## Dependencies
- ffmpeg
- ffplay
- g++
- *nix environment

## Examples
```bash
# Compile code; convert and play mono, LSB, planar test file
./build_test_mono.sh L P 1kHz_mono_p.dsd

# Compile code; convert and play stereo, LSB, planar test file
./build_test_stereo.sh L P 1kHz_stereo_p.dsd
```

.dsd files with `_p` in the names are the equivalent of the .dsf files with the header metadata stripped off.

You can also strip off the metadata at the beginning of any dff file in a hex editor, and use it along with the correct input params (M for MSB first, I for interleaved).