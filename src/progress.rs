use std::sync::Arc;

/// Describes the current stage of archive finalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStage {
    /// Reading input files from disk into memory blocks.
    Reading,
    /// Compressing blocks in parallel and writing them to the output.
    CompressingAndWriting,
    /// Writing the final header and signature (nearly instant).
    Finalizing,
}

/// Progress information passed to the callback during `finish_with_progress`.
#[derive(Debug, Clone)]
pub struct ProgressInfo {
    /// Overall progress between 0.0 and 1.0.
    pub percentage: f64,
    /// Current stage.
    pub stage: ProgressStage,
    /// Blocks completed so far (meaningful during CompressingAndWriting stage).
    pub blocks_completed: usize,
    /// Total number of blocks.
    pub blocks_total: usize,
}

pub(crate) type ProgressCallback = Arc<dyn Fn(&ProgressInfo) + Send + Sync>;

pub(crate) fn report(
    progress: &Option<ProgressCallback>,
    stage: ProgressStage,
    percentage: f64,
    blocks_completed: usize,
    blocks_total: usize,
) {
    if let Some(cb) = progress {
        cb(&ProgressInfo {
            percentage,
            stage,
            blocks_completed,
            blocks_total,
        });
    }
}
