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

#include <iostream>
#include <vector>
#include <cstring>
#include <fstream>
#include <string>
#include <math.h>

#include "dsd2pcm.hpp"
#include "noiseshape.hpp"

namespace {
    const float my_ns_coeffs[] = {
    //     b1           b2           a1           a2
      -1.62666423,  0.79410094,  0.61367127,  0.23311013,  // section 1
      -1.44870017,  0.54196219,  0.03373857,  0.70316556   // section 2
    };

    const int my_ns_soscount = sizeof(my_ns_coeffs) / (sizeof(my_ns_coeffs[0]) * 4);

    // njad vars
	uint32_t fpdL = 1.0;
	uint32_t fpdR = 1.0;
    bool highres = true;
	float scale_factor;
	double noiseShapingL;
	double noiseShapingR;
	double bynL[13];
	double bynR[13];

    // Initialize "Not just another Dither"
    inline void init_outputs(bool hires) {
        while (fpdL < 16386) fpdL = rand() * UINT32_MAX;
        while (fpdR < 16386) fpdR = rand() * UINT32_MAX;

        highres = hires;

	    if (hires)
            scale_factor = 8388608.0;
	    else
            scale_factor = 32768.0;
	    noiseShapingL = 0.0;
	    noiseShapingR = 0.0;
	    bynL[0] = 1000;
	    bynL[1] = 301;
	    bynL[2] = 176;
	    bynL[3] = 125;
	    bynL[4] = 97;
	    bynL[5] = 79;
	    bynL[6] = 67;
	    bynL[7] = 58;
	    bynL[8] = 51;
	    bynL[9] = 46;
	    bynL[10] = 1000;
	
	    bynR[0] = 1000;
	    bynR[1] = 301;
	    bynR[2] = 176;
	    bynR[3] = 125;
	    bynR[4] = 97;
	    bynR[5] = 79;
	    bynR[6] = 67;
	    bynR[7] = 58;
	    bynR[8] = 51;
	    bynR[9] = 46;
	    bynR[10] = 1000;
    }

    // Not just another Dither R
    inline void njad_r(float &sample) {
        double inputSampleR = (double)sample;
		if (fabs(inputSampleR)<1.18e-23) inputSampleR = fpdR * 1.18e-17;
		fpdR ^= fpdR << 13; fpdR ^= fpdR >> 17; fpdR ^= fpdR << 5;
		
		inputSampleR *= scale_factor;
		//0-1 is now one bit, now we dither

		bool cutbinsR; cutbinsR = false;
		double drySampleR; drySampleR = inputSampleR;
		inputSampleR -= noiseShapingR;

		double benfordize; benfordize = floor(inputSampleR);
		while (benfordize >= 1.0) benfordize /= 10;
		while (benfordize < 1.0 && benfordize > 0.0000001) benfordize *= 10;
		int hotbinA; hotbinA = floor(benfordize);
		//hotbin becomes the Benford bin value for this number floored
		double totalA; totalA = 0;

		if ((hotbinA > 0) && (hotbinA < 10))
		{
			bynR[hotbinA] += 1; if (bynR[hotbinA] > 982) cutbinsR = true;
			totalA += (301-bynR[1]); totalA += (176-bynR[2]); totalA += (125-bynR[3]);
			totalA += (97-bynR[4]); totalA += (79-bynR[5]); totalA += (67-bynR[6]);
			totalA += (58-bynR[7]); totalA += (51-bynR[8]); totalA += (46-bynR[9]);
			bynR[hotbinA] -= 1;
		} else hotbinA = 10;
		//produce total number- smaller is closer to Benford real
		
		benfordize = ceil(inputSampleR);
		while (benfordize >= 1.0) benfordize /= 10;
		while (benfordize < 1.0 && benfordize > 0.0000001) benfordize *= 10;		
		int hotbinB = floor(benfordize);
		//hotbin becomes the Benford bin value for this number ceiled
		double totalB = 0;
		if ((hotbinB > 0) && (hotbinB < 10))
		{
			bynR[hotbinB] += 1; if (bynR[hotbinB] > 982) cutbinsR = true;
			totalB += (301-bynR[1]); totalB += (176-bynR[2]); totalB += (125-bynR[3]);
			totalB += (97-bynR[4]); totalB += (79-bynR[5]); totalB += (67-bynR[6]);
			totalB += (58-bynR[7]); totalB += (51-bynR[8]); totalB += (46-bynR[9]);
			bynR[hotbinB] -= 1;
		} else hotbinB = 10;
		//produce total number- smaller is closer to Benford real
		
		double outputSample;
		if (totalA < totalB) {bynR[hotbinA] += 1; outputSample = floor(inputSampleR);}
		else {bynR[hotbinB] += 1; outputSample = floor(inputSampleR+1);}
		//assign the relevant one to the delay line
		//and floor/ceil signal accordingly
		
		if (cutbinsR) {
			bynR[1] *= 0.99; bynR[2] *= 0.99; bynR[3] *= 0.99; bynR[4] *= 0.99; bynR[5] *= 0.99; 
			bynR[6] *= 0.99; bynR[7] *= 0.99; bynR[8] *= 0.99; bynR[9] *= 0.99; bynR[10] *= 0.99; 
		}
		noiseShapingR += outputSample - drySampleR;			
		if (noiseShapingR > fabs(inputSampleR)) noiseShapingR = fabs(inputSampleR);
		if (noiseShapingR < -fabs(inputSampleR)) noiseShapingR = -fabs(inputSampleR);

		outputSample /= scale_factor;
		if (outputSample > 1.0) outputSample = 1.0;
		if (outputSample < -1.0) outputSample = -1.0;

        sample = (float)(inputSampleR + outputSample);
    }

