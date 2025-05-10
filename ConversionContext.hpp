/*
 Copyright (c) 2023 clone206

 This file is part of dsd2dxd

 dsd2dxd is free software: you can redistribute it and/or modify it
 under the terms of the GNU General Public License as published by the
 Free Software Foundation, either version 3 of the License, or
 (at your option) any later version.

 dsd2dxd is distributed in the hope that it will be useful, but
 WITHOUT ANY WARRANTY; without even the implied warranty of
 MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 GNU General Public License for more details.
 You should have received a copy of the GNU General Public License
 along with dsd2dxd. If not, see <https://www.gnu.org/licenses/>.
*/

#pragma once

#include <vector>
#include <string>
#include "Input.hpp"
#include "Output.hpp"
#include "Dither.hpp"
#include "dsd2pcm.hpp"

// Define DSD_64_RATE here so it's available to both .hpp and .cpp
#define DSD_64_RATE 2822400

struct ConversionContext
{
    InputContext inCtx;
    OutputContext outCtx;
    Dither dither;

    std::vector<unsigned char> dsdData;
    std::vector<double> floatData;
    std::vector<unsigned char> pcmData;
    std::vector<dxd> dxds;

    int clips;
    int lastSampsClippedLow;
    int lastSampsClippedHigh;
    bool verboseMode;

    void verbose(string say, bool newLine);

    ConversionContext();
    ConversionContext(InputContext &inCtxParam, OutputContext &outCtxParam, 
                     Dither &ditherParam, bool verboseParam);

    void doConversion();
    void writeFile();
    void checkConv();

    void scaleAndDither(double &sample, int chanNum);
    void writeToBuffer(unsigned char *&out, double &sample, int chanNum);
    static int myRound(double x);
    void writeLSBF(unsigned char *&ptr, unsigned long word);
    static void writeFloat(unsigned char *ptr, double sample);

    template <typename T>
    T clamp(T min, T v, T max)
    {
        if (v < min)
        {
            if (lastSampsClippedLow == 1)
                ++clips;
            ++lastSampsClippedLow;
            return min;
        }
        if (v > max)
        {
            if (lastSampsClippedHigh == 1)
                ++clips;
            ++lastSampsClippedHigh;
            return max;
        }
        return v;
    }
};