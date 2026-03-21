//! Port of `dash.js/src/mss/`.
//!
//! Microsoft Smooth Streaming support — stubbed for future implementation.
//! Structure mirrors: MssHandler, MssParser, MssFragmentProcessor.

/// MSS handler stub.
#[derive(Clone, Debug, Default)]
pub struct MssHandler { _initialized: bool }
impl MssHandler {
    pub fn new() -> Self { Self::default() }
    pub fn reset(&mut self) { self._initialized = false; }
}

/// MSS parser stub.
#[derive(Clone, Debug, Default)]
pub struct MssParser;
impl MssParser {
    pub fn new() -> Self { Self }
}

/// MSS fragment processor stub.
#[derive(Clone, Debug, Default)]
pub struct MssFragmentProcessor;
