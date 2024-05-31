#!/bin/bash

g++ *.c *.cpp -std=c++17 -O3 -o dsd2dxd
./dsd2dxd -f $1 -b 32 -d F -e $2 --volume=$3 < $4 | ffplay -f f32le -ar 352.8k -ac 2 -i -