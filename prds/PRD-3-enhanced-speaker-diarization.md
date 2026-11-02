# PRD 3: Enhanced Speaker Diarization (First-Two-Speakers Prior, Pyannote Segmentation 3.0 & VoxBlink2 SimAM-ResNet100 INT8)

## 1. Overview & Problem Statement
While the initial Rust/WASM Voice Activity Detection (VAD) and speaker diarization pipeline (PRD 1 & PRD 2) successfully runs locally in the browser, real-world conversational testing exposed three structural bottlenecks:

1. **Opening-Turn False Rejections (~29% User Likelihood / `sim ≈ 0.445`)**:
   * When the enrolled user speaks first, their opening turn frequently scores around `29%` user likelihood (`cosine_similarity ≈ 0.445` against the `0.52` threshold) and is misclassified as a conversation partner (`Partner 1`).
   * **Root Causes**:
     * *Prosody & Duration Mismatch*: Onboarding calibration records 5 seconds of continuous read prose, whereas opening turns are typically short (`0.5s–1.2s`) spontaneous greetings with higher pitch variance and limited phoneme diversity, skewing utterance-level Cepstral Mean Normalization (`SubtractGlobalMean`).
     * *WebRTC DSP Cold-Start*: Browser `autoGainControl` (AGC) and `noiseSuppression` (NS) adapt aggressively during the first ~1 second of a fresh microphone stream, shifting spectral tilt on the opening utterance.
     * *Memoryless Per-Segment Thresholding*: [`AhcClusterer`](../crates/vad-diarization-wasm/src/clustering.rs) checks each segment against a hard threshold (`0.52` / `0.48`) *before* clustering. Once Turn 1 (`0.445`) fails that gate, it is assigned to `Speaker 1` (`Partner 1`) and is never re-evaluated even after a second speaker (`Speaker 2`) appears with a much lower similarity (`sim ≈ 0.15–0.25`, `~1–3%`).
2. **Fast Turn-Taking & Overlapping Speech Blind Spot**:
   * `silero_vad_v5.onnx` only distinguishes speech from silence (requiring a `352 ms` silence gap to close a segment). Rapid turn-taking (`< 350 ms` pause) or brief overlapping backchannels (*"Ja"*, *"Genau"*) cause Silero VAD to merge both speakers into a single audio segment, producing a blended embedding that matches neither speaker cleanly.
3. **English-Centric Embedding Model**:
   * `wespeaker_en_voxceleb_resnet152_LM.onnx` (~76 MB FP32, ~0.59% EER) is trained on English celebrity interviews (VoxCeleb2), limiting its discriminative margin on short spontaneous German utterances.

---

## 2. Architecture & Core Enhancements

### A. First-Two-Speakers Bayesian Prior & Competitive Cluster Matching
In Begleit-App conversations, the enrolled user is almost always **one of the first two speakers** (`C0` or `C1`). Instead of making an irreversible per-segment threshold decision before clustering, the clustering engine in [`crates/vad-diarization-wasm/src/clustering.rs`](../crates/vad-diarization-wasm/src/clustering.rs) will switch to **cluster-first competitive attribution**:

1. **Unified AHC Clustering First**:
   * Every speech segment—regardless of its raw similarity to the user profile—is clustered into acoustic speaker clusters $C_0, C_1, \dots, C_{K-1}$ using Agglomerative Hierarchical Clustering (AHC).
   * Each cluster $C_k$ maintains a **duration-weighted L2-normalized centroid** $\mu_{C_k}$ and a running user cosine similarity $s_k = \text{sim}(\mu_{C_k}, \mu_{\text{user}})$.
2. **Single-Cluster Opening Prior ($K = 1$)**:
   * While only one acoustic cluster ($C_0$) has been observed in the session, apply an **opening-turn prior**:
     * If $s_0 \ge 0.38$ (corresponding to `~16%` on the unshifted curve, well above the `0.10–0.28` range of a stranger), provisionally classify $C_0$ as `NutzerIn` (`User`) and boost the displayed posterior probability using the two-speaker prior.