    // Not Just Another Dither L
    inline void njad_l(float &sample) {
        double inputSampleL = (double)sample;

		if (fabs(inputSampleL) < 1.18e-23)
            inputSampleL = fpdL * 1.18e-17;

		fpdL ^= fpdL << 13;
        fpdL ^= fpdL >> 17;
        fpdL ^= fpdL << 5;

		inputSampleL *= scale_factor;

		bool cutbinsL = false;
		double drySampleL = inputSampleL;
		inputSampleL -= noiseShapingL;

		double benfordize = floor(inputSampleL);
		while (benfordize >= 1.0) benfordize /= 10;
		while (benfordize < 1.0 && benfordize > 0.0000001) benfordize *= 10;

		int hotbinA = floor(benfordize);
		//hotbin becomes the Benford bin value for this number floored
		double totalA = 0;

		if ((hotbinA > 0) && (hotbinA < 10))
		{
			bynL[hotbinA] += 1;
            if (bynL[hotbinA] > 982) cutbinsL = true;
			totalA += (301-bynL[1]);
            totalA += (176-bynL[2]);
            totalA += (125-bynL[3]);
			totalA += (97-bynL[4]);
            totalA += (79-bynL[5]);
            totalA += (67-bynL[6]);
			totalA += (58-bynL[7]);
            totalA += (51-bynL[8]);
            totalA += (46-bynL[9]);
			bynL[hotbinA] -= 1;
		} else hotbinA = 10;
		//produce total number- smaller is closer to Benford real
		
		benfordize = ceil(inputSampleL);

		while (benfordize >= 1.0) benfordize /= 10;
		while (benfordize < 1.0 && benfordize > 0.0000001) benfordize *= 10;

		int hotbinB; hotbinB = floor(benfordize);
		//hotbin becomes the Benford bin value for this number ceiled
		double totalB; totalB = 0;
		if ((hotbinB > 0) && (hotbinB < 10))
		{
			bynL[hotbinB] += 1; if (bynL[hotbinB] > 982) cutbinsL = true;
			totalB += (301-bynL[1]); totalB += (176-bynL[2]); totalB += (125-bynL[3]);
			totalB += (97-bynL[4]); totalB += (79-bynL[5]); totalB += (67-bynL[6]);
			totalB += (58-bynL[7]); totalB += (51-bynL[8]); totalB += (46-bynL[9]);
			bynL[hotbinB] -= 1;
		} else hotbinB = 10;
		//produce total number- smaller is closer to Benford real
		
		double outputSample;
		if (totalA < totalB) {bynL[hotbinA] += 1; outputSample = floor(inputSampleL);}
		else {bynL[hotbinB] += 1; outputSample = floor(inputSampleL+1);}
		//assign the relevant one to the delay line
		//and floor/ceil signal accordingly
		if (cutbinsL) {
			bynL[1] *= 0.99; bynL[2] *= 0.99; bynL[3] *= 0.99; bynL[4] *= 0.99; bynL[5] *= 0.99; 
			bynL[6] *= 0.99; bynL[7] *= 0.99; bynL[8] *= 0.99; bynL[9] *= 0.99; bynL[10] *= 0.99; 
		}
		noiseShapingL += outputSample - drySampleL;			
		if (noiseShapingL > fabs(inputSampleL)) noiseShapingL = fabs(inputSampleL);
		if (noiseShapingL < -fabs(inputSampleL)) noiseShapingL = -fabs(inputSampleL);		
		
		outputSample /= scale_factor;
		if (outputSample > 1.0) outputSample = 1.0;
		if (outputSample < -1.0) outputSample = -1.0;

        sample = (float)(inputSampleL + outputSample);
    }

