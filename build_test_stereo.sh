#!/bin/sh

g++ *.c *.cpp -O3 -o dsd2pcm
./dsd2pcm 2 $1 24 $2 $3
ffmpeg -y -f s24le -ar 352.8k -ac 2 -i out.pcm -c:a pcm_s32le stereo.wav
rm -f out.pcm
ffplay stereo.wav