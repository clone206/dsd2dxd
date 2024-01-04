#!/bin/sh

g++ *.c *.cpp -O3 -o dsd2pcm
./dsd2pcm 2 $1 $2 $3 $4
ffmpeg -y -f s${2}le -ar 352.8k -ac 2 -i out.pcm -c:a pcm_s${2}le stereo.wav
rm -f out.pcm
ffplay stereo.wav