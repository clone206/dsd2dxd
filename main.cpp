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

#include <cstring>
#include <math.h>
#include <ctype.h>
#include <typeinfo>
#include <taglib/fileref.h>

#include "dsd2pcm.hpp"
#include "argagg.hpp"
#include "AudioFile.h"
#include "dsdin.hpp"
#include "Output.hpp"
#include "Dither.hpp"
#include "Input.hpp"

using namespace std;

#define DSD_64_RATE 2822400

namespace
{
    bool verboseMode = false;

    inline void verbose(string say, bool newLine = true)
    {
        if (verboseMode)
        {
            cerr << say << (newLine ? "\n" : "");
        }
    }

    int calculateOutRate(int dsdRate, int decimation)
    {
        return DSD_64_RATE * dsdRate / decimation;
    }

    struct ConversionContext
    {
        InputContext inCtx;
        OutputContext outCtx;
        Dither dither;

        vector<unsigned char> dsdData;
        vector<double> floatData;
        vector<unsigned char> pcmData;
        vector<dxd> dxds;

        // Trivial inits
        int clips;
        int lastSampsClippedLow;
        int lastSampsClippedHigh;

        inline void scaleAndDither(double &sample, int chanNum);
        inline void writeToBuffer(unsigned char *&out, double &sample, int chanNum);
        template <typename T>
        inline T clamp(T min, T v, T max);
        static inline int myRound(double x);
        inline void writeLSBF(unsigned char *&ptr, unsigned long word);
        static inline void writeFloat(unsigned char *ptr, double sample);

        ConversionContext() {}

        ConversionContext(InputContext &inCtxParam, OutputContext &outCtxParam,
                          Dither &ditherParam)
        {
            inCtx = inCtxParam;
            outCtx = outCtxParam;
            dither = ditherParam;

            outCtx.setRate(calculateOutRate(inCtx.dsdRate, outCtx.decimRatio));
            outCtx.setBlockSize(inCtx.blockSize, inCtx.channelsNum);
            outCtx.initFile();
            dither.init();

            clips = 0;
            lastSampsClippedHigh = 0;
            lastSampsClippedLow = 0;
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

        void writeFile()
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
    };

    template <typename T>
    inline T ConversionContext::clamp(T min, T v, T max)
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

    inline int ConversionContext::myRound(double x)
    {
        // x += x >= 0 ? 0.5 : -0.5;
        return static_cast<int>(round(x));
    }

    inline void ConversionContext::scaleAndDither(double &sample, int chanNum)
    {
        // Scale up so 0-1 is one bit of output format
        sample *= outCtx.scaleFactor;
        dither.processSamp(sample, chanNum);
    }

    inline void ConversionContext::writeToBuffer(unsigned char *&out, double &sample, int chanNum)
    {
        if (outCtx.output == 's')
        {
            if (outCtx.bits == 32)
            {
                writeFloat(out, sample);
            }
            else
            {
                writeLSBF(out, clamp(-outCtx.peakLevel,
                                     myRound(sample), outCtx.peakLevel - 1));
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
                outCtx.pushSamp(clamp(-outCtx.peakLevel, myRound(sample),
                                      outCtx.peakLevel - 1),
                                chanNum);
            }
        }
    }

    inline void ConversionContext::writeLSBF(unsigned char *&ptr, unsigned long word)
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

    inline void ConversionContext::writeFloat(unsigned char *ptr, double sample)
    {
        float word = (float)sample;

        memcpy(ptr, &word, sizeof(float));
    }

