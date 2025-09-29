#!/usr/bin/env bash

cargo build
./target/debug/dsd2dxd -f $1 -b 32 -d F -e $2 --level=$3 < $4 | ffplay -f f32le -ar 352.8k -ch_layout stereo -i -