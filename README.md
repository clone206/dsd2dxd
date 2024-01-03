# dsd2dxd

Based on dsd2pcm by Sebastian Gesemann.

Added shell scripts to build and test with 1kHz test tone files.

20240102: Only mono working fully. Some artifacts in stereo. Possibly due to block/stride size issue in code.

.dsd files are the .dsf files with the header metadata stripped off.

## Dependencies
- ffmpeg
- ffplay
- g++
- *nix environment