# PRD 1: VAD + Speaker Diarization in Rust/WASM

## 1. Overview
The objective is to develop a highly efficient WebAssembly (WASM) module written in Rust that provides real-time Voice Activity Detection (VAD) and Speaker Diarization. Designed for a mobile-focused browser application, this module must operate under strict memory constraints (devices with 4-5 GB RAM usually cap browser tabs at ~500 MB). It will function alongside the browser's native Web Speech API (for Speech-to-Text).

## 2. Architecture & Tech Stack
*   **Core Logic**: Rust compiled to `wasm32-unknown-unknown` via `wasm-bindgen`.
*   **Inference Engine**: ONNX Runtime Web (leveraging WebGPU for acceleration where available, falling back to WASM SIMD). Rust will orchestrate the inference via bindings or JS interoperability.
*   **VAD Model**: Silero VAD (ONNX format). Highly accurate and extremely lightweight (~1 MB).
*   **Diarization/Embedding Model**: A lightweight speaker verification model such as ECAPA-TDNN or a MobileNet-based embedding model (ONNX, ~15-30 MB).
*   **Clustering**: Agglomerative Hierarchical Clustering (AHC) implemented natively in Rust to avoid massive Python/JS dependencies.
*   **Audio Pipeline**: Browser `AudioWorklet` (capturing downsampled 16kHz PCM audio) -> SharedArrayBuffer / WASM Memory -> Rust Processing.

## 3. Resource & Memory Constraints
*   **VRAM / RAM Target**: The WASM module and loaded models must keep their active memory footprint under **200 MB**. 
*   **Garbage Collection Avoidance**: Rust must manage audio ring-buffers statically to prevent JavaScript garbage collection spikes that cause audio dropouts on mobile.

## 4. Data Flow & Integration
1.  **Audio Capture**: `AudioWorklet` captures microphone data, downsamples it to 16kHz mono, and pushes `Float32Array` chunks to the Rust WASM module.
2.  **Voice Activity Detection (Rust)**: Every 30ms, Rust runs the VAD model. It identifies precise `speech_start` and `speech_end` timestamps, ignoring background noise and pauses.
3.  **Embedding Extraction (Rust/WebGPU)**: When a contiguous speech segment completes (or reaches a maximum length of ~3 seconds), the audio chunk is fed to the embedding model to extract a dense float vector (e.g., a 192D x-vector).
4.  **Diarization (Rust)**: The AHC algorithm compares the new vector against existing speaker clusters using Cosine Similarity. The segment is assigned a generic ID (e.g., "Speaker 1").
5.  **JS Synchronization**: Rust emits an event to the JS main thread containing: `{ start_time, end_time, speaker_id }`.
6.  **STT Merging (JS)**: The main thread merges the WASM diarization timestamps with the Web Speech API's transcriptions to render the final dialog:
    *   *Speaker 1: "Hello there."*
    *   *Speaker 2: "Hi, how are you?"*

## 5. Proposed WASM Interface (Rust -> JS)
```rust
#[wasm_bindgen]
pub struct DiarizationEngine { /* internal state */ }

#[wasm_bindgen]
impl DiarizationEngine {
    // Initialize with model weights (loaded via fetch in JS)
    pub fn new(vad_model_bytes: &[u8], embed_model_bytes: &[u8]) -> Self;
    
    // Feed PCM float32 data; returns JSON array of diarized segments
    pub fn process_audio_chunk(&mut self, pcm_data: &[f32]) -> String; 
    
    // Retrieve current cluster state/speakers
    pub fn get_current_clusters(&self) -> String;
    
    // Reset state for a new recording session
    pub fn reset(&mut self);
}
```

## 6. Implementation Steps for Antigravity Agent
1.  **Scaffold Rust/WASM Project**: Initialize `wasm-pack`, setup `wasm-bindgen`, and configure WebGPU/ONNX Runtime Web dependencies.
2.  **Audio Ingestion Buffer**: Create a static lock-free ring buffer in Rust for memory-safe audio chunking.
3.  **Integrate Silero VAD**: Implement the inference loop for Silero on 30ms frames, including thresholding and temporal smoothing.
4.  **Speaker Embeddings**: Route completed VAD segments into the embedding ONNX model to extract x-vectors.
5.  **AHC Clustering Logic**: Implement cosine distance metrics and hierarchical clustering purely in Rust.
6.  **JS Wrapper & STT Sync**: Write a TypeScript class that instantiates the WASM module, manages the Web Speech API, and aligns the text with the speaker timestamps.