    // Delegates to njad left or right, which each have their own state
    inline void njad(float &sample, int chan_num) {
        if (chan_num == 0) {
            njad_l(sample);
        } else if (chan_num == 1) {
            njad_r(sample);
        }
    }

    inline long myround(float x)
    {
        return static_cast<long>(x + (x >= 0 ? 0.5f : -0.5f));
    }

    template<typename T>
    struct id { typedef T type; };

    template<typename T>
    inline T clip(
        typename id<T>::type min,
        T v,
        typename id<T>::type max)
    {
        if (v < min) return min;
        if (v > max) return max;
        return v;
    }

    inline void write_intel(unsigned char * ptr, unsigned long word)
    {
        ptr[0] =  word        & 0xFF;
        ptr[1] = (word >>  8) & 0xFF;
        if (highres) {
            ptr[2] = (word >> 16) & 0xFF;
        }
    }

    inline void tpdf(float &sample)
    {
		sample *= scale_factor;
        float rand1 = ((float) rand()) / ((float) RAND_MAX); // rand value between 0 and 1
    	float rand2 = ((float) rand()) / ((float) RAND_MAX); // rand value between 0 and 1
        sample += (rand1 - rand2);
    }

} // anonymous namespace

using namespace std;

int main(int argc, char *argv[])
{
    const int blockSize = 4096;
    int channelsNum   = -1;
    int lsbitfirst = -1;
    int bits       = -1;
    int interleaved = -1;
    string infileName = "";

    if (argc==6) {
        if ('1' <= argv[1][0] && argv[1][0] <= '9')
            channelsNum = 1 + (argv[1][0] - '1');
        if (argv[2][0] == 'm' || argv[2][0] == 'M') lsbitfirst = 0;
        if (argv[2][0] == 'l' || argv[2][0] == 'L') lsbitfirst = 1;
        if (!strcmp(argv[3], "16")) bits = 16;
        if (!strcmp(argv[3], "24")) bits = 24;
        if (argv[4][0] == 'i' || argv[4][0] == 'I') interleaved = 1;
        if (argv[4][0] == 'p' || argv[4][0] == 'P') interleaved = 0;
        infileName = argv[5];
    }
    if (channelsNum < 1 || lsbitfirst < 0 || bits < 0 || interleaved < 0) {
        cerr << "\n"
            "DSD2PCM filter (raw DSD64 --> 352 kHz raw PCM)\n"
            "(c) 2009 Sebastian Gesemann\n\n"
            "Syntax: dsd2pcm <channels> <bitorder> <bitdepth> <format> <infile>\n"
            "channels = 1,2,3,...,9 (number of channels in DSD stream)\n"
            "bitorder = L (lsb first), M (msb first) (DSD stream option)\n"
            "bitdepth = 16 or 24 (intel byte order, output option)\n"
            "format = I (interleaved) or P (planar)\n"
            "infile = Input file name, containing raw dsd with either \n"
            "planar format and 4096 byte block size,\n"
            "or interleaved with 1 byte per channel.\n\n"
            "Note: At 16 bits/sample a noise shaper kicks in that can preserve\n"
            "a dynamic range of 135 dB below 30 kHz.\n\n"
            "Outputs raw pcm to file named 'out.pcm'\n\n";
        return 1;
    }

    // Seed rng
    srand (static_cast <unsigned> (time(0)));

    ofstream outFile;
    ifstream inFile;
    outFile.open("out.pcm", ios::out | ios::app | ios::binary);
    inFile.open(infileName, ios::binary | ios::in);

    int bytespersample = bits / 8;
    vector<dxd> dxds (channelsNum);
    vector<noise_shaper> ns;

    if (bits == 16) {
        init_outputs(false);
    } else {
        init_outputs(true);
    }

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
                njad(r, c);
                //tpdf(r);
                long smp = clip(-scale_factor, myround(r), scale_factor);
                write_intel(out, smp);
                out += channelsNum * bytespersample;
            }
        }
        outFile.write(pcmOut, blockSize * channelsNum * bytespersample);
    }

    outFile.close();
    inFile.close();
}