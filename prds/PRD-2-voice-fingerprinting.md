# PRD 2: Voice Fingerprinting & Onboarding Integration

## 1. Overview
This PRD defines the extension of the Rust/WASM VAD and Diarization module (from PRD 1) to support Voice Fingerprinting. This allows the application to "remember" a specific user's voice through an onboarding process. During subsequent diarization sessions, the system will identify the saved voice and assign the correct name (e.g., "Alice") instead of a generic ID ("Speaker 1").

## 2. Core Concepts
*   **Voice Profile (Fingerprint)**: A mathematical representation of a person's voice, derived by averaging multiple speaker embeddings extracted during the onboarding phase.
*   **Centroid**: The mean vector of the voice fingerprint.
*   **Similarity Threshold**: A minimum cosine similarity score (e.g., `0.75`) required to definitively identify a real-time voice segment as belonging to a known profile.

## 3. Architecture & Data Flow
### A. Onboarding Phase (Recording the Fingerprint)
1.  The UI prompts the user to read a specific calibration phrase (approx. 5-10 seconds of speech).
2.  JavaScript initiates the process via `engine.start_onboarding("User_Name")`.
3.  The WASM module relies on the VAD to isolate actual speech from the calibration phrase.
4.  WASM slices the speech into 1-second overlapping chunks and extracts embeddings for each using the ONNX model.
5.  Rust calculates the centroid (average) of these embeddings, creating a stable voice profile.
6.  JS calls `engine.finish_onboarding()`, which returns the `VoiceProfile` (a serialized float array + ID).
7.  JS securely saves this profile to the browser's `IndexedDB`.

### B. Recognition Phase (Active Diarization)
1.  On application startup, JS loads all saved `VoiceProfile`s from `IndexedDB` and injects them into the WASM module via `engine.load_profiles(profiles_json)`.
2.  During active recording, when a new speech segment is processed and its embedding is extracted, the clustering algorithm intercepts standard AHC.
3.  Rust computes the cosine similarity between the new embedding and all known `VoiceProfile` centroids.
4.  If `max(similarity) > THRESHOLD`, the segment is assigned the known profile's ID.
5.  If `max(similarity) <= THRESHOLD`, the segment falls back to standard AHC clustering and is assigned a generic ID.

## 4. Proposed WASM API Extensions
```rust
#[wasm_bindgen]
impl DiarizationEngine {
    // Starts an onboarding session for a specific user ID
    pub fn start_onboarding(&mut self, profile_id: &str);
    
    // Concludes onboarding and returns the serialized fingerprint vector (JSON)
    pub fn finish_onboarding(&mut self) -> String; 
    
    // Loads previously saved fingerprints from IndexedDB into the active session
    pub fn load_profiles(&mut self, profiles_json: &str);
    
    // Fine-tune the similarity threshold for strict/loose matching (e.g., 0.75)
    pub fn set_recognition_threshold(&mut self, threshold: f32);
}
```

## 5. Storage & Privacy Strategy
*   **No Raw Audio Storage**: Ensure only mathematical embeddings (float arrays) are stored in `IndexedDB`. Raw audio is immediately discarded after processing.
*   **On-Device Processing**: Because feature extraction and matching happen locally in WASM, biometric voice data never leaves the mobile device, satisfying strict privacy requirements.

## 6. Implementation Steps for Antigravity Agent
1.  **State Management**: Add a `profiles` HashMap to the `DiarizationEngine` struct in Rust to store known centroids.
2.  **Onboarding Logic**: Update `process_audio_chunk`. If `is_onboarding` is active, bypass normal clustering and instead append the extracted vectors to an internal onboarding buffer.
3.  **Math Utilities**: Implement vector averaging and L2 normalization functions in Rust to compute centroids.
4.  **Similarity Gate (Recognition)**: Update the AHC clustering loop. Before assigning a new cluster or joining an existing one, run a `find_best_match(known_profiles)` check.
5.  **IndexedDB Integration**: Write a TypeScript wrapper using `idb` to persist fingerprints locally and reload them automatically on page load.
