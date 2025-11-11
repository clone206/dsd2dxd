#!/usr/bin/env bash

cargo install --path . && dsd2dxd -r 96000  test/1kHz_stereo_p.dsf | ffplay -f s24le -ar 96k -ch_layout stereo -i -
dsd2dxd -r 192000  test/1kHz_stereo_p.dsf   | ffplay -f s24le -ar 192k -ch_layout stereo -i -
dsd2dxd -r 384000  test/1kHz_stereo_p.dsf   | ffplay -f s24le -ar 384k -ch_layout stereo -i -
dsd2dxd -r 96000   test/1kHz_stereo_128.dsf  | ffplay -f s24le -ar 96k -ch_layout stereo -i -
dsd2dxd -r 192000  test/1kHz_stereo_128.dsf | ffplay -f s24le -ar 192k -ch_layout stereo -i -
dsd2dxd -r 384000  test/1kHz_stereo_128.dsf | ffplay -f s24le -ar 384k -ch_layout stereo -i -
dsd2dxd -r 96000   test/1kHz_stereo_256.dsf | ffplay -f s24le -ar 96k -ch_layout stereo -i -
dsd2dxd -r 192000  test/1kHz_stereo_256.dsf | ffplay -f s24le -ar 192k -ch_layout stereo -i -
dsd2dxd -r 384000  test/1kHz_stereo_256.dsf | ffplay -f s24le -ar 384k -ch_layout stereo -i -

dsd2dxd -r 96000   -v test/1kHz_stereo_p.dsf    > /dev/null
dsd2dxd -r 192000  -v test/1kHz_stereo_p.dsf   > /dev/null
dsd2dxd -r 384000  -v test/1kHz_stereo_p.dsf   > /dev/null
dsd2dxd -r 96000   -v test/1kHz_stereo_128.dsf  > /dev/null
dsd2dxd -r 192000  -v test/1kHz_stereo_128.dsf > /dev/null
dsd2dxd -r 384000  -v test/1kHz_stereo_128.dsf > /dev/null
dsd2dxd -r 96000   -v test/1kHz_stereo_256.dsf > /dev/null
dsd2dxd -r 192000  -v test/1kHz_stereo_256.dsf > /dev/null
dsd2dxd -r 384000  -v test/1kHz_stereo_256.dsf > /dev/null