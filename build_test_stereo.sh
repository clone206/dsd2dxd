#!/bin/bash

outbits=$2

if [[ $2 -eq 20 ]]
then
    outbits=24
fi

g++ *.c *.cpp -std=c++11 -O3 -o dsd2dxd
./dsd2dxd -f $1 -b $2 -e $3 < $4 | ffplay -f s${outbits}le -ar 352.8k -ac 2 -i -