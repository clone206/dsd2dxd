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

#include <cstring>
#include <math.h>
#include <ctype.h>
#include <typeinfo>
#include <filesystem>

#include "dsd2pcm.hpp"
#include "argagg.hpp"
#include "AudioFile.h"
#include "dsdin.hpp"

using namespace std;
namespace fs = std::filesystem;

#define DSD_64_RATE 2822400

namespace
{
    bool loudMode = false;

    inline void loud(string say, bool newLine = true)
    {
        if (loudMode)
        {
            cerr << say << (newLine ? "\n" : "");
        }
    }

    inline bool lowercmp(char a, char b)
    {
        return tolower(a) == b;
    }

    struct InputContext
    {
        int lsbitfirst;
        bool interleaved;
        bool stdIn;
        int dsdRate;
        string input;
        fs::path filePath;

        int dsdStride;
        int dsdChanOffset;
        int channelsNum;
        int blockSize;
        long audioLength;
        off_t audioPos;

        InputContext() {}

        InputContext(string inFile, char fmt, char endianness, int inRate, int blockSizeIn, int channels)
        {
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
                throw "No endianness detected!";
            }

            if (lowercmp(fmt, 'p'))
            {
                interleaved = false;
            }
            else if (lowercmp(fmt, 'i'))
            {
                interleaved = true;
            }
            else
            {
                throw "No fmt detected!";
            }

            if (inRate != 1 && inRate != 2)
            {
                throw "Unsupported DSD input rate.";
            }

            input = inFile; // "-" == stdin
            channelsNum = channels;
            dsdRate = inRate;
            setBlockSize(blockSizeIn);

            if (input == "-")
            {
                stdIn = true;
                audioLength = 0;
                audioPos = 0;
            }
            else
            {
                stdIn = false;
                filePath = fs::path(input);
                loud("Input file basename: ", false);
                loud(filePath.stem());
                loud("Parent path: ", false);
                loud(fs::absolute(filePath).parent_path());

                FILE *inFile;
                if ((inFile = fopen(input.c_str(), "rb")) != NULL)
                {
                    auto myDsd = dsd(inFile);
                    audioPos = myDsd.audioPos;
                    audioLength = myDsd.audioLength;
                    channelsNum = myDsd.channelCount;
                    dsdRate = myDsd.dsdRate;
                    interleaved = myDsd.interleaved;
                    lsbitfirst = myDsd.isLsb;

                    if (myDsd.blockSize)
                    {
                        loud("Setting block size from file");
                        setBlockSize(myDsd.blockSize);
                    }
                    loud("Audio length in bytes: ", false);
                    loud(std::to_string(audioLength));
                }
            }
        }