    argagg::parser_results parseArgs(int argc, char *argv[])
    {
        argagg::parser argparser{{{"help", {"-h", "--help"}, "shows this help message", 0},
                                  {"channels", {"-c", "--channels"}, "Number of channels (default: 2)", 1},
                                  {"format",
                                   {"-f", "--fmt"},
                                   "I (interleaved) or P (planar) (DSD stream option) (default: I)",
                                   1},
                                  {"bitdepth", {"-b", "--bitdepth"}, "16, 20, 24, or 32 (float) (intel byte order, output option) (default: 24)", 1},
                                  {"filtertype", {"-t", "--filttype"}, "X (XLD filter), D (Original dsd2pcm filter. Only \n\tavailable with 8:1 decimation ratio), \n\tE (Equiripple. Only \n\tavailable with double rate DSD input),\n\tC (Chebyshev. Only available with double rate DSD input)\n\t(default: X [single rate] or C [double rate])", 1},
                                  {"endianness", {"-e", "--endianness"}, "Byte order of input. M (MSB first) or L (LSB first) (default: M)", 1},
                                  {"blocksize", {"-s", "--bs"}, "Block size to read/write at a time in bytes, e.g. 4096 (default: 4096)", 1},
                                  {"dithertype", {"-d", "--dither"}, "Which type of dither to use. T (TPDF), R (rectangular), N (Not Just Another\n\tDither), F (floating point dither), or X (no dither) (default: F for 32 bit, T otherwise)", 1},
                                  {"decimation", {"-r", "--ratio"}, "Decimation ratio. 8, 16, 32, or 64 (to 1) (default: 8. 64 only available with \n\tdouble rate DSD, Chebyshev filter)", 1},
                                  {"inputrate", {"-i", "--inrate"}, "Input DSD data rate. 1 (dsd64) or 2 (dsd128) (default: 1. 2 only available with \n\tDecimation ratio of 16, 32, or 64)", 1},
                                  {"output", {"-o", "--output"}, "Output type. S (stdout), A (aif), W (wave), or F (flac)\n\t(default: S. Note that W, A, or F outputs to either \n\t<basename>.[wav|aif|flac] in current directory,\n\twhere <basename> is the input filename \n\twithout the extension, or outfile.[wav|aif|flac] if reading from stdin.)", 1},
                                  {"level", {"-l", "--level"}, "Volume level adjustment in dB. If a negative number is needed use the --level= \n\tformat (with no space after the \"=\"). (default: 0).", 1},
                                  {"verbose", {"-v", "--verbose"}, "Print diagnostic messages to standard error while converting.", 0}}};

        argagg::parser_results args = argparser.parse(argc, argv);

        if (args["help"])
        {
            cerr << "\ndsd2dxd filter (DSD --> PCM).\n"
                    "Reads from stdin or file and writes to stdout or file in a *nix environment.\n\n"
                    "Usage: dsd2dxd [options] [infile(s)|-], where - means read from stdin\n\n"
                    "If reading from a file, certain command line options you provide (e.g. block size) may be overridden \n"
                    "using the metadata found in that file (either a dsf or dff file).\n"
                    "If neither filename(s) or - is provided, standard in is assumed.\n"
                    "Multiple filenames can be provided and the input-related options specified will be applied to each, \n"
                    "except where overridden by each file's metadata.\n"
                 << argparser;
            throw 0;
        }

        if (args["verbose"])
        {
            verboseMode = true;
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
    auto bitDepth = args["bitdepth"].as<int>(24);
    auto format = args["format"].as<string>("I").c_str()[0];
    auto endianness = args["endianness"].as<string>("M").c_str()[0];
    auto ditherType = args["dithertype"].as<string>(bitDepth == 32
                                                        ? "F"
                                                        : "T")
                          .c_str()[0];

    // cerr << "Dither type: "<< ditherType << "\n";

    OutputContext outCtx;
    try
    {
        outCtx = OutputContext(bitDepth,
                               args["output"].as<string>("S").c_str()[0],
                               decimation,
                               args["filtertype"].as<string>(inputRate == 2
                                                                 ? "C"
                                                                 : "X")
                                   .c_str()[0],
                               args["level"].as<double>(0.0));
    }
    catch (const char *str)
    {
        cerr << str << "\n";
        return 1;
    }

    for (const string &input : inputs)
    {
        // Check for unexpanded glob patterns (any input containing '*')
        if (input.find('*') != string::npos)
        {
            verbose(
                "Warning: Unexpanded glob pattern detected in input: \"" + 
                input + "\". Skipping.");
            continue; // Skip this input
        }

        verbose("Input: " + input);

        InputContext inCtx;
        try
        {
            inCtx = InputContext(input, format, endianness, inputRate, blockSize,
                                 channels, verboseMode);
        }
        catch (const char *str)
        {
            cerr << str << "\n";
            return 1;
        }

        try
        {
            Dither dither(ditherType);
            ConversionContext convCtx(inCtx, outCtx, dither);
            convCtx.doConversion();
        }
        catch (int r)
        {
            return r;
        }
    }

    verbose("About to exit.");
    return 0;
}