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

#include "ConversionContext.hpp"
#include <fstream>
#include <iostream>
#include <cmath>
#include <cstring>
#include <taglib/fileref.h>
#include "AudioFile.h"

using namespace std;

// Helper function (if not already global)
int calculateOutRate(int dsdRate, int decimation)
{
    return DSD_64_RATE * dsdRate / decimation;
}

ConversionContext::ConversionContext() {}

ConversionContext::ConversionContext(InputContext &inCtxParam, OutputContext &outCtxParam, Dither &ditherParam, bool verboseParam)
{
    inCtx = inCtxParam;
    outCtx = outCtxParam;
    dither = ditherParam;
    verboseMode = verboseParam;

    outCtx.setRate(calculateOutRate(inCtx.dsdRate, outCtx.decimRatio));
    outCtx.setBlockSize(inCtx.blockSize, inCtx.channelsNum);
    outCtx.initFile();
    dither.init();

    clips = 0;
    lastSampsClippedHigh = 0;
    lastSampsClippedLow = 0;
}

void ConversionContext::verbose(string say, bool newLine = true)
    {
        if (verboseMode)
        {
            cerr << say << (newLine ? "\n" : "");
        }
    }

void ConversionContext::doConversion()
{
    // Set up input stream depending on whether we're working
    // with a file or stdin
    ifstream fp;
    istream &in = (!inCtx.stdIn)
        ? [&fp](string input, int64_t audioPos) -> istream &
    {
        fp.open(input);
        if (!fp)
            abort();
        fp.seekg(audioPos);
        return fp;
    }(inCtx.input, inCtx.audioPos)
        : std::cin;

    checkConv();

    dsdData.resize(inCtx.blockSize * inCtx.channelsNum);
    floatData.resize(outCtx.pcmBlockSize);
    pcmData.resize(outCtx.pcmBlockSize * outCtx.channelsNum * outCtx.bytespersample);
    dxds = vector<dxd>(outCtx.channelsNum, dxd(outCtx.filtType, inCtx.lsbitfirst,
                                               outCtx.decimRatio, inCtx.dsdRate));
    char *const dsdIn = reinterpret_cast<char *>(&dsdData[0]);
    char *const pcmOut = reinterpret_cast<char *>(&pcmData[0]);

    int frameSize = inCtx.blockSize * inCtx.channelsNum;
    long bytesRemaining = inCtx.audioLength > 0 ? inCtx.audioLength : frameSize;
    int blockRemaining = inCtx.blockSize;

    verbose("About to start main conversion loop.");

    while ((inCtx.stdIn ? true : (bytesRemaining > 0)) && in.read(dsdIn, bytesRemaining > frameSize ? frameSize : bytesRemaining))
    {
        if (!inCtx.stdIn)
        {
            blockRemaining = (bytesRemaining > frameSize
                                  ? frameSize
                                  : bytesRemaining) /
                             inCtx.channelsNum;
            bytesRemaining -= frameSize;
        }

        for (int c = 0; c < inCtx.channelsNum; ++c)
        {
            dxds[c].translate(blockRemaining,
                              &dsdData[0] + c * inCtx.dsdChanOffset,
                              inCtx.dsdStride, &floatData[0], 1,
                              outCtx.decimRatio);

            unsigned char *out = &pcmData[0] + c * outCtx.bytespersample;

            for (int s = 0; s < outCtx.pcmBlockSize; ++s)
            {
                double r = floatData[s];

                scaleAndDither(r, c);
                writeToBuffer(out, r, c);
            }
        }

        if (outCtx.output == 's')
        {
            cout.write(pcmOut, outCtx.outBlockSize);
        }
    }

    verbose("\nDone with main conversion loop.");

    if (fp.is_open())
    {
        verbose("About to close input file.");
        fp.close();
    }

    if (clips)
    {
        cerr << "Clipped " << clips << " times.\n";
    }

    if (outCtx.output != 's')
    {
        writeFile();
    }
}

