#!/bin/sh

g++ *.c *.cpp -O3 -o dsd2pcm
./dsd2pcm 1 L 24 1kHz.dsd
ffmpeg -y -f u24le -ar 352.8k -ac 1 -i out.pcm -c:a pcm_s32le 1kHz.wav
rm -f out.pcm
ffplay 1kHz.wav