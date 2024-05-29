#!/bin/bash

g++ *.c *.cpp -std=c++11 -O3 -o dsd2dxd
./dsd2dxd -f $1 -b 32 -d X -e $2 < $3 | ffplay -f f32le -ar 352.8k -ac 2 -i -