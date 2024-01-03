#!/bin/sh

g++ *.c *.cpp -O3 -o dsd2pcm
./dsd2pcm 2 L 24 < 1000hz.dsd > 1000hz.pcm
ffmpeg -y -f s24le -ar 352.8k -ac 2 -i 1000hz.pcm -c:a pcm_s32le 1000hz.wav
ffplay 1000hz.wav