#!/bin/sh

g++ *.c *.cpp -O3 -o dsd2pcm
./dsd2pcm 1 $1 $2 $3 $4
ffmpeg -y -f s${2}le -ar 352.8k -ac 1 -i out.pcm -c:a pcm_s${2}le mono.wav
rm -f out.pcm
ffplay mono.wav