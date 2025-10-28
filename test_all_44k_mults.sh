#!/usr/bin/env bash

cargo install --path . && dsd2dxd -r 88200   -v 1kHz_stereo_p.dsf | ffplay -f s24le -ar 88200  -ac: 2 -i -
dsd2dxd -r 176400  1kHz_stereo_p.dsf    | ffplay -f s24le -ar 176400 -ac: 2 -i -
dsd2dxd -r 352800  1kHz_stereo_p.dsf    | ffplay -f s24le -ar 352800 -ac: 2 -i -
dsd2dxd -r 88200   1kHz_stereo_128.dsf  | ffplay -f s24le -ar 88200  -ac: 2 -i -
dsd2dxd -r 176400  1kHz_stereo_128.dsf  | ffplay -f s24le -ar 176400 -ac: 2 -i -
dsd2dxd -r 352800  1kHz_stereo_128.dsf  | ffplay -f s24le -ar 352800 -ac: 2 -i -

dsd2dxd -r 88200  1kHz_stereo_256.dsf  | ffplay -f s24le -ar 88200   -ac: 2 -i -
dsd2dxd -r 176400  1kHz_stereo_256.dsf  | ffplay -f s24le -ar 176400 -ac: 2 -i -
dsd2dxd -r 352800  1kHz_stereo_256.dsf  | ffplay -f s24le -ar 352800 -ac: 2 -i -

dsd2dxd -r 88200   1kHz_stereo_p.dsf    > /dev/null
dsd2dxd -r 176400  1kHz_stereo_p.dsf    > /dev/null 
dsd2dxd -r 352800  1kHz_stereo_p.dsf    > /dev/null
dsd2dxd -r 88200   1kHz_stereo_128.dsf  > /dev/null
dsd2dxd -r 176400  1kHz_stereo_128.dsf  > /dev/null
dsd2dxd -r 352800  1kHz_stereo_128.dsf  > /dev/null

dsd2dxd -r 88200  1kHz_stereo_256.dsf  > /dev/null
dsd2dxd -r 176400  1kHz_stereo_256.dsf  > /dev/null
dsd2dxd -r 352800  1kHz_stereo_256.dsf  > /dev/null