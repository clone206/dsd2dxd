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
#include <fstream>
#include <string>
#include <math.h>

#include "dsd2pcm.hpp"
#include "noiseshape.hpp"

using namespace std;

namespace {
    const float my_ns_coeffs[] = {
    //     b1           b2           a1           a2
      -1.62666423,  0.79410094,  0.61367127,  0.23311013,  // section 1
      -1.44870017,  0.54196219,  0.03373857,  0.70316556   // section 2
    };

    const int my_ns_soscount = sizeof(my_ns_coeffs)
        / (sizeof(my_ns_coeffs[0]) * 4);

    int clips = 0;

    // Output/dither vars
    uint32_t fpd_l = 1.0; // FP Dither L
    uint32_t fpd_r = 1.0; // FP Dither R
    double noise_shaping_l;// Noise shape state L
    double noise_shaping_r;// Noise shape state R
    double byn_l[13]; // Delay line L
    double byn_r[13]; // Delay line R;

    // Initialize outputs/dither state
    inline void init_outputs() {
        while (fpd_l < 16386) fpd_l = rand() * UINT32_MAX;
        while (fpd_r < 16386) fpd_r = rand() * UINT32_MAX;

        noise_shaping_l = 0.0;
        noise_shaping_r = 0.0;
       
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

    // Not Just Another Dither 
    // Not truly random. Uses Benford Real Numbers for the dither values
    // Part of Airwindows plugin suite
    inline int njad(float &sample, int chanNum, float scaleFactor)
    {
        double inputSample = (double)sample;
        uint32_t *fpd;
        double *noiseShaping;
        double (*byn)[13];

        if (chanNum == 0) {
            fpd = &fpd_l;
            noiseShaping = &noise_shaping_l;
            byn = &byn_l;
        } else if (chanNum ==1) {
            fpd = &fpd_r;
            noiseShaping = &noise_shaping_r;
            byn = &byn_r;
        } else {
            cerr << "njad only supports a maximum of 2 channels!";
            return 1;
        }

        if (fabs(inputSample) < 1.18e-23)
            inputSample = *fpd * 1.18e-17;

        *fpd ^= (*fpd) << 13;
        *fpd ^= (*fpd) >> 17;
        *fpd ^= (*fpd) << 5;

        inputSample *= scaleFactor;

        bool cutbins = false;
        double drySample = inputSample;
        inputSample -= *noiseShaping;

        double benfordize = floor(inputSample);
        while (benfordize >= 1.0)
            benfordize /= 10;
        while (benfordize < 1.0 && benfordize > 0.0000001)
            benfordize *= 10;

        //hotbin becomes the Benford bin value for this number floored
        int hotbinA = floor(benfordize);

        double totalA = 0;
        //produce total number- smaller is closer to Benford real
        if ((hotbinA > 0) && (hotbinA < 10))
        {
            *byn[hotbinA] += 1;

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
            *byn[hotbinA] -= 1;
        } else
            hotbinA = 10;
        
        //hotbin becomes the Benford bin value for this number ceiled
        benfordize = ceil(inputSample);

        while (benfordize >= 1.0)
            benfordize /= 10;
        while (benfordize < 1.0 && benfordize > 0.0000001)
            benfordize *= 10;

        int hotbinB = floor(benfordize);

        double totalB = 0;
        //produce total number- smaller is closer to Benford real
        if ((hotbinB > 0) && (hotbinB < 10))
        {
            (*byn[hotbinB]) += 1;
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
            *byn[hotbinB] -= 1;
        } else
            hotbinB = 10;
        
        double outputSample;

        //assign the relevant one to the delay line
        //and floor/ceil signal accordingly
        if (totalA < totalB) {
            (*byn[hotbinA]) += 1; 
            outputSample = floor(inputSample);
        } else {
            (*byn[hotbinB]) += 1;
            outputSample = floor(inputSample + 1);
        }

        if (cutbins) {
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
        
        outputSample /= scaleFactor;

        if (outputSample > 1.0)
            outputSample = 1.0;
        if (outputSample < -1.0)
            outputSample = -1.0;

        sample = (float)(inputSample + outputSample);

        return 0;
    }

    inline long myround(float x)
    {
        return static_cast<long>(round(x));
    }

    template<typename T>
    struct id { typedef T type; };

    template<typename T>
    inline T clip(
        typename id<T>::type min,
        T v,
        typename id<T>::type max)
    {
        if (v < min) {
            ++clips;
            return min;
        }
        if (v > max) {
            ++clips;
            return max;
        }
        return v;
    }

    inline void write_intel(unsigned char * ptr, unsigned long word,
        unsigned char bitsNum)
    {
        if (bitsNum == 20) {
            word <<= 4;
        }

        ptr[0] =  word        & 0xFF;
        ptr[1] = (word >>  8) & 0xFF;

        if (bitsNum == 24 || bitsNum == 20) {
            ptr[2] = (word >> 16) & 0xFF;
        } 
    }

    inline void tpdf(float &sample, float scaleFactor)
    {
        sample *= scaleFactor;
        double rand1 = ((double) rand()) / ((double) RAND_MAX); // rand value between 0 and 1
        double rand2 = ((double) rand()) / ((double) RAND_MAX); // rand value between 0 and 1
        sample += (float)(rand1 - rand2);
    }

} // anonymous namespace


int main(int argc, char *argv[])
{
    const int blockSize = 4096;
    char filtType = ' ';
    int channelsNum   = -1;
    int lsbitfirst = -1;
    int bits       = -1;
    int interleaved = -1;
    string infileName = "";

    if (argc==6) {
        if ('1' <= argv[1][0] && argv[1][0] <= '9')
            channelsNum = 1 + (argv[1][0] - '1');
        if (argv[2][0] == 'i' || argv[2][0] == 'I') {
            interleaved = 1;
            lsbitfirst = 0;
        }
        if (argv[2][0] == 'p' || argv[2][0] == 'P') {
            interleaved = 0;
            lsbitfirst = 1;
        }
        if (!strcmp(argv[3], "16")) bits = 16;
        if (!strcmp(argv[3], "20")) bits = 20;
        if (!strcmp(argv[3], "24")) bits = 24;
        if (argv[4][0] == 'x' || argv[4][0] == 'X') {
            filtType = 'x';
        }
        if (argv[4][0] == 'd' || argv[4][0] == 'D') {
            filtType = 'd';
        }
        infileName = argv[5];
    }
    if (channelsNum < 1 || lsbitfirst < 0 || bits < 0 || interleaved < 0
            || filtType == ' ') {
        cerr << "\nError: Got " << argc << " args.\n";
        cerr << "\n"
            "DSD2PCM filter (raw DSD64 --> 352 kHz raw PCM)\n"
            "(c) 2009 Sebastian Gesemann\n\n"
            "Syntax: dsd2pcm <channels> <format> <bitdepth> <infile>\n"
            "channels = 1,2,3,...,9 (number of channels in DSD stream)\n"
            "format = I (interleaved) or P (planar) (DSD stream option)\n"
            "bitdepth = 16, 20, or 24 (intel byte order, output option)\n"
            "infile = Input file name, containing raw dsd with either \n"
            "filter = X (XLD filter) or D (Original dsd2pcm filter)\n"
            "planar format and 4096 byte block size,\n"
            "or interleaved with 1 byte per channel.\n\n"
            "Outputs raw pcm to stdout (only supports *nix environment).'\n\n";
        return 1;
    }

    // Seed rng
    srand (static_cast <unsigned> (time(0)));

    ifstream inFile;
    inFile.open(infileName, ios::binary | ios::in);

    int bytespersample = bits == 20 ? 3 : (bits / 8);
    vector<dxd> dxds (channelsNum, dxd(filtType, lsbitfirst));
    float scaleFactor;

    if (bits == 16) {
        scaleFactor = 32768.0;
    } else if (bits == 24) {
        scaleFactor = 8388608.0;
    } else if (bits == 20) {
        scaleFactor = 524288.0;
    } else {
        cerr << "Unsupported bit depth";
        return 1;
    }

    init_outputs();

    vector<unsigned char> dsdData (blockSize * channelsNum);
    vector<float> floatData (blockSize);
    vector<unsigned char> pcmData (blockSize * channelsNum * bytespersample);
    char * const dsdIn  = reinterpret_cast<char*>(&dsdData[0]);
    char * const pcmOut = reinterpret_cast<char*>(&pcmData[0]);
    int dsdStride = interleaved ? channelsNum : 1;
    int dsdChanOffset = 1; // Default to one byte for interleaved

    while (inFile.read(dsdIn, blockSize * channelsNum)) {
        for (int c = 0; c < channelsNum; ++c) {
            if (!interleaved) {
                dsdChanOffset = blockSize;
            }
            dxds[c].translate(blockSize, &dsdData[0] + c * dsdChanOffset, dsdStride,
                lsbitfirst, &floatData[0], 1);

            unsigned char * out = &pcmData[0] + c * bytespersample;

            for (int s = 0; s < blockSize; ++s) {
                float r = floatData[s];
                if(njad(r, c, scaleFactor))
                    return 1;
                //tpdf(r, scaleFactor);
                long smp = clip(-scaleFactor, myround(r), scaleFactor);
                write_intel(out, smp, bits);
                out += channelsNum * bytespersample;
            }
        }
        cout.write(pcmOut, blockSize * channelsNum * bytespersample);
    }

    if (clips) {
        cerr << "Clipped " << clips << " times.\n";
    }

    inFile.close();
}