void ConversionContext::writeFile()
{
    string outBaseName = "outfile";
    string outExt = "";
    AudioFileFormat fmt;

    switch (outCtx.output)
    {
    case 'w':
        outExt = ".wav";
        fmt = AudioFileFormat::Wave;
        break;
    case 'a':
        outExt = ".aif";
        fmt = AudioFileFormat::Aiff;
        break;
    case 'f':
        outExt = ".flac";
        break;
    default:
        break;
    }

    string outPath = outBaseName + outExt;

    if (!inCtx.stdIn)
    {
        outPath = inCtx.parentPath.string() + "/" +
                  inCtx.filePath.stem().string() + outExt;
    }

    if (outCtx.output == 'f')
    {
        outCtx.saveFlacFile(outPath);
    }
    else
    {
        outCtx.saveAndPrintFile(outPath, fmt);
    }

    if (inCtx.props.size() > 0)
    {
        TagLib::FileRef f(outPath.c_str());
        f.setProperties(inCtx.props);
        f.save();
    }
}

void ConversionContext::checkConv()
{
    if (inCtx.dsdRate == 2 && outCtx.decimRatio != 16 && outCtx.decimRatio != 32 && outCtx.decimRatio != 64)
    {
        cerr << "\nOnly decimation value of 16, 32, or 64 allowed with dsd128 input.\n";
        cerr << "dec: " << outCtx.decimRatio << ".\nrate: " << inCtx.dsdRate;
        throw 1;
    }
    else if (inCtx.dsdRate == 1 && outCtx.decimRatio != 8 && outCtx.decimRatio != 16 && outCtx.decimRatio != 32)
    {
        cerr << "\nOnly decimation value of 8, 16, or 32 allowed with dsd64 input.\n";
        cerr << "dec: " << outCtx.decimRatio << ".\nrate: " << inCtx.dsdRate;
        throw 1;
    }

    if (verboseMode && outCtx.output != 's')
    {
        cerr << "Bits: " << outCtx.bits << " SR: " << outCtx.rate << " Chans: "
             << outCtx.channelsNum << "\n";
    }

    cerr << "\nInterleaved: " << (inCtx.interleaved ? "yes" : "no")
         << "\nLs bit first: " << (inCtx.lsbitfirst ? "yes" : "no")
         << "\nDither type: " << dither.type
         << "\nFilter type: " << outCtx.filtType
         << "\nBit depth: " << outCtx.bits
         << "\nOutput Rate: " << outCtx.rate
         << "\nDecimation: " << outCtx.decimRatio
         << "\nPeak level: " << outCtx.peakLevel
         << "\nIn Block Size: " << inCtx.blockSize
         << "\nOut Block Size: " << outCtx.blockSize
         << "\nPcm Block Size: " << outCtx.pcmBlockSize
         << "\nOutput Block Size: " << outCtx.outBlockSize
         << "\nChannels: " << outCtx.channelsNum << "\n\n";
}

void ConversionContext::scaleAndDither(double &sample, int chanNum)
{
    sample *= outCtx.scaleFactor;
    dither.processSamp(sample, chanNum);
}

void ConversionContext::writeToBuffer(unsigned char *&out, double &sample, int chanNum)
{
    if (outCtx.output == 's')
    {
        if (outCtx.bits == 32)
        {
            writeFloat(out, sample);
        }
        else
        {
            writeLSBF(out, clamp(-outCtx.peakLevel, myRound(sample), outCtx.peakLevel - 1));
        }
        out += outCtx.channelsNum * outCtx.bytespersample;
    }
    else
    {
        if (outCtx.bits == 32)
        {
            outCtx.pushSamp((float)sample, chanNum);
        }
        else
        {
            outCtx.pushSamp(clamp(-outCtx.peakLevel, myRound(sample), outCtx.peakLevel - 1), chanNum);
        }
    }
}

void ConversionContext::writeLSBF(unsigned char *&ptr, unsigned long word)
{
    if (outCtx.bits == 20)
    {
        word <<= 4;
    }
    ptr[0] = word & 0xFF;
    ptr[1] = (word >> 8) & 0xFF;
    if (outCtx.bits == 24 || outCtx.bits == 20)
    {
        ptr[2] = (word >> 16) & 0xFF;
    }
}

void ConversionContext::writeFloat(unsigned char *ptr, double sample)
{
    float word = (float)sample;
    memcpy(ptr, &word, sizeof(float));
}

int ConversionContext::myRound(double x)
{
    return static_cast<int>(round(x));
}