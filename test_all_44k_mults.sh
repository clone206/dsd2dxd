#!/usr/bin/env bash

cargo install --path . && dsd2dxd -r 88200   -v test/1kHz_stereo_p.dsf | ffplay -f s24le -ar 88200  -ch_layout stereo -i -
dsd2dxd -r 176400  test/1kHz_stereo_p.dsf    | ffplay -f s24le -ar 176400 -ch_layout stereo -i -
dsd2dxd -r 352800  test/1kHz_stereo_p.dsf    | ffplay -f s24le -ar 352800 -ch_layout stereo -i -
dsd2dxd -r 88200   test/1kHz_stereo_128.dsf  | ffplay -f s24le -ar 88200  -ch_layout stereo -i -
dsd2dxd -r 176400  test/1kHz_stereo_128.dsf  | ffplay -f s24le -ar 176400 -ch_layout stereo -i -
dsd2dxd -r 352800  test/1kHz_stereo_128.dsf  | ffplay -f s24le -ar 352800 -ch_layout stereo -i -
dsd2dxd -r 705600  test/1kHz_stereo_128.dsf  | ffplay -f s24le -ar 705600 -ch_layout stereo -i -

dsd2dxd -r 88200   test/1kHz_stereo_256.dsf  | ffplay -f s24le -ar 88200   -ch_layout stereo -i -
dsd2dxd -r 176400  test/1kHz_stereo_256.dsf  | ffplay -f s24le -ar 176400 -ch_layout stereo -i -
dsd2dxd -r 352800  test/1kHz_stereo_256.dsf  | ffplay -f s24le -ar 352800 -ch_layout stereo -i -
dsd2dxd -r 705600  test/1kHz_stereo_256.dsf  | ffplay -f s24le -ar 705600 -ch_layout stereo -i -

dsd2dxd -r 88200   test/1kHz_stereo_p.dsf    > /dev/null
dsd2dxd -r 176400  test/1kHz_stereo_p.dsf    > /dev/null 
dsd2dxd -r 352800  test/1kHz_stereo_p.dsf    > /dev/null
dsd2dxd -r 88200   test/1kHz_stereo_128.dsf  > /dev/null
dsd2dxd -r 176400  test/1kHz_stereo_128.dsf  > /dev/null
dsd2dxd -r 352800  test/1kHz_stereo_128.dsf  > /dev/null
dsd2dxd -r 705600  test/1kHz_stereo_128.dsf  > /dev/null
dsd2dxd -r 88200  test/1kHz_stereo_256.dsf  > /dev/null
dsd2dxd -r 176400  test/1kHz_stereo_256.dsf  > /dev/null
dsd2dxd -r 352800  test/1kHz_stereo_256.dsf  > /dev/null
dsd2dxd -r 705600  test/1kHz_stereo_256.dsf  > /dev/null
dsd2dxd -r 1411200  test/1kHz_stereo_256.dsf  > /dev/null