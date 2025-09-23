pub struct FirConvolve {
    coefficients: Vec<f64>, // Filter impulse response coefficients
    state: Vec<f64>,        // Circular buffer for past input samples
    state_ptr: usize,       // Pointer to the current position in the circular buffer
}

impl FirConvolve {
    pub fn new(second_half_taps: &[f64]) -> Self {
        let num_taps = second_half_taps.len() * 2; // Double the size of half the number of taps
        let mut first_half_taps = second_half_taps.to_vec();
        first_half_taps.reverse();
        let full_taps = [first_half_taps, second_half_taps.to_vec()].concat();

        FirConvolve {
            coefficients: full_taps,
            state: vec![0.0; num_taps], // Initialize state with zeros
            state_ptr: 0,
        }
    }

    pub fn process_sample(&mut self, input_sample: f64) -> f64 {
        // Store the new input sample in the circular buffer
        self.state[self.state_ptr] = input_sample;

        // Calculate the output by convolving coefficients with the state
        let mut output = 0.0;
        for i in 0..self.coefficients.len() {
            let state_index = (self.state_ptr + self.coefficients.len() - i) % self.coefficients.len();
            output += self.coefficients[i] * self.state[state_index];
        }

        // Advance the state pointer
        self.state_ptr = (self.state_ptr + 1) % self.coefficients.len();

        output
    }

    // A method to process a block of samples could also be implemented for efficiency
    // pub fn process_block(&mut self, input_block: &[f64], output_block: &mut [f64]) { /* ... */ }
}