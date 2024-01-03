#!/bin/sh

g++ *.c *.cpp -O3 -o dsd2pcm
./dsd2pcm 2 L 24 1000hz.dsd
ffmpeg -y -f s24le -ar 352.8k -ac 2 -i out.pcm -c:a pcm_s32le 1000hz.wav
rm -f out.pcm
ffplay 1000hz.wav