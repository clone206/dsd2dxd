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

#include <stdlib.h>
#include <string.h>

#include "dsd2pcm.h"

static const unsigned char bitreverse[] =
    {
        0x00, 0x80, 0x40, 0xC0, 0x20, 0xA0, 0x60, 0xE0, 0x10, 0x90, 0x50, 0xD0, 0x30, 0xB0, 0x70, 0xF0,
        0x08, 0x88, 0x48, 0xC8, 0x28, 0xA8, 0x68, 0xE8, 0x18, 0x98, 0x58, 0xD8, 0x38, 0xB8, 0x78, 0xF8,
        0x04, 0x84, 0x44, 0xC4, 0x24, 0xA4, 0x64, 0xE4, 0x14, 0x94, 0x54, 0xD4, 0x34, 0xB4, 0x74, 0xF4,
        0x0C, 0x8C, 0x4C, 0xCC, 0x2C, 0xAC, 0x6C, 0xEC, 0x1C, 0x9C, 0x5C, 0xDC, 0x3C, 0xBC, 0x7C, 0xFC,
        0x02, 0x82, 0x42, 0xC2, 0x22, 0xA2, 0x62, 0xE2, 0x12, 0x92, 0x52, 0xD2, 0x32, 0xB2, 0x72, 0xF2,
        0x0A, 0x8A, 0x4A, 0xCA, 0x2A, 0xAA, 0x6A, 0xEA, 0x1A, 0x9A, 0x5A, 0xDA, 0x3A, 0xBA, 0x7A, 0xFA,
        0x06, 0x86, 0x46, 0xC6, 0x26, 0xA6, 0x66, 0xE6, 0x16, 0x96, 0x56, 0xD6, 0x36, 0xB6, 0x76, 0xF6,
        0x0E, 0x8E, 0x4E, 0xCE, 0x2E, 0xAE, 0x6E, 0xEE, 0x1E, 0x9E, 0x5E, 0xDE, 0x3E, 0xBE, 0x7E, 0xFE,
        0x01, 0x81, 0x41, 0xC1, 0x21, 0xA1, 0x61, 0xE1, 0x11, 0x91, 0x51, 0xD1, 0x31, 0xB1, 0x71, 0xF1,
        0x09, 0x89, 0x49, 0xC9, 0x29, 0xA9, 0x69, 0xE9, 0x19, 0x99, 0x59, 0xD9, 0x39, 0xB9, 0x79, 0xF9,
        0x05, 0x85, 0x45, 0xC5, 0x25, 0xA5, 0x65, 0xE5, 0x15, 0x95, 0x55, 0xD5, 0x35, 0xB5, 0x75, 0xF5,
        0x0D, 0x8D, 0x4D, 0xCD, 0x2D, 0xAD, 0x6D, 0xED, 0x1D, 0x9D, 0x5D, 0xDD, 0x3D, 0xBD, 0x7D, 0xFD,
        0x03, 0x83, 0x43, 0xC3, 0x23, 0xA3, 0x63, 0xE3, 0x13, 0x93, 0x53, 0xD3, 0x33, 0xB3, 0x73, 0xF3,
        0x0B, 0x8B, 0x4B, 0xCB, 0x2B, 0xAB, 0x6B, 0xEB, 0x1B, 0x9B, 0x5B, 0xDB, 0x3B, 0xBB, 0x7B, 0xFB,
        0x07, 0x87, 0x47, 0xC7, 0x27, 0xA7, 0x67, 0xE7, 0x17, 0x97, 0x57, 0xD7, 0x37, 0xB7, 0x77, 0xF7,
        0x0F, 0x8F, 0x4F, 0xCF, 0x2F, 0xAF, 0x6F, 0xEF, 0x1F, 0x9F, 0x5F, 0xDF, 0x3F, 0xBF, 0x7F, 0xFF};

static void precalc(dsd2pcm_ctx *ctx, const double *htaps, int numCoeffs, int lsbf)
{
    int t, dsdSeq, bit, k;
    double acc;

    for (t = 0; t < ctx->numTables; ++t)
    {
        k = numCoeffs - t * 8;

        if (k > 8)
            k = 8;

        for (dsdSeq = 0; dsdSeq < 256; ++dsdSeq)
        {
            acc = 0.0;
            for (bit = 0; bit < k; ++bit)
            {
                if (lsbf)
                    acc += (((dsdSeq >> (bit)) & 1) * 2 - 1) * htaps[t * 8 + bit];
                else
                    acc += (((dsdSeq >> (7 - bit)) & 1) * 2 - 1) * htaps[t * 8 + bit];
            }
            ctx->ctables[ctx->numTables - 1 - t][dsdSeq] = acc;
        }
    }
}

