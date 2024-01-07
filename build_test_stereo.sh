#!/bin/bash

outbits=$2

if [[ $2 -eq 20 ]]
then
    outbits=24
fi

g++ *.c *.cpp -O3 -o dsd2pcm
./dsd2pcm 2 $1 $2 $3 < $4 > out.pcm
ffmpeg -y -f s${outbits}le -ar 352.8k -ac 2 -i out.pcm -c:a pcm_s${outbits}le stereo.wav
rm -f out.pcm
ffplay stereo.wav