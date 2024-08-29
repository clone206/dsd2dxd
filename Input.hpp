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

#include <filesystem>
#include <taglib/tag.h>
#include <taglib/fileref.h>
#include <taglib/tpropertymap.h>
#include <iostream>

#include "dsdin.hpp"

using namespace std;
namespace fs = std::filesystem;

struct InputContext
{
    bool verboseMode = false;
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
    TagLib::PropertyMap props;

    inline void verbose(string say, bool newLine = true);
    inline bool lowercmp(char a, char b);

    InputContext() {}

    InputContext(string inFile, char fmt, char endianness, int inRate,
                 int blockSizeIn, int channels, bool verboseParam)
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
        props = TagLib::PropertyMap();
        verboseMode = verboseParam;

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
            verbose("Input file basename: ", false);
            verbose(filePath.stem());
            verbose("Parent path: ", false);
            verbose(fs::absolute(filePath).parent_path());

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
                    verbose("Setting block size from file");
                    setBlockSize(myDsd.blockSize);
                }
                verbose("Audio length in bytes: ", false);
                verbose(std::to_string(audioLength));
            }

            TagLib::FileRef f(input.c_str());

            if (!f.isNull() && f.tag())
            {
                TagLib::Tag *tag = f.tag();
                verbose("Artist: ", false);
                verbose(tag->artist().to8Bit());
                props = f.properties();
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

inline void InputContext::verbose(string say, bool newLine)
{
    if (verboseMode)
    {
        cerr << say << (newLine ? "\n" : "");
    }
}

inline bool InputContext::lowercmp(char a, char b)
{
    return tolower(a) == b;
}

