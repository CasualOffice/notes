//! # media-pipeline (later-phase stub)
//!
//! Will implement the DSP pipeline of **HLD §4/§8.4** and **Architecture §5**:
//! ring→16 kHz mono f32, DC/gain, VAD, overlapping chunking, drift correction.
//! No alloc/lock on the RT audio callback (N13). Stub only in Phase 1.
