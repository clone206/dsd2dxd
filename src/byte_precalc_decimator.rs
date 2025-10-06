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

// ============================================================================
// BytePrecalcDecimator
// Minimal, safe, idiomatic Rust adaptation of the dsd2pcm precalc() approach,
// used ONLY for dense bipolar DSD bitstreams with straight integer decimation
// (no zero stuffing). You precompute 256-entry tables for 8-bit windows of the
// HALF (right) filter taps. At each decimated output boundary we sum mirrored
// table contributions, reproducing the full linear‑phase FIR result with
// far fewer operations (O(numTables) vs O(taps)).
//
// Assumptions:
// - Filter specified by right-half taps (second_half_taps), even full length = 2 * len.
// - decim is an integer multiple of 8 (16, 32, 64, etc).
// - No zero insertion; feed raw DSD bytes in arrival order.
// - Produces one output per 'decim' input bits (decim/8 bytes).
// ============================================================================

pub struct BytePrecalcDecimator {
    // Precomputed tables: tables[i][byte] gives partial sum for segment i
    tables: Vec<Box<[f64; 256]>>,
    num_tables: usize,
    bytes_per_out: u32,
    fifo: Vec<u8>,
    fifo_pos: usize,
    gain: f64,          // DC normalization (like ptr->gain in C)
    delay_count: u64,   // Countdown before starting to emit
    // Cached for mirror addressing
    table_span: usize,  // num_tables * 2 - 1
}

impl BytePrecalcDecimator {
    /// Build from right-half taps (second_half_taps) and integer decimation factor.
    pub fn new(second_half_taps: &[f64], decim: u32) -> Option<Self> {
        if decim % 8 != 0 { return None; } // requires byte alignment
        let half = second_half_taps.len();
        if half == 0 { return None; }
        // Number of 8-bit windows covering half the filter
        let num_tables = (half + 7) / 8;
        // Precompute 256-entry table for each window (like C precalc)
        let mut tables: Vec<Box<[f64; 256]>> = Vec::with_capacity(num_tables);
        for t in 0..num_tables {
            let base = t * 8;
            let remain = half.saturating_sub(base);
            let k = remain.min(8); // up to 8 taps in this window
            // Table index is reversed order (ctx->numTables-1 - t) in C; we can mimic
            // by pushing and later indexing appropriately. Simpler: store in reverse now.
            let mut arr = Box::new([0.0f64; 256]);
            for dsd_seq in 0..256u16 {
                let mut acc = 0.0;
                for bit in 0..k {
                    let bit_is_one = (dsd_seq >> bit) & 1;
                    // Map 0 -> -1, 1 -> +1
                    let sample = if bit_is_one != 0 { 1.0 } else { -1.0 };
                    acc += sample * second_half_taps[base + bit];
                }
                arr[dsd_seq as usize] = acc;
            }
            tables.push(arr);
        }
        // Reverse to align with C's tableIdx = numTables - 1 - t
        tables.reverse();

        // DC gain normalization: full symmetric FIR gain ≈ 2 * sum(half taps)
        let sum_half: f64 = second_half_taps.iter().copied().sum();
        let full_gain = 2.0 * sum_half;
        let gain = if full_gain != 0.0 { 1.0 / full_gain } else { 1.0 };

        let full_len = (half * 2) as u64;
        let delay = (full_len - 1) / 2;
        Some(Self {
            tables,
            num_tables,
            bytes_per_out: decim / 8,
            fifo: vec![0u8; (num_tables * 2 + 8).next_power_of_two()], // simple ring
            fifo_pos: 0,
            gain,
            delay_count: delay,
            table_span: num_tables * 2 - 1,
        })
    }

    /// Feed a block of DSD bytes; produce decimated PCM outputs.
    /// Returns number of PCM samples written to `out`.
    pub fn process_bytes(&mut self, bytes: &[u8], out: &mut [f64]) -> usize {
        if self.num_tables == 0 || self.bytes_per_out == 0 { return 0; }
        let mask = self.fifo.len() - 1; // fifo len is power-of-two
        let mut byte_count_in_frame = 0u32;
        let mut produced = 0usize;

        for &b in bytes {
            // Push newest byte
            self.fifo[self.fifo_pos & mask] = b;
            self.fifo_pos = (self.fifo_pos + 1) & mask;
            byte_count_in_frame += 1;

            if byte_count_in_frame == self.bytes_per_out {
                byte_count_in_frame = 0;

                // Accumulate mirrored table contributions
                let mut acc = 0.0;
                // Current position is one past last written
                let cur = self.fifo_pos;
                for i in 0..self.num_tables {
                    // Recent window i
                    let idx1 = cur.wrapping_sub(1 + i) & mask;
                    // Mirrored window
                    let idx2 = cur.wrapping_sub(1 + (self.table_span - i)) & mask;
                    let byte1 = self.fifo[idx1];
                    let byte2 = self.fifo[idx2];
                    // Table i corresponds to tables[i]; mirrored byte must be bit-reversed
                    acc += self.tables[i][byte1 as usize] + self.tables[i][bit_reverse_u8(byte2) as usize];
                }

                // Apply normalization
                acc *= self.gain;

                // Handle startup latency (group delay)
                if self.delay_count > 0 {
                    self.delay_count -= 1;
                } else if produced < out.len() {
                    out[produced] = acc;
                    produced += 1;
                }
                if produced == out.len() { break; }
            }
        }
        produced
    }
}

// ADD: missing bit reversal helper (used by BytePrecalcDecimator)
#[inline]
pub fn bit_reverse_u8(mut b: u8) -> u8 {
    // Reverse bits in a byte (branchless, 3 shuffle steps)
    b = (b & 0xF0) >> 4 | (b & 0x0F) << 4;
    b = (b & 0xCC) >> 2 | (b & 0x33) << 2;
    b = (b & 0xAA) >> 1 | (b & 0x55) << 1;
    b
}