3. **Two-Speaker Competitive Posterior & Retroactive Relabeling ($K \ge 2$)**:
   * Once at least two distinct acoustic clusters ($C_0, C_1$) exist, compute the competitive user assignment across clusters:
     $$k^* = \arg\max_{k \in \{0, \dots, K-1\}} \text{sim}(\mu_{C_k}, \mu_{\text{user}})$$
   * Assign `NutzerIn` to cluster $C_{k^*}$ if:
     * $s_{k^*} \ge 0.35$ (minimum sanity floor so two strangers cannot force a match), **and**
     * Either $s_{k^*} \ge 0.50$ **or** the margin over the runner-up cluster $s_{\text{runner-up}}$ satisfies $s_{k^*} - s_{\text{runner-up}} \ge 0.08$.
   * Compute the calibrated user likelihood for each cluster using both its absolute score $s_k$ and the competitive log-odds across the first two clusters:
     $$P(C_k = \text{user} \mid s_0, s_1) = \sigma\!\left(\alpha (s_k - \tau_{\text{prior}}) + \beta (s_k - s_{\text{other}})\right)$$
   * **Retroactive Cluster Relabeling**: If the emergence of $C_1$ (or additional segments in $C_0$) flips the winning user cluster from $C_0 \to C_1$ or promotes a provisional `Partner 1` cluster to `NutzerIn`, the engine emits the updated `cluster_id -> speaker_id` mapping so the UI retroactively updates earlier turn cards from that cluster.
4. **Calibration & Cold-Start Mitigation**:
   * **Conversational Calibration Text**: Update the 5-second onboarding prompt to start with a natural greeting and conversational question (*"Hallo, guten Tag! Ich habe heute einen Termin und hätte eine kurze Frage..."*) followed by a prose sentence, capturing both greeting prosody and read speech in the reference centroid.
   * **AGC Warm-Up Guard**: For the very first speech segment after `getUserMedia` initialization, down-weight or skip the first `150 ms` of frames when computing the utterance-level Fbank mean (`SubtractGlobalMean`) so WebRTC AGC gain ramp-up does not bias the spectral mean.

---

### B. Neural Speaker Segmentation via `pyannote/segmentation-3.0`
To resolve rapid speaker switches (`< 350 ms` pause) and overlapping speech, replace pure silence-based segmentation with **`pyannote/segmentation-3.0`** (`sherpa-onnx-pyannote-segmentation-3-0`):

* **Model Footprint**: **~5.8 MB ONNX** (`float32` / `int8`).
* **Input / Output**:
  * Accepts sliding windows of 16 kHz mono PCM audio (`5.0s` or `10.0s` window, `1.0s` step).
  * Outputs frame-level powerset probabilities (~16 ms frame resolution) across 7 classes: `{silence, spk1, spk2, spk3, spk1+spk2, spk1+spk3, spk2+spk3}`.
* **Pipeline Integration**:
  1. **Sub-Second Speaker Change Detection**: Detects exact speaker turn boundaries inside continuous speech even when two speakers talk back-to-back with zero silence gap.
  2. **Overlap Exclusion for Pure Embeddings**: Frames classified as overlapping speech (`spk1+spk2`, etc.) are masked out before extracting the 80-dim Fbank features for the embedding model, ensuring speaker embeddings are never contaminated by two simultaneous voices.

---

### C. Multilingual Speaker Embeddings via `WeSpeaker SimAM-ResNet100` (VoxBlink2 + VoxCeleb2 LM) in INT8
Upgrade the speaker embedding backbone from `wespeaker_en_voxceleb_resnet152_LM.onnx` to **`WeSpeaker SimAM-ResNet100` pretrained on `VoxBlink2` and fine-tuned with Large-Margin on `VoxCeleb2` (INT8 quantized)**:

* **Model Comparison**:

| Metric | Current: `ResNet152-LM` (FP32) | Target: `SimAM-ResNet100 VoxBlink2-LM` (INT8) |
| :--- | :--- | :--- |
| **Training Corpus** | VoxCeleb2 (~6k speakers, mostly English) | **VoxBlink2 (>100k multilingual speakers in the wild)** + VoxCeleb2 LM |
| **Parameters** | ~19.8M | ~50.2M |
| **ONNX File Size** | ~76 MB (FP32) | **~50.3 MB (INT8)** (~200 MB FP32) |
| **VoxCeleb1-O EER** | ~0.59% | **~0.17% – 0.21%** (~3× lower error rate) |
| **Frontend Features** | 80-dim Kaldi Fbank (`25 ms` / `10 ms`, CMN) | **Identical 80-dim Kaldi Fbank** (drop-in compatible with [`fbank.rs`](../crates/vad-diarization-wasm/src/fbank.rs)) |
| **Output Embedding** | 256-D L2-normalized vector | **256-D L2-normalized vector** |

---

## 3. Resource & Memory Constraints (WASM & Mobile)
* **Combined Model Download Size**:
  * `pyannote-segmentation-3.0.onnx` (~5.8 MB) + `wespeaker_voxblink2_samresnet100_LM.int8.onnx` (~50.3 MB) = **~56.1 MB total** (a **~26% size reduction** compared to the current 78.2 MB `Silero VAD + FP32 ResNet152` bundle).
* **Active Runtime RAM / VRAM**:
  * Peak WASM + ONNX Runtime Web memory stays under **150 MB** (well within the 200 MB budget from PRD 1).
  * On native mobile apps (iOS CoreML / Android NNAPI / `sherpa-onnx`), the exact same ONNX models execute on the NPU/GPU in `< 25 ms` per segment.

---

## 4. Proposed WASM & JS Interface Extensions

### [`crates/vad-diarization-wasm/src/lib.rs`](../crates/vad-diarization-wasm/src/lib.rs)
```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DiarizedSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Stable acoustic cluster identifier (e.g., "cluster_0", "cluster_1")
    pub cluster_id: String,
    /// Resolved display speaker label ("User", "Speaker 1", "Speaker 2")
    pub speaker_id: String,
    /// Raw cosine similarity between the segment's cluster centroid and the User profile
    pub user_similarity: Option<f32>,
    /// Competitive calibrated probability [0.0, 1.0] that this segment's cluster is the User
    pub user_likelihood: Option<f32>,
    /// Optional map of retroactive cluster_id -> speaker_id updates when a new cluster shifts the competitive posterior
    pub cluster_relabels: Option< Vec<ClusterRelabel> >,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClusterRelabel {
    pub cluster_id: String,
    pub speaker_id: String,
    pub user_likelihood: f32,
}
```

---

## 5. Implementation Steps
1. **Cluster-First Competitive Attribution (`clustering.rs`)**:
   * Refactor `AhcClusterer` so all segments enter AHC clusters (`cluster_0`, `cluster_1`, …) with duration-weighted centroids.
   * Implement the single-cluster opening prior (`sim >= 0.38`) and two-speaker competitive posterior with `ClusterRelabel` emission.
   * Add unit tests covering the `sim = 0.445` (29%) Turn 1 opening scenario followed by a `sim = 0.18` Turn 2 partner utterance.
2. **Calibration & Cold-Start Updates (`vad_diarization.html` & `fbank.rs`)**:
   * Update the calibration reading prompt to include a conversational greeting and question.
   * Add the `150 ms` initial-frame warm-up guard in Fbank CMN for the first segment after stream start.
3. **Model Upgrade (`SimAM-ResNet100 VoxBlink2 INT8`)**:
   * Update `scripts/download_models.sh` and `static/ort_bridge.js` to fetch and cache the INT8 `VoxBlink2 SimAM-ResNet100` ONNX model.
4. **Neural Speaker Segmentation (`pyannote/segmentation-3.0`)**:
   * Integrate `pyannote-segmentation-3.0.onnx` into `static/ort_bridge.js` and `crates/vad-diarization-wasm` to split zero-gap speaker switches and mask overlapping speech frames prior to Fbank extraction.
5. **Retroactive UI Card Relabeling (`transcript_alignment.js` & `vad_diarization.html`)**:
   * Persist `clusterId` on each finalized dialog card and apply `cluster_relabels` events so Turn 1 automatically updates if Turn 2 disambiguates the speaker clusters.
