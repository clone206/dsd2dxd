#!/bin/bash

outbits=$2

if [[ $2 -eq 20 ]]
then
    outbits=24
fi

g++ *.c *.cpp -O3 -o dsd2pcm
./dsd2pcm 1 $1 $2 $3 > out.pcm
ffmpeg -y -f s${outbits}le -ar 352.8k -ac 1 -i out.pcm -c:a pcm_s${outbits}le mono.wav
rm -f out.pcm
ffplay mono.wav