extern dsd2pcm_ctx *dsd2pcm_init(char filtType, int lsbf, int decimation, int dsdRate)
{
    dsd2pcm_ctx *ptr;
    ptr = (dsd2pcm_ctx *)malloc(sizeof(dsd2pcm_ctx));

    if (ptr)
    {
        int numCoeffs;
        const double *htaps;
        int err = 0;

        if (dsdRate == 2)
        {
            if (decimation == 16)
            {
                switch (filtType)
                {
                case 'X':
                case 'x':
                case 'c':
                case 'C':
                    numCoeffs = 450;
                    htaps = htaps_ddr_16to1_cheb;
                    ptr->decimation = 16;
                    ptr->delay = 6;
                    break;
                case 'D':
                case 'd':
                case 'e':
                case 'E':
                    numCoeffs = 220;
                    htaps = htaps_ddr_16to1_eq;
                    ptr->decimation = 16;
                    ptr->delay = 6;
                    break;
                default:
                    err = 1;
                }
            }
            else if (decimation == 32)
            {
                switch (filtType)
                {
                case 'X':
                case 'x':
                case 'c':
                case 'C':
                    numCoeffs = 750;
                    htaps = htaps_ddr_32to1_cheb;
                    ptr->decimation = 32;
                    ptr->delay = 8;
                    break;
                case 'D':
                case 'd':
                case 'e':
                case 'E':
                    numCoeffs = 250;
                    htaps = htaps_ddr_32to1_eq;
                    ptr->decimation = 32;
                    ptr->delay = 8;
                    break;
                default:
                    err = 1;
                }
            }
        }
        else if (decimation == 8)
        {
            if (filtType == 'X' || filtType == 'x')
            {
                numCoeffs = 56;
                htaps = htaps_xld;
                ptr->decimation = 8;
                ptr->delay = 6;
            }
            else if (filtType == 'D' || filtType == 'd')
            {
                numCoeffs = 48;
                htaps = htaps_d2p;
                ptr->decimation = 8;
                ptr->delay = 0;
            }
            else
            {
                err = 1;
            }
        }
        else if (decimation == 16)
        {
            numCoeffs = 112;
            htaps = htaps_16to1_xld;
            ptr->decimation = 16;
            ptr->delay = 6;
        }
        else if (decimation == 32)
        {
            numCoeffs = 288;
            htaps = htaps_32to1;
            ptr->decimation = 32;
            ptr->delay = 8;
        }
        else
        {
            err = 1;
        }

        if (err)
        {
            return NULL;
        }

        ptr->numTables = (numCoeffs + 7) / 8;
        ptr->lsbfirst = lsbf;
        ptr->ctables = (double **)malloc(sizeof(double *) * ptr->numTables);

        for (int i = 0; i < ptr->numTables; ++i)
        {
            ptr->ctables[i] = (double *)malloc(sizeof(double) * 256);
        }

        precalc(ptr, htaps, numCoeffs, lsbf);
        dsd2pcm_reset(ptr);
    }

    return ptr;
}

extern void dsd2pcm_destroy(dsd2pcm_ctx *ptr)
{
    free(ptr);
}

extern dsd2pcm_ctx *dsd2pcm_clone(dsd2pcm_ctx *ptr)
{
    dsd2pcm_ctx *p2;
    p2 = (dsd2pcm_ctx *)malloc(sizeof(dsd2pcm_ctx));

    if (p2)
    {
        memcpy(p2, ptr, sizeof(dsd2pcm_ctx));
    }
    return p2;
}

extern void dsd2pcm_reset(dsd2pcm_ctx *ptr)
{
    int i;
    for (i = 0; i < FIFOSIZE; ++i)
        ptr->fifo[i] = 0x69; /* my favorite silence pattern */
    ptr->fifopos = 0;
    /* 0x69 = 01101001
     * This pattern "on repeat" makes a low energy 352.8 kHz tone
     * and a high energy 1.0584 MHz tone which should be filtered
     * out completely by any playback system --> silence
     */
    ptr->delay2 = ptr->delay;
}

extern void dsd2pcm_translate(
    dsd2pcm_ctx *handle, size_t blockSize,
    const unsigned char *dsdData, ptrdiff_t dsdStride,
    int lsbf, double *floatData, ptrdiff_t floatStride, int decimation)
{
    unsigned fifoPos, i, bite1, bite2;
    unsigned char *p;
    double acc;
    unsigned int out = decimation / 8;
    fifoPos = handle->fifopos;
    int numTables = handle->numTables;

    while (blockSize-- > 0)
    {
        bite1 = *dsdData & 0xFFu;
        handle->fifo[fifoPos] = bite1;
        dsdData += dsdStride;

        p = handle->fifo + ((fifoPos - numTables) & FIFOMASK);
        *p = bitreverse[*p & 0xFF];

        if (!--out)
        {
            out = decimation / 8;
            acc = 0.0;
            for (i = 0; i < numTables; ++i)
            {
                bite1 = handle->fifo[(fifoPos - i) & FIFOMASK] & 0xFF;
                bite2 = handle->fifo[(fifoPos - (numTables * 2 - 1) + i) & FIFOMASK] & 0xFF;
                acc += handle->ctables[i][bite1] + handle->ctables[i][bite2];
            }

            if (handle->delay2)
                handle->delay2--;
            else
            {
                *floatData = acc;
                floatData += floatStride;
            }
        }
        fifoPos = (fifoPos + 1) & FIFOMASK;
    }

    handle->fifopos = fifoPos;
}