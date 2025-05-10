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

#include "dsd2pcm.hpp"
#include "argagg.hpp"
#include "AudioFile.h"
#include "dsdin.hpp"
#include "Output.hpp"
#include "Dither.hpp"
#include "Input.hpp"
#include "ConversionContext.hpp"

using namespace std;

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
            ConversionContext convCtx(inCtx, outCtx, dither, verboseMode);
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