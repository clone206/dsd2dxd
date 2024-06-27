/*

Copyright 2009, 2011 Sebastian Gesemann. All rights reserved.

Redistribution and use in source and binary forms, with or without modification, are
permitted provided that the following conditions are met:

   1. Redistributions of source code must retain the above copyright notice, this list of
      conditions and the following disclaimer.

   2. Redistributions in binary form must reproduce the above copyright notice, this list
      of conditions and the following disclaimer in the documentation and/or other materials
      provided with the distribution.

THIS SOFTWARE IS PROVIDED BY SEBASTIAN GESEMANN ''AS IS'' AND ANY EXPRESS OR IMPLIED
WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL SEBASTIAN GESEMANN OR
CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF
ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

The views and conclusions contained in the software and documentation are those of the
authors and should not be interpreted as representing official policies, either expressed
or implied, of Sebastian Gesemann.

 */
/* ========================================
 *  NotJustAnotherDither - NotJustAnotherDither.h
 *  Copyright (c) 2016 airwindows, Airwindows uses the MIT license
 * ======================================== */

#include <iostream>
#include <vector>
#include <cstring>
#include <string>
#include <math.h>
#include <ctype.h>
#include <typeinfo>

#include "dsd2pcm.hpp"
#include "argagg.hpp"
#include "AudioFile.h"

using namespace std;

#define DSD_64_RATE 2822400

namespace
{

    int clips = 0;
    int lastSampsClippedLow = 0;
    int lastSampsClippedHigh = 0;
    bool loudMode = false;

    // Output/dither vars
    double noise_shaping_l; // Noise shape state L
    double noise_shaping_r; // Noise shape state R
    double byn_l[13];       // Delay line L
    double byn_r[13];       // Delay line R;
    uint32_t fpd;           // Floating-point dither

    inline bool lowercmp(char a, char b)
    {
        return tolower(a) == b;
    }

    struct OutputContext
    {
        int bits;
        int channelsNum;
        int rate;
        char output;

        template <typename T>
        static AudioFile<T> aFile;

        OutputContext(int outBits, int outChansNum, int outRate, char outType)
        {
            bits = outBits;
            channelsNum = outChansNum;
            rate = outRate;
            output = outType;

            if (!lowercmp(outType, 's'))
            {
                if (outBits == 32)
                {
                    aFile<float> = AudioFile<float>();
                    setFileParams<float>();
                    if (loudMode)
                    {
                        cerr << "Finished setting file params for float type\n";
                    }
                }
                else
                {
                    aFile<int> = AudioFile<int>();
                    setFileParams<int>();
                    if (loudMode)
                    {
                        cerr << "Finished setting file params for int type\n";
                    }
                }
            }
        }

        template <typename ST>
        void setFileParams()
        {
            if (loudMode) {
                cerr << "About to set params for file type " 
                    << typeid(OutputContext::aFile<ST>).name() << "\n";
            }

            aFile<ST>.setNumChannels(channelsNum);
            aFile<ST>.setBitDepth(bits);
            aFile<ST>.setSampleRate(rate);
        }

        void saveAndPrintFile(string fileName)
        {
            if (bits == 32)
            {
                if (loudMode)
                {
                    cerr << "About to write 32b file\n";
                }
                aFile<float>.save(fileName, AudioFileFormat::Wave);
                aFile<float>.printSummary();
            }
            else
            {
                if (loudMode)
                {
                    cerr << "About to write int file\n";
                }
                aFile<int>.save(fileName, AudioFileFormat::Wave);
                aFile<int>.printSummary();
            }
        }
    };

    template <typename T>
    AudioFile<T> OutputContext::aFile = {};

    template <typename ST>
    inline void pushSamp(ST samp, int c)
    {
        OutputContext::aFile<ST>.samples[c].push_back(samp);
    }

    // Initialize outputs/dither state
    inline void init_outputs()
    {
        // Weights based on Benford's law. Smaller leading digits more likely.
        byn_l[0] = 1000;
        byn_l[1] = 301;
        byn_l[2] = 176;
        byn_l[3] = 125;
        byn_l[4] = 97;
        byn_l[5] = 79;
        byn_l[6] = 67;
        byn_l[7] = 58;
        byn_l[8] = 51;
        byn_l[9] = 46;
        byn_l[10] = 1000;

        byn_r[0] = 1000;
        byn_r[1] = 301;
        byn_r[2] = 176;
        byn_r[3] = 125;
        byn_r[4] = 97;
        byn_r[5] = 79;
        byn_r[6] = 67;
        byn_r[7] = 58;
        byn_r[8] = 51;
        byn_r[9] = 46;
        byn_r[10] = 1000;
    }

