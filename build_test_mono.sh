#!/bin/sh

g++ *.c *.cpp -O3 -o dsd2pcm
./dsd2pcm 1 L 24 < 1kHz.dsd > 1kHz.pcm
ffmpeg -y -f s24le -ar 352.8k -ac 1 -i 1kHz.pcm -c:a pcm_s32le 1kHz.wav
ffplay 1kHz.wav