        void setBlockSize(int blockSizeIn)
        {
            blockSize = blockSizeIn;
            dsdChanOffset = interleaved ? 1 : blockSize; // Default to one byte for interleaved
            dsdStride = interleaved ? channelsNum : 1;
        }
    };

    struct OutputContext
    {
        // Trivial inits
        int clips;
        int lastSampsClippedLow;
        int lastSampsClippedHigh;

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
        char ditherType;

        // Set freely
        int peakLevel;
        double scaleFactor;

        // Dither vars
        double noise_shaping_l; // Noise shape state L
        double noise_shaping_r; // Noise shape state R
        double byn_l[13];       // Delay line L
        double byn_r[13];       // Delay line R;
        uint32_t fpd;           // Floating-point dither

        template <typename T>
        static AudioFile<T> aFile;

        inline void fpdither(double &sample);
        inline void njad(double &sample, int chanNum);
        inline void tpdf(double &sample);
        inline void writeLSBF(unsigned char *&ptr, unsigned long word);
        static inline void writeFloat(unsigned char *ptr, double sample);
        static inline int myround(double x);
        template <typename ST>
        static inline void pushSamp(ST samp, int c);
        inline void scaleAndDither(double &sample, int chanNum);
        inline void writeToBuffer(unsigned char *&out, double &sample, int chanNum);

        template <typename T>
        inline T clip(T min, T v, T max);

        OutputContext() {}

        OutputContext(int outBits, int outRate, char outType,
                      int decimation, char ditherTypeOut, char filtTypeOut,
                      double outVol)
        {
            if (outBits != 16 && outBits != 20 && outBits != 24 && outBits != 32)
            {
                throw "Unsupported bit depth";
            }

            bits = outBits;
            rate = outRate;
            output = tolower(outType);
            decimRatio = decimation;
            clips = 0;
            lastSampsClippedHigh = 0;
            lastSampsClippedLow = 0;
            bytespersample = bits == 20 ? 3 : bits / 8;
            ditherType = tolower(ditherTypeOut);
            filtType = tolower(filtTypeOut);

            setScaling(outVol);

            if (ditherType != 'n' && ditherType != 't' && ditherType != 'f' && ditherType != 'x')
            {
                throw "Invalid dither type!";
            }
        }

        void setBlockSize(int blockSizeOut, int chanNumOut)
        {
            loud("Got block size of ", false);
            loud(std::to_string(blockSizeOut));
            channelsNum = chanNumOut;
            blockSize = blockSizeOut;
            pcmBlockSize = blockSize / (decimRatio / 8);
            outBlockSize = pcmBlockSize * channelsNum * bytespersample;
        }

        void initDither()
        {
            if (ditherType != 'x')
            {
                // Seed rng
                srand(static_cast<unsigned>(time(0)));
            }

            if (ditherType == 'n')
            {
                initOutputs();
            }
            else if (ditherType == 'f')
            {
                initRand();
            }
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
                loud("Finished setting file params for float type");
            }
            else
            {
                aFile<int> = AudioFile<int>();
                setFileParams<int>();
                loud("Finished setting file params for int type");
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
            loud("About to set params for file type:");
            loud(typeid(OutputContext::aFile<ST>).name());

            aFile<ST>.setNumChannels(channelsNum);
            aFile<ST>.setBitDepth(bits);
            aFile<ST>.setSampleRate(rate);
        }

        // Initialize outputs/dither state
        void initOutputs()
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

        void initRand()
        {
            fpd = 1.0;
            while (fpd < 16386)
                fpd = rand() * UINT32_MAX;
        }

        void saveAndPrintFile(string fileName, AudioFileFormat fmt)
        {
            if (bits == 32)
            {
                loud("About to write 32b file");
                aFile<float>.save(fileName, fmt);
                aFile<float>.printSummary();
            }
            else
            {
                loud("About to write int file");
                aFile<int>.save(fileName, fmt);
                aFile<int>.printSummary();
            }
            cerr << "Wrote to file: " << fileName << "\n";
        }
    };

    template <typename T>
    AudioFile<T> OutputContext::aFile = {};

    inline void OutputContext::writeToBuffer(unsigned char *&out, double &sample, int chanNum)
    {
        if (output == 's')
        {
            if (bits == 32)
            {
                writeFloat(out, sample);
            }
            else
            {
                writeLSBF(out, clip(-peakLevel,
                                    myround(sample), peakLevel - 1));
            }

            out += channelsNum * bytespersample;
        }
        else
        {
            if (bits == 32)
            {
                pushSamp((float)sample, chanNum);
            }
            else
            {
                pushSamp(clip(-peakLevel, myround(sample),
                              peakLevel - 1),
                         chanNum);
            }
        }
    }

    inline void OutputContext::scaleAndDither(double &sample, int chanNum)
    {
        // Scale up so 0-1 is one bit of output format
        sample *= scaleFactor;

        if (ditherType == 'n')
        {
            njad(sample, chanNum);
        }
        else if (ditherType == 't')
        {
            tpdf(sample);
        }
        else if (ditherType == 'f')
        {
            fpdither(sample);
        }
    }

    template <typename ST>
    inline void OutputContext::pushSamp(ST samp, int c)
    {
        aFile<ST>.samples[c].push_back(samp);
    }

    // Floating point dither for going from double to float
    // Part of Airwindows plugin suite
    inline void OutputContext::fpdither(double &inputSample)
    {
        // begin stereo 32 bit floating point dither
        int expon;
        frexpf((float)inputSample, &expon);
        fpd ^= fpd << 13;
        fpd ^= fpd >> 17;
        fpd ^= fpd << 5;
        inputSample += (fpd * 3.4e-36l * pow(2, expon + 62)); // removed 'blend' for real use, it's for the demo;
        // end stereo 32 bit floating point dither

        inputSample = (float)inputSample; // equivalent of 'floor' for 32 bit floating point
    }

    // Not Just Another Dither
    // Not truly random. Uses Benford Real Numbers for the dither values
    // Part of Airwindows plugin suite
    inline void OutputContext::njad(double &inputSample, int chanNum)
    {
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
            throw 1;
        }

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
    }

    template <typename T>
    inline T OutputContext::clip(T min, T v, T max)
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

    // TPDF dither
    inline void OutputContext::tpdf(double &sample)
    {
        double rand1 = ((double)rand()) / ((double)RAND_MAX); // rand value between 0 and 1
        double rand2 = ((double)rand()) / ((double)RAND_MAX); // rand value between 0 and 1
        sample += (rand1 - rand2);
    }

    inline int OutputContext::myround(double x)
    {
        // x += x >= 0 ? 0.5 : -0.5;
        return static_cast<int>(round(x));
    }

    inline void OutputContext::writeLSBF(unsigned char *&ptr, unsigned long word)
    {
        if (bits == 20)
        {
            word <<= 4;
        }

        ptr[0] = word & 0xFF;
        ptr[1] = (word >> 8) & 0xFF;

        if (bits == 24 || bits == 20)
        {
            ptr[2] = (word >> 16) & 0xFF;
        }
    }

    inline void OutputContext::writeFloat(unsigned char *ptr, double sample)
    {
        float word = (float)sample;

        memcpy(ptr, &word, sizeof(float));
    }

    struct ConversionContext
    {
        InputContext inCtx;
        OutputContext outCtx;

        vector<unsigned char> dsdData;
        vector<double> floatData;
        vector<unsigned char> pcmData;
        vector<dxd> dxds;

        ConversionContext() {}

        ConversionContext(InputContext &inCtxParam, OutputContext &outCtxParam)
        {
            inCtx = inCtxParam;
            outCtx = outCtxParam;
            outCtx.setBlockSize(inCtx.blockSize, inCtx.channelsNum);
            outCtx.initFile();
            outCtx.initDither();
        }

        void doConversion()
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

            loud("About to start main conversion loop.");

            while ((inCtx.stdIn ? true : (bytesRemaining > 0)) && in.read(dsdIn, bytesRemaining > frameSize ? frameSize : bytesRemaining))
            {
                if (!inCtx.stdIn)
                {
                    blockRemaining = (bytesRemaining > frameSize
                                          ? frameSize
                                          : bytesRemaining) /
                                     inCtx.channelsNum;
                    bytesRemaining -= frameSize;
                    // loud(std::to_string(bytesRemaining), false);
                }
                loud("-", false);

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

                        outCtx.scaleAndDither(r, c);
                        outCtx.writeToBuffer(out, r, c);
                    }
                }

                if (outCtx.output == 's')
                {
                    cout.write(pcmOut, outCtx.outBlockSize);
                }
            }

            loud("\nDone with main conversion loop.");

            if (fp.is_open())
            {
                loud("About to close input file.");
                fp.close();
            }

            if (outCtx.clips)
            {
                cerr << "Clipped " << outCtx.clips << " times.\n";
            }

            if (outCtx.output != 's')
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
                default:
                    break;
                }

                string outName = outBaseName + outExt;

                if (!inCtx.stdIn)
                {
                    outName = inCtx.filePath.stem().string() + outExt;
                }

                outCtx.saveAndPrintFile(outName, fmt);
            }
        }

        void checkConv()
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

            if (loudMode && outCtx.output != 's')
            {
                cerr << "Bits: " << outCtx.bits << " SR: " << outCtx.rate << " Chans: "
                     << outCtx.channelsNum << "\n";
            }

            cerr << "\nInterleaved: " << (inCtx.interleaved ? "yes" : "no")
                 << "\nLs bit first: " << (inCtx.lsbitfirst ? "yes" : "no")
                 << "\nDither type: " << outCtx.ditherType
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
    };

    argagg::parser_results parseArgs(int argc, char *argv[])
    {
        argagg::parser argparser{{{"help", {"-h", "--help"}, "shows this help message", 0},
                                  {"channels", {"-c", "--channels"}, "Number of channels (default: 2)", 1},
                                  {"format",
                                   {"-f", "--fmt"},
                                   "I (interleaved) or P (planar) (DSD stream option) (default: I)",
                                   1},
                                  {"bitdepth", {"-b", "--bitdepth"}, "16, 20, 24, or 32 (float) (intel byte order, output option) (default: 24)", 1},
                                  {"filtertype", {"-t", "--filttype"}, "X (XLD filter), D (Original dsd2pcm filter. Only \n\tavailable with 8:1 decimation ratio), \n\tE (Equiripple. Only \n\tavailable with double rate DSD input), C (Chebyshev. Only available with double rate DSD input)\n\t(default: X [single rate] or C [double rate])", 1},
                                  {"endianness", {"-e", "--endianness"}, "Byte order of input. M (MSB first) or L (LSB first) (default: M)", 1},
                                  {"blocksize", {"-s", "--bs"}, "Block size to read/write at a time in bytes, e.g. 4096 (default: 4096)", 1},
                                  {"dithertype", {"-d", "--dither"}, "Which type of dither to use. T (TPDF), N (Not Just Another Dither), F (floating \n\tpoint dither), or X (no dither) (default: F for 32 bit, T otherwise)", 1},
                                  {"decimation", {"-r", "--ratio"}, "Decimation ratio. 8, 16, 32, or 64 (to 1) (default: 8. 64 only available with \n\tdouble rate DSD, Chebyshev filter)", 1},
                                  {"inputrate", {"-i", "--inrate"}, "Input DSD data rate. 1 (dsd64) or 2 (dsd128) (default: 1. 2 only available with \n\tDecimation ratio of 16, 32, or 64)", 1},
                                  {"output", {"-o", "--output"}, "Output type. S (stdout), A (aif), or W (wave) (default: S. Note that W or A outputs to either \n\t<basename>.[wav|aif] in current directory, where <basename> is the input filename \n\twithout the extension, or outfile.[wav|aif] if reading from stdin.)", 1},
                                  {"volume", {"-v", "--volume"}, "Volume adjustment in dB. If a negative number is needed use the --volume= \n\tformat. (default: 0).", 1},
                                  {"loudmode", {"-l", "--loud"}, "Print diagnostic messages to stderr", 0}}};

        argagg::parser_results args = argparser.parse(argc, argv);

        if (args["help"])
        {
            cerr << "\ndsd2dxd filter (raw DSD --> PCM).\n"
                    "Reads from stdin or file and writes to stdout or file in a *nix environment.\n\n"
                    "Usage: dsd2dxd [options] [infile(s)|-], where - means read from stdin\n\n"
                    "If reading from a file, certain command line options you provide (e.g. block size) may be overridden \n"
                    "using the metadata found in that file (either a dsf or dff file).\n"
                    "If neither filename(s) or - is provided, stdin is assumed.\n"
                    "Multiple filenames can be provided and the input-related options specified will be applied to each, \n"
                    "except where overridden by each file's metadata.\n"
                 << argparser;
            throw 0;
        }

        if (args["loudmode"])
        {
            loudMode = true;
        }

        return args;
    }
} // anonymous namespace