    inline void init_rand()
    {
        fpd = 1.0;
        while (fpd < 16386)
            fpd = rand() * UINT32_MAX;
    }

    // Floating point dither for going from double to float
    // Part of Airwindows plugin suite
    inline void fpdither(double &sample)
    {
        double inputSample = sample;

        // begin stereo 32 bit floating point dither
        int expon;
        frexpf((float)inputSample, &expon);
        fpd ^= fpd << 13;
        fpd ^= fpd >> 17;
        fpd ^= fpd << 5;
        inputSample += (fpd * 3.4e-36l * pow(2, expon + 62)); // remove 'blend' for real use, it's for the demo;
        // end stereo 32 bit floating point dither

        inputSample = (float)inputSample; // equivalent of 'floor' for 32 bit floating point

        sample = inputSample;
    }

    // Not Just Another Dither
    // Not truly random. Uses Benford Real Numbers for the dither values
    // Part of Airwindows plugin suite
    inline int njad(double &sample, int chanNum, double scaleFactor)
    {
        double inputSample = sample;
        double *noiseShaping;
        double(*byn)[13];

        if (chanNum == 0)
        {
            noiseShaping = &noise_shaping_l;
            byn = &byn_l;
        }
        else if (chanNum == 1)
        {
            noiseShaping = &noise_shaping_r;
            byn = &byn_r;
        }
        else
        {
            cerr << "njad only supports a maximum of 2 channels!";
            return 1;
        }

        // Scale up so 0-1 is one bit of output format
        inputSample *= scaleFactor;

        bool cutbins = false;
        double drySample = inputSample;

        // Subtract error from previous iteration
        inputSample -= *noiseShaping;

        // Isolate leading digit of number
        double benfordize = floor(inputSample);
        while (benfordize >= 1.0)
            benfordize /= 10;
        while (benfordize < 1.0 && benfordize > 0.0000001)
            benfordize *= 10;

        // Hotbin A becomes the Benford bin value for this number floored (leading digit)
        int hotbinA = floor(benfordize);

        double totalA = 0;
        // produce total number- smaller of totalA & totalB is closer to Benford real
        if ((hotbinA > 0) && (hotbinA < 10))
        {
            // Temp add weight to this leading digit
            *byn[hotbinA] += 1;

            // Coeffs get permanently incremented later in the loop, eventually need
            // to be scaled back down (cut bins).
            if (*byn[hotbinA] > 982)
                cutbins = true;

            totalA += (301 - (*byn[1]));
            totalA += (176 - (*byn[2]));
            totalA += (125 - (*byn[3]));
            totalA += (97 - (*byn[4]));
            totalA += (79 - (*byn[5]));
            totalA += (67 - (*byn[6]));
            totalA += (58 - (*byn[7]));
            totalA += (51 - (*byn[8]));
            totalA += (46 - (*byn[9]));

            // Remove temp weight from this leading digit
            *byn[hotbinA] -= 1;
        }
        else
            hotbinA = 10; // 1000

        // Isolate leading digit of number
        benfordize = ceil(inputSample);

        while (benfordize >= 1.0)
            benfordize /= 10;
        while (benfordize < 1.0 && benfordize > 0.0000001)
            benfordize *= 10;

        // Hotbin B becomes the Benford bin value for this number ceiled
        int hotbinB = floor(benfordize);

        double totalB = 0;
        // produce total number- smaller of totalA & totalB is closer to Benford real
        if ((hotbinB > 0) && (hotbinB < 10))
        {
            // Temp add weight to this leading digit
            (*byn[hotbinB]) += 1;

            // Coeffs get permanently incremented later in the loop, eventually need
            // to be scaled back down (cut bins).
            if (*byn[hotbinB] > 982)
                cutbins = true;

            totalB += (301 - (*byn[1]));
            totalB += (176 - (*byn[2]));
            totalB += (125 - (*byn[3]));
            totalB += (97 - (*byn[4]));
            totalB += (79 - (*byn[5]));
            totalB += (67 - (*byn[6]));
            totalB += (58 - (*byn[7]));
            totalB += (51 - (*byn[8]));
            totalB += (46 - (*byn[9]));

            // Remove temp weight from this leading digit
            *byn[hotbinB] -= 1;
        }
        else
            hotbinB = 10;

        double outputSample;

        // assign the relevant one to the delay line
        // and floor/ceil (quantize) signal accordingly
        if (totalA < totalB)
        {
            // Add weight to relevant coeff
            (*byn[hotbinA]) += 1;
            outputSample = floor(inputSample);
        }
        else
        {
            // Add weight to relevant coeff
            (*byn[hotbinB]) += 1;
            // If totalB is less, we got here by using a sample that was
            // previously ceil'd to create hotbin, so add one
            outputSample = floor(inputSample + 1);
        }

        if (cutbins)
        {
            // Scale down coeffs (weights based on Benford's Law)
            (*byn[1]) *= 0.99;
            (*byn[2]) *= 0.99;
            (*byn[3]) *= 0.99;
            (*byn[4]) *= 0.99;
            (*byn[5]) *= 0.99;
            (*byn[6]) *= 0.99;
            (*byn[7]) *= 0.99;
            (*byn[8]) *= 0.99;
            (*byn[9]) *= 0.99;
            (*byn[10]) *= 0.99;
        }

        // Store the error
        *noiseShaping += outputSample - drySample;

        // Error shouldn't be greater than input sample value
        if (*noiseShaping > fabs(inputSample))
            *noiseShaping = fabs(inputSample);
        if (*noiseShaping < -fabs(inputSample))
            *noiseShaping = -fabs(inputSample);

        sample = outputSample / scaleFactor;

        return 0;
    }

