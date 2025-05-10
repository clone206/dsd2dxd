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

#include <filesystem>
#include <math.h>
#include <FLAC++/encoder.h>

#include "AudioFile.h"

using namespace std;
namespace fs = std::filesystem;

class OutputContext {
public:
    // Init'd via input params
    int bits;
    int channelsNum;
    int rate;
    int decimRatio;
    int bytespersample;
    int blockSize;
    int pcmBlockSize;
    int outBlockSize;
    char output;
    char filtType;

    // Set freely
    int peakLevel;
    double scaleFactor;

    template <typename T>
    static AudioFile<T> aFile;

    OutputContext() {}

    OutputContext(int outBits, char outType,
                  int decimation, char filtTypeOut,
                  double outVol)
    {
        if (outBits != 16 && outBits != 20 && outBits != 24 && outBits != 32)
        {
            throw "Unsupported bit depth";
        }

        bits = outBits;
        output = tolower(outType);

        if (output != 's' && output != 'w' && output != 'a' && output != 'f')
        {
            throw "Unrecognized output type";
        }

        if (output == 'f' && outBits == 32)
        {
            throw "32 bit float not allowed with flac output";
        }

        decimRatio = decimation;
        bytespersample = bits == 20 ? 3 : bits / 8;
        filtType = tolower(filtTypeOut);

        setScaling(outVol);
    }

    void setBlockSize(int blockSizeOut, int chanNumOut)
    {
        channelsNum = chanNumOut;
        blockSize = blockSizeOut;
        pcmBlockSize = blockSize / (decimRatio / 8);
        outBlockSize = pcmBlockSize * channelsNum * bytespersample;
    }

    void initFile()
    {
        if (output == 's')
        {
            return;
        }

        if (bits == 32)
        {
            aFile<float> = AudioFile<float>();
            setFileParams<float>();
        }
        else
        {
            aFile<int> = AudioFile<int>();
            setFileParams<int>();
        }
    }

    void setScaling(double volume)
    {
        scaleFactor = 1.0;
        double volScale = pow(10.0, volume / 20);

        if (bits != 32)
        {
            scaleFactor = pow(2.0, (bits - 1));
        }

        peakLevel = (int)floor(scaleFactor);
        scaleFactor *= volScale;
    }

    template <typename ST>
    void setFileParams()
    {
        aFile<ST>.setNumChannels(channelsNum);
        aFile<ST>.setBitDepth(bits);
        aFile<ST>.setSampleRate(rate);
    }

    void saveAndPrintFile(string fileName, AudioFileFormat fmt)
    {
        if (bits == 32)
        {
            aFile<float>.save(fileName, fmt);
            aFile<float>.printSummary();
        }
        else
        {
            aFile<int>.save(fileName, fmt);
            aFile<int>.printSummary();
        }
        cerr << "Wrote to file: " << fileName << "\n";
    }

    void saveFlacFile(string filePath)
    {
        // flac vars
        bool ok = true;
        FLAC::Encoder::File encoder;
        FLAC__StreamEncoderInitStatus init_status;

        if (!encoder)
        {
            cerr << "Couldn't init flac encoder\n";
            throw 1;
        }

        int samplesNum = aFile<int>.getNumSamplesPerChannel();
        FLAC__int32 *samples[channelsNum];

        for (int c = 0; c < channelsNum; ++c)
        {
            samples[c] = &(aFile<int>.samples[c][0]);
        }

        ok &= encoder.set_verify(true);
        ok &= encoder.set_compression_level(8);
        ok &= encoder.set_channels(channelsNum);
        ok &= encoder.set_bits_per_sample(bits);
        ok &= encoder.set_sample_rate(rate);
        ok &= encoder.set_total_samples_estimate(samplesNum * channelsNum);

        if (ok)
        {
            init_status = encoder.init(filePath.c_str());

            if (init_status != FLAC__STREAM_ENCODER_INIT_STATUS_OK)
            {
                fprintf(stderr, "ERROR: initializing encoder: %s\n", FLAC__StreamEncoderInitStatusString[init_status]);
                ok = false;
            }
        }

        if (!ok)
        {
            throw 1;
        }

        if (!(ok = encoder.process(samples, samplesNum)))
        {
            fprintf(stderr, "   state: %s\n", encoder.get_state().resolved_as_cstring(encoder));
            throw 1;
        }

        ok &= encoder.finish();

        if (ok)
        {
            fprintf(stderr, "Conversion completed sucessfully for %s.\n", filePath.c_str());
        }
        else
        {
            fprintf(stderr, "\nError during conversion.\n");
            fprintf(stderr, "encoding: %s\n", ok ? "succeeded" : "FAILED");
            fprintf(stderr, "   state: %s\n", encoder.get_state().resolved_as_cstring(encoder));
            throw 1;
        }
    }

    template <typename ST>
    void pushSamp(ST samp, int c)
    {
        aFile<ST>.samples[c].push_back(samp);
    }

    void setRate(int newRate) {
        rate = newRate;
        // Update any other dependent members if necessary
    }
};

template <typename T>
AudioFile<T> OutputContext::aFile = {};
