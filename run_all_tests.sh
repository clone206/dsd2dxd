#!/usr/bin/env bash

rm -rf out/*

./test_all_44k_mults.sh
./test_all_48k_mults.sh

./build_test_mono.sh P 24 L 4 test/1kHz_mono_p.dsd
./build_test_stereo.sh P 16 L -4 test/1kHz_stereo_p.dsd
./build_test_stereo_flt.sh P L 0 test/1kHz_stereo_p.dsd

dsd2dxd -R -a -o w -f p -e l -r 88200 -p out . - < test/1kHz_stereo_p.dsd