    // TPDF dither
    inline void tpdf(double &sample, double scaleFactor)
    {
        sample *= scaleFactor;
        double rand1 = ((double)rand()) / ((double)RAND_MAX); // rand value between 0 and 1
        double rand2 = ((double)rand()) / ((double)RAND_MAX); // rand value between 0 and 1
        sample += (rand1 - rand2);
        sample /= scaleFactor;
    }

    inline int myround(double x)
    {
        // x += x >= 0 ? 0.5 : -0.5;
        return static_cast<int>(round(x));
    }

    template <typename T>
    struct id
    {
        typedef T type;
    };

    template <typename T>
    inline T clip(
        typename id<T>::type min,
        T v,
        typename id<T>::type max)
    {
        if (v < min)
        {
            if (lastSampsClippedLow == 1)
                ++clips;
            ++lastSampsClippedLow;
            return min;
        }
        lastSampsClippedLow = 0;

        if (v > max)
        {
            if (lastSampsClippedHigh == 1)
                ++clips;
            ++lastSampsClippedHigh;
            return max;
        }
        lastSampsClippedHigh = 0;

        return v;
    }

    inline void write_intel(unsigned char *ptr, unsigned long word,
                            unsigned char bitsNum)
    {
        if (bitsNum == 20)
        {
            word <<= 4;
        }

        ptr[0] = word & 0xFF;
        ptr[1] = (word >> 8) & 0xFF;

        if (bitsNum == 24 || bitsNum == 20)
        {
            ptr[2] = (word >> 16) & 0xFF;
        }
    }

    inline void write_float(unsigned char *ptr, double sample)
    {
        float word = (float)sample;

        memcpy(ptr, &word, sizeof(float));
    }
} // anonymous namespace

