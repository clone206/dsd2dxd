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

#ifndef DSD2PCM_H_INCLUDED
#define DSD2PCM_H_INCLUDED

#include <stddef.h>
#include <string.h>

#ifdef __cplusplus
extern "C"
{
#endif

#define FIFOSIZE 128            /* must be a power of two */
#define FIFOMASK (FIFOSIZE - 1) /* bit mask for FIFO offsets */

   /*
    * Properties of this 96-tap lowpass filter when applied on a signal
    * with sampling rate of 44100*64 Hz:
    *
    * () has a delay of 17 microseconds.
    *
    * () flat response up to 48 kHz
    *
    * () if you downsample afterwards by a factor of 8, the
    *    spectrum below 70 kHz is practically alias-free.
    *
    * () stopband rejection is about 160 dB
    *
    * The coefficient tables ("ctables") take only 6 Kibi Bytes and
    * should fit into a modern processor's fast cache.
    */

   /*
    * The 2nd half (48 coeffs) of a 96-tap symmetric lowpass filter
    */
   static const double htaps_d2p[48] = {
       0.09950731974056658,
       0.09562845727714668,
       0.08819647126516944,
       0.07782552527068175,
       0.06534876523171299,
       0.05172629311427257,
       0.0379429484910187,
       0.02490921351762261,
       0.0133774746265897,
       0.003883043418804416,
       -0.003284703416210726,
       -0.008080250212687497,
       -0.01067241812471033,
       -0.01139427235000863,
       -0.0106813877974587,
       -0.009007905078766049,
       -0.006828859761015335,
       -0.004535184322001496,
       -0.002425035959059578,
       -0.0006922187080790708,
       0.0005700762133516592,
       0.001353838005269448,
       0.001713709169690937,
       0.001742046839472948,
       0.001545601648013235,
       0.001226696225277855,
       0.0008704322683580222,
       0.0005381636200535649,
       0.000266446345425276,
       7.002968738383528e-05,
       -5.279407053811266e-05,
       -0.0001140625650874684,
       -0.0001304796361231895,
       -0.0001189970287491285,
       -9.396247155265073e-05,
       -6.577634378272832e-05,
       -4.07492895872535e-05,
       -2.17407957554587e-05,
       -9.163058931391722e-06,
       -2.017460145032201e-06,
       1.249721855219005e-06,
       2.166655190537392e-06,
       1.930520892991082e-06,
       1.319400334374195e-06,
       7.410039764949091e-07,
       3.423230509967409e-07,
       1.244182214744588e-07,
       3.130441005359396e-08};

   static const double htaps_xld[56] = {
       +1.003599032761268994e-01,
       +9.662981560884242871e-02,
       +8.945349213182296477e-02,
       +7.937003090599190069e-02,
       +6.711999843322885573e-02,
       +5.357287880424892179e-02,
       +3.964424879125778151e-02,
       +2.621214413106798952e-02,
       +1.404181427224378970e-02,
       +3.726638223919860951e-03,
       -4.349367691769268421e-03,
       -1.002491923722929022e-02,
       -1.335121721731015974e-02,
       -1.456134879100988953e-02,
       -1.402473061802538001e-02,
       -1.219324909388730047e-02,
       -9.546051479083451571e-03,
       -6.539324989916536768e-03,
       -3.566042373069515121e-03,
       -9.288141542618654628e-04,
       +1.173027324018304039e-03,
       +2.642883925836397949e-03,
       +3.475194700261226202e-03,
       +3.735904314188005001e-03,
       +3.538838154561203993e-03,
       +3.021587525576849027e-03,
       +2.323915921681954030e-03,
       +1.570806204643151061e-03,
       +8.612537902079315079e-04,
       +2.629381379992477125e-04,
       -1.878960137078334030e-04,
       -4.825621199727765783e-04,
       -6.335370394217578393e-04,
       -6.670230543688062102e-04,
       -6.157309569032056416e-04,
       -5.128053356239906969e-04,
       -3.874181080355335862e-04,
       -2.621813014123864261e-04,
       -1.522269998707380064e-04,
       -6.560081853307215519e-05,
       -4.520812542841012188e-06,
       +3.294602930918357880e-05,
       +5.116792066565596268e-05,
       +5.546209091390614230e-05,
       +5.098758042924450833e-05,
       +4.205866389020815099e-05,
       +3.183958589294015248e-05,
       +2.233259319611928908e-05,
       +1.455336859409994022e-05,
       +8.794181965752857413e-06,
       +4.896565360135651024e-06,
       +2.483029535958191067e-06,
       +1.124189775289309014e-06,
       +4.387878673693896045e-07,
       +1.376777283954879050e-07,
       +2.904117154225673850e-08,
   };

   /*
    * The 2nd half (112 coeffs) of a 224-tap symmetric lowpass filter
    *  - flat response up to ~35 kHz
    *  - alias free below ~35 kHz
    *  - stopband rejection ~-160 dB
    */
   static double htaps_16to1_xld[112] = {
       +5.760462813961340911e-02, +5.691757911729226210e-02, +5.555982934698910219e-02, +5.356356970757100017e-02, +5.097584071644974124e-02, +4.785708826560070711e-02, +4.427932021228758730e-02, +4.032393001491144102e-02, +3.607926497084759654e-02, +3.163802486407806674e-02, +2.709458166634984982e-02, +2.254231220535396138e-02, +1.807103341873207009e-02, +1.376462411228964074e-02, +9.698908328714049912e-03, +5.939863928921530062e-03,
       +2.542206329958864031e-03, -4.516178595725602886e-04, -3.012008656370503135e-03, -5.122359614640761838e-03, -6.778787301343743021e-03, -7.989471529052894969e-03, -8.773647602567644088e-03, -9.160302763993922320e-03, -9.186637450460090337e-03, -8.896357660427813702e-03, -8.337867548688329689e-03, -7.562431350066944717e-03, -6.622370993341054196e-03, -5.569360561977704585e-03, -4.452871427796344095e-03, -3.318812840644432948e-03,
       -2.208402475303640131e-03, -1.157290413781186019e-03, -1.949487751238639024e-04, +6.556718097281456770e-04, +1.378228000447144063e-03, +1.962830183962950819e-03, +2.405675727112777888e-03, +2.708501415055656822e-03, +2.877891852193870201e-03, +2.924483152906128154e-03, +2.862101978458481914e-03, +2.706878996383537978e-03, +2.476373319315937002e-03, +2.188740653069128940e-03, +1.861973022276113086e-03, +1.513232339191010987e-03,
       +1.158294059628507027e-03, +8.111110219952220434e-04, +4.835015649465252210e-04, +1.849604310259179106e-04, -7.741400027371707175e-05, -2.988867964233316867e-04, -4.769618051506617756e-04, -6.111814994833964041e-04, -7.028689690242875083e-04, -7.548301317186775859e-04, -7.710338230080065417e-04, -7.562862630242533132e-04, -7.159146274091129480e-04, -6.554722205113200752e-04, -5.804752203233431215e-04, -4.961782711053265614e-04,
       -4.073935045445318940e-04, -3.183550094944409157e-04, -2.326284177570372112e-04, -1.530632418997455091e-04, -8.178395736251813184e-05, -2.021457860897648995e-05, +3.086935161782307266e-05, +7.126786445358902723e-05, +1.012693221865872058e-04, +1.215551028696340054e-04, +1.331019332169323043e-04, +1.370868574255101988e-04, +1.347989395314041930e-04, +1.275608199228540093e-04, +1.166622847621144983e-04, +1.033070589774495030e-04,
       +8.857317092942181480e-05, +7.338651153573718159e-05, +5.850660458614138270e-05, +4.452313192296150120e-05, +3.186146077678470723e-05, +2.079524623147637055e-05, +1.146416398967912983e-05, +3.894913917889374591e-06, -1.976237783286506113e-06, -6.280878589462072164e-06, -9.197003076568571641e-06, -1.092962463456785992e-05, -1.169410169367897026e-05, -1.170252663016168039e-05, -1.115328413493569935e-05, -1.022369729486433082e-05,
       -9.065501913715266297e-06, -7.802825857677858607e-06, -6.532217683647328614e-06, -5.324223752151299107e-06, -4.226118520432855354e-06, -3.265375069810791995e-06, -2.453390583768283789e-06, -1.789251932048668056e-06, -1.263325393243766064e-06, -8.603805222502517747e-07, -5.622136294265281904e-07, -3.498174364819740902e-07, -2.048713691800104068e-07, -1.108150375211134017e-07, -5.349252062858066909e-08, -2.369569955628185962e-08};

   struct dsd2pcm_ctx_s
   {
      unsigned char fifo[FIFOSIZE];
      unsigned fifopos;
      unsigned int numTables;
      float **ctables;
      int decimation;
      int lsbfirst;
      int delay;
      int delay2;
      int (*translate)(struct dsd2pcm_ctx_s *, size_t, const unsigned char *, ptrdiff_t, float *, ptrdiff_t);
      int (*finalize)(struct dsd2pcm_ctx_s *, float *, ptrdiff_t);
   };

   typedef struct dsd2pcm_ctx_s dsd2pcm_ctx;

   /**
    * initializes a "dsd2pcm engine" for one channel
    * (precomputes tables and allocates memory)
    *
    * This is the only function that is not thread-safe in terms of the
    * POSIX thread-safety definition because it modifies global state
    * (lookup tables are computed during the first call)
    */
   extern dsd2pcm_ctx *dsd2pcm_init(char filtType, int lsbf, int decimation);

   /**
    * deinitializes a "dsd2pcm engine"
    * (releases memory, don't forget!)
    */
   extern void dsd2pcm_destroy(dsd2pcm_ctx *ctx);

   /**
    * clones the context and returns a pointer to the
    * newly allocated copy
    */
   extern dsd2pcm_ctx *dsd2pcm_clone(dsd2pcm_ctx *ctx);

   /**
    * resets the internal state for a fresh new stream
    */
   extern void dsd2pcm_reset(dsd2pcm_ctx *ctx);

   /**
    * "translates" a stream of octets to a stream of floats
    * (8:1 decimation)
    * @param handle -- pointer to abstract context (buffers)
    * @param blockSize -- number of octets/samples to "translate"
    * @param dsdData -- pointer to first octet (input)
    * @param dsdStride -- src pointer increment
    * @param lsbitfirst -- bitorder, 0=msb first, 1=lsbfirst
    * @param floatData -- pointer to first float (output)
    * @param floatStride -- dst pointer increment
    * @param decimation -- decimation ration (to 1)
    */
   extern void dsd2pcm_translate(dsd2pcm_ctx *handle, size_t blockSize,
                                 const unsigned char *dsdData, ptrdiff_t dsdStride, int lsbitfirst,
                                 double *floatData, ptrdiff_t floatStride, int decimation);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* include guard DSD2PCM_H_INCLUDED */
