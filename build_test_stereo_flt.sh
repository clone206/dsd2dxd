#!/bin/bash

g++ *.c *.cpp -std=c++17 -O2 -o dsd2dxd $(pkg-config --libs --cflags taglib flac++)
./dsd2dxd -f $1 -b 32 -d F -e $2 --level=$3 < $4 | ffplay -f f32le -ar 352.8k -ac 2 -i -