int main(int argc, char *argv[])
{
    argagg::parser argparser{{{"help", {"-h", "--help"}, "shows this help message", 0},
                              {"channels", {"-c", "--channels"}, "Number of channels (default: 2)", 1},
                              {"format",
                               {"-f", "--fmt"},
                               "I (interleaved) or P (planar) (DSD stream option) (default: I)",
                               1},
                              {"bitdepth", {"-b", "--bitdepth"}, "16, 20, 24, or 32 (float) (intel byte order, output option) (default: 24)", 1},
                              {"filtertype", {"-t", "--filttype"}, "X (XLD filter), D (Original dsd2pcm filter. Only available with 8:1 decimation ratio), \n\tE (Equiripple. Only available with double rate DSD input), C (Chebyshev. Only available with double rate DSD input)\n\t(default: X [single rate] or C [double rate])", 1},
                              {"endianness", {"-e", "--endianness"}, "Byte order of input. M (MSB first) or L (LSB first) (default: M)", 1},
                              {"blocksize", {"-s", "--bs"}, "Block size to read/write at a time in bytes, e.g. 4096 (default: 4096)", 1},
                              {"dithertype", {"-d", "--dither"}, "Which type of dither to use. T (TPDF), N (Not Just Another Dither), F (floating point dither), or X (no dither) (default: F for 32 bit, T otherwise)", 1},
                              {"decimation", {"-r", "--ratio"}, "Decimation ratio. 8, 16, 32, or 64 (to 1) (default: 8. 64 only available with double rate DSD, Chebyshev filter)", 1},
                              {"inputrate", {"-i", "--inrate"}, "Input DSD data rate. 1 (dsd64) or 2 (dsd128) (default: 1. 2 only available with Decimation ratio of 16, 32, or 64)", 1},
                              {"output", {"-o", "--output"}, "Output type. S (stdout), or W (wave) (default: S. Note that W outputs to outfile.wav in current directory)", 1},
                              {"volume", {"-v", "--volume"}, "Volume adjustment in dB. If a negative number is needed use the --volume= format. (default: 0).", 1},
                              {"loudmode", {"-l", "--loud"}, "Print diagnostic messages to stderr", 0}}};

    argagg::parser_results args;
    try
    {
        args = argparser.parse(argc, argv);
    }
    catch (const std::exception &e)
    {
        cerr << e.what() << '\n';
        return 1;
    }

    if (args["help"])
    {
        cerr << "\ndsd2dxd filter (raw DSD --> raw PCM).\n"
                "Reads from stdin and writes to stdout in a *nix environment.\n"
             << argparser;
        return 0;
    }

    int channelsNum = args["channels"].as<int>(2);
    char fmt = args["format"].as<string>("I").c_str()[0];
    const int bits = args["bitdepth"].as<int>(24);
    char endianness = args["endianness"].as<string>("M").c_str()[0];
    int blockSize = args["blocksize"].as<int>(4096);
    char ditherType = args["dithertype"].as<string>(bits == 32 ? "F" : "T").c_str()[0];
    int decimation = args["decimation"].as<int>(8);
    int dsdRate = args["inputrate"].as<int>(1);
    char filtType = args["filtertype"].as<string>(dsdRate == 2 ? "C" : "X").c_str()[0];
    char output = args["output"].as<string>("S").c_str()[0];
    double volAdj = args["volume"].as<double>(0.0);

    if (args["loudmode"])
    {
        loudMode = true;
    }

    int lsbitfirst;
    int interleaved;

    if (lowercmp(endianness, 'l'))
    {
        lsbitfirst = 1;
    }
    else if (lowercmp(endianness, 'm'))
    {
        lsbitfirst = 0;
    }
    else
    {
        cerr << "\nNo endianness detected!\n";
        return 1;
    }

    if (lowercmp(fmt, 'p'))
    {
        interleaved = 0;
    }
    else if (lowercmp(fmt, 'i'))
    {
        interleaved = 1;
    }
    else
    {
        cerr << "\nNo fmt detected!\n";
        return 1;
    }

    if (bits != 16 && bits != 20 && bits != 24 && bits != 32)
    {
        cerr << "\nUnsupported bit depth\n";
        return 1;
    }

    if (dsdRate != 1 && dsdRate != 2)
    {
        cerr << "Unsupported DSD input rate.\n";
        return 1;
    }
    else if (dsdRate == 2 && decimation != 16 && decimation != 32 && decimation != 64)
    {
        cerr << "\nOnly decimation value of 16, 32, or 64 allowed with dsd128 input.\n";
        return 1;
    }
    else if (dsdRate == 1 && decimation != 8 && decimation != 16 && decimation != 32)
    {
        cerr << "\nOnly decimation value of 8, 16, or 32 allowed with dsd64 input.\n";
        return 1;
    }

    // Seed rng
    srand(static_cast<unsigned>(time(0)));

    vector<dxd> dxds(channelsNum, dxd(filtType, lsbitfirst, decimation, dsdRate));
    int bytespersample = bits == 20 ? 3 : bits / 8;
    double scaleFactor = 1.0;
    double volScale = pow(10.0, volAdj / 20);

    if (bits != 32)
    {
        scaleFactor = pow(2.0, (bits - 1));
    }

    int peakLevel = (int)floor(scaleFactor);
    scaleFactor *= volScale;
    int outRate = DSD_64_RATE * dsdRate / decimation;

    OutputContext outCtx(bits, channelsNum, outRate, output);

    if (loudMode && !lowercmp(outCtx.output, 's'))
    {
        if (outCtx.bits == 32) {
            cerr << "File type " << typeid(OutputContext::aFile<float>).name() << "\n";
        } else {
            cerr << "File type " << typeid(OutputContext::aFile<int>).name() << "\n";
        }

        cerr << "Bits: " << outCtx.bits << " SR: " << outCtx.rate << " Chans: "
            << outCtx.channelsNum << "\n";

        OutputContext::aFile<float>.printSummary();
    }

    cerr << "\nInterleaved: " << (interleaved ? "yes" : "no")
         << "\nLs bit first: " << (lsbitfirst ? "yes" : "no")
         << "\nDither type: " << ditherType
         << "\nFilter type: " << filtType
         << "\nBit depth: " << outCtx.bits
         << "\nOutput Rate: " << outCtx.rate
         << "\nDecimation: " << decimation
         << "\nPeak level: " << peakLevel
         << "\nChannels: " << outCtx.channelsNum << "\n\n";

    if (lowercmp(ditherType, 'n'))
    {
        init_outputs();
    }
    else if (lowercmp(ditherType, 'f'))
    {
        init_rand();
    }

    int pcmBlockSize = blockSize / (decimation / 8);
    vector<unsigned char> dsdData(blockSize * channelsNum);
    vector<double> floatData(pcmBlockSize);
    vector<unsigned char> pcmData(pcmBlockSize * channelsNum * bytespersample);
    char *const dsdIn = reinterpret_cast<char *>(&dsdData[0]);
    char *const pcmOut = reinterpret_cast<char *>(&pcmData[0]);
    int dsdStride = interleaved ? outCtx.channelsNum : 1;
    int dsdChanOffset = interleaved ? 1 : blockSize; // Default to one byte for interleaved
    int outBlockSize = pcmBlockSize * outCtx.channelsNum * bytespersample;

    if (loudMode)
    {
        cerr << "About to start main conversion loop\n";
    }

    while (cin.read(dsdIn, blockSize * outCtx.channelsNum))
    {
        if (loudMode) {
            cerr << "-";
        }

        for (int c = 0; c < outCtx.channelsNum; ++c)
        {
            dxds[c].translate(blockSize, &dsdData[0] + c * dsdChanOffset, dsdStride,
                              &floatData[0], 1, decimation);

            unsigned char *out = &pcmData[0] + c * bytespersample;

            for (int s = 0; s < pcmBlockSize; ++s)
            {
                double r = floatData[s];

                // Dither (scaled up and down within functions)
                if (lowercmp(ditherType, 'n'))
                {
                    if (njad(r, c, scaleFactor))
                        return 1;
                }
                else if (lowercmp(ditherType, 't'))
                {
                    tpdf(r, scaleFactor);
                }
                else if (lowercmp(ditherType, 'f'))
                {
                    fpdither(r);
                }
                else if (!lowercmp(ditherType, 'x'))
                {
                    cerr << "\nInvalid dither type!\n\n";
                    return 1;
                }

                // Scale based on destination bit depth/vol adjustment
                r *= scaleFactor;

                if (lowercmp(outCtx.output, 's'))
                {
                    if (outCtx.bits == 32)
                    {
                        write_float(out, r);
                    }
                    else
                    {
                        int smp = clip(-peakLevel, myround(r), peakLevel - 1);
                        write_intel(out, smp, bits);
                    }

                    out += channelsNum * bytespersample;
                }
                else
                {
                    if (outCtx.bits == 32)
                    {
                        pushSamp((float)r, c);
                    }
                    else
                    {
                        pushSamp(clip(-peakLevel, myround(r), peakLevel - 1), c);
                    }
                }
            }
        }

        if (lowercmp(outCtx.output, 's'))
        {
            cout.write(pcmOut, outBlockSize);
        }
    }

    if (loudMode) {
        cerr << "\n";
    }

    if (clips)
    {
        cerr << "Clipped " << clips << " times.\n";
    }

    if (!lowercmp(outCtx.output, 's'))
    {
        outCtx.saveAndPrintFile("outfile.wav");
    }

    return 0;
}