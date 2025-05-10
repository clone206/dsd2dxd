/* ========================================
 *  NotJustAnotherDither - NotJustAnotherDither.h
 *  Copyright (c) 2016 airwindows, Airwindows uses the MIT license
 * ======================================== */

#pragma once

#include <math.h>
#include <cstring>
#include <ctype.h>
#include <typeinfo>
#include <cstdint>
#include <time.h>
#include <iostream>

using namespace std;

struct Dither
{
    double noise_shaping_l; // Noise shape state L
    double noise_shaping_r; // Noise shape state R
    double byn_l[13];       // Benford Real number weights L
    double byn_r[13];       // Benford Real number weights R
    uint32_t fpd;           // Floating-point dither

    char type;

    inline void processSamp(double &sample, int chanNum);
    inline void fpdither(double &sample);
    inline void njad(double &sample, int chanNum);
    inline void tpdf(double &sample);
    inline void rect(double &sample);

    Dither() {}

    Dither(char ditherTypeOut)
    {
        type = tolower(ditherTypeOut);

        if (type != 'n' && type != 't' && type != 'f' && type != 'x' && type != 'r')
        {
            throw "Invalid dither type!";
        }
    }

    void init()
    {
        if (type != 'x')
        {
            // Seed rng
            srand(static_cast<unsigned>(time(0)));
        }

        if (type == 'n')
        {
            initOutputs();
        }
        else if (type == 'f')
        {
            initRand();
        }
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
};

inline void Dither::processSamp(double &sample, int chanNum)
{
    if (type == 'n')
    {
        njad(sample, chanNum);
    }
    else if (type == 't')
    {
        tpdf(sample);
    }
    else if (type == 'f')
    {
        fpdither(sample);
    }
    else if (type == 'r')
    {
        rect(sample);
    }
}
// Floating point dither for going from double to float
// Part of Airwindows plugin suite
inline void Dither::fpdither(double &inputSample)
{
    int expon;
    frexpf((float)inputSample, &expon);
    fpd ^= fpd << 13;
    fpd ^= fpd >> 17;
    fpd ^= fpd << 5;
    inputSample += (fpd * 3.4e-36l * pow(2, expon + 62)); // removed 'blend' for real use, it's for the demo;

    inputSample = (float)inputSample; // equivalent of 'floor' for 32 bit floating point
}

// Not Just Another Dither
// Not truly random. Uses Benford Real Numbers for the dither values
// Part of Airwindows plugin suite
inline void Dither::njad(double &inputSample, int chanNum)
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

// TPDF dither
inline void Dither::tpdf(double &sample)
{
    double rand1 = ((double)rand()) / ((double)RAND_MAX) / 2; // rand value between 0 and 0.5
    double rand2 = ((double)rand()) / ((double)RAND_MAX) / 2; // rand value between 0 and 0.5
    sample += (rand1 - rand2); // Range from -0.5 to +0.5
}

// Rectangular dither
inline void Dither::rect(double &sample)
{
    double rand1 = ((double)rand()) / ((double)RAND_MAX); // rand value between 0 and 1
    sample += (rand1 - 0.5); // Range from -0.5 to +0.5
}