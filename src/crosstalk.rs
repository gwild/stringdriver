use nalgebra::{DMatrix, DVector};
use gethostname::gethostname;

pub struct CrosstalkFilter {
    matrix_inv: DMatrix<f32>,
    num_channels: usize,
}

pub fn load_crosstalk() -> Option<Vec<Vec<f32>>> {
    // Use config_loader as single source of truth
    let host = gethostname().to_string_lossy().to_string();
    log::debug!("Looking for crosstalk data for host: {}", host);
    
    let matrix = crate::config_loader::load_crosstalk_matrix(&host);
    
    if let Some(ref m) = matrix {
        let rows = m.len();
        let cols = if rows > 0 { m[0].len() } else { 0 };
        log::info!(target: "crosstalk::matrix", "Loaded crosstalk matrix: {}x{} (first row: {:?})", rows, cols, m.get(0));
    } else {
        log::debug!("No valid matrix found for host {}", host);
    }
    
    matrix
}

impl CrosstalkFilter {
    pub fn new(matrix: Vec<Vec<f32>>) -> Self {
        let rows = matrix.len();
        let cols = matrix[0].len();
        let data: Vec<f32> = matrix.into_iter().flatten().collect();
        let mat = DMatrix::from_row_slice(rows, cols, &data);
        
        // Add small regularization to avoid instability
        let mut reg = mat.clone();
        for i in 0..rows.min(cols) {
            reg[(i,i)] += 1e-6;
        }
        
        // For non-square or non-invertible matrices, fall back to identity (no demix)
        let inv = if rows == cols {
            reg.try_inverse()
        } else {
            None
        };
        let matrix_inv = match inv {
            Some(m) => m,
            None => {
                log::error!(target: "crosstalk::filter", "Invalid or non-invertible crosstalk matrix ({}x{}). Using identity (pass-through) to avoid crash.", rows, cols);
                DMatrix::identity(rows, rows)
            }
        };
        
        log::debug!("Created filter from {}x{} matrix", rows, cols);
        
        Self { 
            matrix_inv,
            num_channels: rows,
        }
    }

    pub fn channels(&self) -> usize {
        self.num_channels
    }

    pub fn filter(&self, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
        if input.is_empty() {
            return vec![];
        }
        if input.len() != self.num_channels {
            log::error!(target: "crosstalk::filter", "Channel count mismatch: filter={} input={}. Clamping to min for safety.", self.num_channels, input.len());
        }

        let active_channels = std::cmp::min(self.num_channels, input.len());
        let num_samples = (0..active_channels)
            .map(|ch| input[ch].len())
            .min()
            .unwrap_or(0);

        if num_samples == 0 {
            return vec![vec![]; active_channels];
        }

        let mut output = vec![vec![0.0; num_samples]; active_channels];

        // Process each sample up to the shortest channel length
        for i in 0..num_samples {
            let mut y = DVector::zeros(active_channels);
            for ch in 0..active_channels {
                // Extra guard against ragged inputs
                y[ch] = if i < input[ch].len() { input[ch][i] } else { 0.0 };
            }

            // Apply inverse matrix to remove crosstalk (identity becomes pass-through)
            let x = &self.matrix_inv * y;

            for ch in 0..active_channels {
                output[ch][i] = x[ch];
            }
        }

        output
    }
}