int main(int argc, char *argv[])
{
    argagg::parser_results args;
    try
    {
        args = parseArgs(argc, argv);
    }
    catch (int r)
    {
        return r;
    }
    catch (const std::exception &e)
    {
        cerr << e.what() << '\n';
        return 1;
    }

    auto inputsNum = args.pos.size();
    vector<string> inputs;

    if (inputsNum == 0)
    {
        inputs.push_back("-");
    }

    for (int i = 0; i < inputsNum; ++i)
    {
        inputs.push_back(args.as<string>(i));
    }

    auto blockSize = args["blocksize"].as<int>(4096);
    auto channels = args["channels"].as<int>(2);
    auto inputRate = args["inputrate"].as<int>(1);
    auto decimation = args["decimation"].as<int>(8);
    auto outRate = DSD_64_RATE * inputRate / decimation;
    auto bitDepth = args["bitdepth"].as<int>(24);
    auto format = args["format"].as<string>("I").c_str()[0];
    auto endianness = args["endianness"].as<string>("M").c_str()[0];
    auto ditherType = args["dithertype"].as<string>(bitDepth == 32
                                                        ? "F"
                                                        : "T")
                          .c_str()[0];

    OutputContext outCtx;
    try
    {
        outCtx = OutputContext(bitDepth, outRate,
                               args["output"].as<string>("S").c_str()[0],
                               decimation, ditherType,
                               args["filtertype"].as<string>(inputRate == 2
                                                                 ? "C"
                                                                 : "X")
                                   .c_str()[0],
                               args["volume"].as<double>(0.0));
    }
    catch (const char *str)
    {
        cerr << str << "\n";
        return 1;
    }

    for (const string &input : inputs)
    {
        if (loudMode)
        {
            cerr << "Input: " << input << "\n";
        }

        InputContext inCtx;
        try
        {
            inCtx = InputContext(input, format, endianness, inputRate, blockSize,
                                 channels);
        }
        catch (const char *str)
        {
            cerr << str << "\n";
            return 1;
        }

        try
        {
            ConversionContext convCtx(inCtx, outCtx);
            convCtx.doConversion();
        }
        catch (int r)
        {
            return r;
        }
    }

    loud("About to exit.");
    return 0;
}