#!/usr/bin/env bash

outbits=$2

if [[ $2 -eq 20 ]]
then
    outbits=24
fi

cargo build
./target/debug/dsd2dxd -c 1 -f $1 -b $2 -e $3 --level=$4 < $5 | ffplay -f s${outbits}le -ar 352.8k -ch_layout mono -i -