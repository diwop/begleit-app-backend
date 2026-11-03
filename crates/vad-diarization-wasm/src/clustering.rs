use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Maximum number of recent segment embeddings retained for Agglomerative Hierarchical Clustering
/// so active memory remains strictly bounded over hour-long sessions (~512 KB for 256 x 512-D vectors).
const MAX_STORED_SEGMENTS: usize = 256;

/// Default minimum cosine similarity (`0.42`) required for AHC average-linkage to merge two clusters
/// using L2-normalized WeSpeaker ResNet-152 LM VoxCeleb speaker embeddings.
pub const DEFAULT_CLUSTER_SIMILARITY_THRESHOLD: f32 = 0.42;

/// Default minimum cosine similarity (`0.52`) reserved for matching known voice profiles (PRD-2)
/// with the 256-D WeSpeaker speaker embedding model.
/// Tuned to reliably match the calibrated speaker across dynamic vocal changes (e.g. louder, faster speech)
/// while remaining well above distinct speakers (~0.10–0.32).
pub const DEFAULT_PROFILE_RECOGNITION_THRESHOLD: f32 = 0.52;

/// Minimum cosine similarity floor (`0.35`) required for any cluster to be eligible for `"NutzerIn"`
/// under the First-Two-Speakers prior or competitive cluster attribution (PRD-3).
pub const MIN_COMPETITIVE_USER_FLOOR: f32 = 0.35;

/// Minimum cosine similarity margin (`0.08`) by which the leading speaker cluster must exceed the
/// runner-up cluster to claim `"NutzerIn"` when its absolute similarity is below the strict standalone threshold.
pub const COMPETITIVE_CLUSTER_MARGIN: f32 = 0.08;

/// Returns an L2-normalized copy of `vec`. Returns a zero vector if the norm is near zero.
pub fn l2_normalize(vec: &[f32]) -> Vec<f32> {
    let norm_sq: f32 = vec.iter().map(|&v| v * v).sum();
    let norm = norm_sq.sqrt();
    if norm <= 1e-12 {
        vec![0.0; vec.len()]
    } else {
        vec.iter().map(|&v| v / norm).collect()
    }
}

/// Computes the Cosine Similarity in `[-1.0, 1.0]` between two L2-normalized vectors of equal length.
pub fn cosine_similarity_normalized(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "Embedding dimensions must match");
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| x * y)
        .sum::<f32>()
        .clamp(-1.0, 1.0)
}

/// Computes the posterior likelihood in `[0.0, 1.0]` that an embedding belongs to the calibrated
/// voice profile given its cosine similarity to the profile centroid and the recognition threshold.
///
/// Uses a logistic sigmoid calibrated so `similarity == threshold` yields a 50% likelihood (`0.50`),
/// strong matches (`>= 0.70`) yield `>= 90%`, and distinct speakers (`<= 0.30`) yield `<= 7%`.
pub fn cosine_similarity_to_likelihood(similarity: f32, threshold: f32) -> f32 {
    let k = 12.0_f32;
    let logit = k * (similarity - threshold);
    (1.0 / (1.0 + (-logit).exp())).clamp(0.0, 1.0)
}

/// Computes the competitive calibrated posterior likelihood in `[0.0, 1.0]` for a speaker cluster
/// with user similarity `cluster_sim` when at least one competing speaker cluster exists with
/// maximum competing similarity `other_best_sim`.
///
/// Incorporates both the single-cluster opening threshold `opening_threshold` and the relative
/// margin `cluster_sim - other_best_sim`, while attenuating positive margin bonuses when
/// `cluster_sim` is below the sanity floor so two non-user strangers never inflate toward 50%.
pub fn competitive_cluster_likelihood(
    cluster_sim: f32,
    other_best_sim: f32,
    opening_threshold: f32,
) -> f32 {
    let delta = cluster_sim - other_best_sim;
    let bonus = if delta > 0.0 {
        let floor_scale = ((cluster_sim - 0.25) / 0.15).clamp(0.0, 1.0);
        8.0 * delta * floor_scale
    } else {
        8.0 * delta
    };
    let logit = 10.0 * (cluster_sim - opening_threshold) + bonus;
    (1.0 / (1.0 + (-logit).exp())).clamp(0.0, 1.0)
}

/// Retroactive speaker label and calibrated user likelihood update emitted when the arrival
/// of a new segment or cluster shifts the competitive attribution of an earlier cluster.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClusterRelabel {
    /// Stable acoustic cluster identifier (e.g., `"cluster_0"`, `"cluster_1"`).
    pub cluster_id: String,
    /// Updated canonical speaker label (e.g., `"NutzerIn"`, `"Speaker 1"`).
    pub speaker_id: String,
    /// Updated calibrated user posterior likelihood in `[0.0, 1.0]`, if a `"NutzerIn"` profile is loaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_likelihood: Option<f32>,
}

/// Result of speaker assignment for a diarized speech segment including cluster metadata,
/// user likelihood metrics, and any retroactive cluster relabels.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerAssignment {
    /// Stable acoustic cluster identifier (e.g. `"cluster_0"`, `"cluster_1"`).
    pub cluster_id: String,
    /// Canonical speaker label (e.g. `"NutzerIn"`, `"Speaker 1"`).
    pub speaker_id: String,
    /// Cosine similarity between the assigned cluster centroid and the `"NutzerIn"` profile centroid.
    pub user_similarity: Option<f32>,
    /// Calibrated user posterior likelihood in `[0.0, 1.0]`, if a profile is loaded.
    pub user_likelihood: Option<f32>,
    /// Retroactive cluster label updates emitted when an existing cluster's speaker or likelihood changed.
    pub cluster_relabels: Vec<ClusterRelabel>,
}

/// Summary information for an active speaker cluster exposed by `get_current_clusters()`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerClusterSummary {
    /// Stable acoustic cluster identifier (e.g., `"cluster_0"`, `"cluster_1"`).
    pub cluster_id: String,
    /// Human-readable speaker identifier (e.g., `"Speaker 1"`, `"Speaker 2"`, or a known profile ID).
    pub speaker_id: String,
    /// Number of speech segments assigned to this speaker cluster.
    pub segment_count: usize,
    /// Earliest session timestamp (seconds) at which this speaker spoke.
    pub first_seen_time: f64,
}

/// Serialized voice profile stored in IndexedDB and loaded into the diarization engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VoiceProfile {
    /// Unique profile identifier (e.g. `"NutzerIn"`).
    pub id: String,
    /// L2-normalized centroid embedding vector.
    pub centroid: Vec<f32>,
    /// Number of speech embeddings accumulated into this centroid during calibration.
    pub embedding_count: usize,
    /// Total duration of active speech (in seconds) used to construct the profile.
    pub speech_seconds: f32,
    /// Optional identifier of the embedding model used to extract this profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

/// Computes the element-wise average of multiple embedding vectors and returns an L2-normalized centroid.
///
/// Returns an empty vector if `embeddings` is empty or if vectors have mismatched dimensions.
pub fn compute_centroid(embeddings: &[Vec<f32>]) -> Vec<f32> {
    if embeddings.is_empty() {
        return Vec::new();
    }
    let dim = embeddings[0].len();
    if dim == 0 {
        return Vec::new();
    }
    let mut sum = vec![0.0_f32; dim];
    let mut valid_count = 0usize;
    for emb in embeddings {
        if emb.len() == dim {
            for (s, &v) in sum.iter_mut().zip(emb.iter()) {
                *s += v;
            }
            valid_count += 1;
        }
    }
    if valid_count == 0 {
        return vec![0.0; dim];
    }
    let factor = 1.0 / (valid_count as f32);
    for s in &mut sum {
        *s *= factor;
    }
    l2_normalize(&sum)
}

/// Stored embedding record for an individual speech segment participating in AHC.
#[derive(Debug, Clone)]
struct SegmentEmbeddingRecord {
    uid: u64,
    start_time: f64,
    duration_weight: f32,
    embedding: Vec<f32>,
}

/// Native Rust Agglomerative Hierarchical Clustering (AHC) speaker diarization engine
/// with cluster-first competitive attribution and First-Two-Speakers Bayesian prior (PRD-3).
///
/// Groups all L2-normalized speaker embeddings into acoustic clusters (`"cluster_0"`, `"cluster_1"`, ...)
/// using duration-weighted centroid cosine linkage, competitively evaluates known voice profiles across
/// cluster centroids, and emits [`ClusterRelabel`] events when a new segment shifts an earlier cluster's role.
pub struct AhcClusterer {
    /// Minimum cosine similarity required to merge two clusters (`distance <= 1.0 - threshold`).
    cluster_similarity_threshold: f32,
    /// Minimum cosine similarity required to match a known voice fingerprint (PRD-2 / PRD-3).
    profile_recognition_threshold: f32,
    /// Known voice profiles (`profile_id -> L2-normalized centroid`).
    known_profiles: HashMap<String, Vec<f32>>,
    /// Chronological ring/list of recent segment embeddings for hierarchical re-clustering.
    segments: Vec<SegmentEmbeddingRecord>,
    /// Monotonically increasing segment UID counter used to anchor stable `cluster_id`s.
    next_segment_uid: u64,
    /// Monotonically increasing cluster sequence counter (`0 -> "cluster_0"`, `1 -> "cluster_1"`).
    next_cluster_seq: usize,
    /// Maps a cluster's earliest member `uid` to its stable `"cluster_N"` identifier.
    anchor_uid_to_cluster_id: HashMap<u64, String>,
    /// Maps each segment `uid` to the `cluster_id` it belonged to on the previous AHC pass.
    prev_segment_cluster_id: HashMap<u64, String>,
    /// Maps each `cluster_id` to its `(speaker_id, user_likelihood)` from the previous AHC pass.
    prev_cluster_assignment: HashMap<String, (String, Option<f32>)>,
    /// Latest cluster summary snapshot computed during the most recent assignment.
    latest_clusters: Vec<SpeakerClusterSummary>,
    /// Speaker label assigned to the most recent segment (used as fallback for empty embeddings).
    last_speaker_id: Option<String>,
    /// Cluster ID assigned to the most recent segment.
    last_cluster_id: Option<String>,
    /// End timestamp (seconds) of the most recently assigned segment for temporal continuity.
    last_end_time: Option<f64>,
}

impl AhcClusterer {
    /// Creates a new [`AhcClusterer`] with the specified cosine similarity threshold.
    pub fn new(cluster_similarity_threshold: f32) -> Self {
        Self {
            cluster_similarity_threshold,
            profile_recognition_threshold: DEFAULT_PROFILE_RECOGNITION_THRESHOLD,
            known_profiles: HashMap::new(),
            segments: Vec::with_capacity(64),
            next_segment_uid: 0,
            next_cluster_seq: 0,
            anchor_uid_to_cluster_id: HashMap::new(),
            prev_segment_cluster_id: HashMap::new(),
            prev_cluster_assignment: HashMap::new(),
            latest_clusters: Vec::new(),
            last_speaker_id: None,
            last_cluster_id: None,
            last_end_time: None,
        }
    }

    /// Resets session segments and active speaker clusters while keeping any loaded known profiles.
    pub fn reset(&mut self) {
        self.segments.clear();
        self.next_segment_uid = 0;
        self.next_cluster_seq = 0;
        self.anchor_uid_to_cluster_id.clear();
        self.prev_segment_cluster_id.clear();
        self.prev_cluster_assignment.clear();
        self.latest_clusters.clear();
        self.last_speaker_id = None;
        self.last_cluster_id = None;
        self.last_end_time = None;
    }

    /// Loads known voice profile centroids (`profile_id -> embedding`), L2-normalizing each vector.
    pub fn set_known_profiles(&mut self, profiles: HashMap<String, Vec<f32>>) {
        self.known_profiles = profiles
            .into_iter()
            .map(|(id, vec)| (id, l2_normalize(&vec)))
            .collect();
    }

    /// Upserts a single known voice profile with an L2-normalized centroid.
    pub fn upsert_known_profile(&mut self, id: String, centroid: Vec<f32>) {
        self.known_profiles.insert(id, l2_normalize(&centroid));
    }

    /// Loads known voice profiles from a JSON string.
    ///
    /// Accepts either a JSON array of [`VoiceProfile`] objects (`[{"id":"NutzerIn","centroid":[...]}]`)
    /// or a JSON map (`{"NutzerIn":[...]}`).
    pub fn load_profiles_json(&mut self, json_str: &str) -> Result<usize, String> {
        let trimmed = json_str.trim();
        if trimmed.is_empty() || trimmed == "[]" || trimmed == "{}" {
            self.known_profiles.clear();
            return Ok(0);
        }

        if trimmed.starts_with('[') {
            let list: Vec<VoiceProfile> = serde_json::from_str(trimmed)
                .map_err(|e| format!("Failed to parse VoiceProfile array: {e}"))?;
            let count = list.len();
            for p in list {
                self.upsert_known_profile(p.id, p.centroid);
            }
            Ok(count)
        } else {
            let map: HashMap<String, Vec<f32>> = serde_json::from_str(trimmed)
                .map_err(|e| format!("Failed to parse voice profile map: {e}"))?;
            let count = map.len();
            for (id, centroid) in map {
                self.upsert_known_profile(id, centroid);
            }
            Ok(count)
        }
    }

    /// Updates the recognition similarity threshold used against known voice profiles.
    pub fn set_recognition_threshold(&mut self, threshold: f32) {
        self.profile_recognition_threshold = threshold.clamp(0.0, 1.0);
    }

    /// Returns a slice of the currently identified speaker clusters ordered by first appearance.
    pub fn current_clusters(&self) -> &[SpeakerClusterSummary] {
        &self.latest_clusters
    }

    /// Assigns a speaker ID (`"Speaker 1"`, `"Speaker 2"`, or a matched profile ID) to a newly
    /// extracted raw `embedding` covering `[start_time, end_time]`.
    pub fn assign_speaker(
        &mut self,
        raw_embedding: &[f32],
        start_time: f64,
        end_time: f64,
    ) -> String {
        self.assign_speaker_with_scores(raw_embedding, start_time, end_time)
            .speaker_id
    }

    /// Assigns a speaker ID, stable `cluster_id`, calibrated user likelihood metrics, and any
    /// retroactive [`ClusterRelabel`] updates for a newly extracted raw `embedding` covering
    /// `[start_time, end_time]`.
    pub fn assign_speaker_with_scores(
        &mut self,
        raw_embedding: &[f32],
        start_time: f64,
        end_time: f64,
    ) -> SpeakerAssignment {
        if raw_embedding.is_empty() {
            let speaker_id = self
                .last_speaker_id
                .clone()
                .unwrap_or_else(|| "Speaker 1".to_string());
            let cluster_id = self
                .last_cluster_id
                .clone()
                .unwrap_or_else(|| "cluster_0".to_string());
            return SpeakerAssignment {
                cluster_id,
                speaker_id,
                user_similarity: None,
                user_likelihood: None,
                cluster_relabels: Vec::new(),
            };
        }

        let norm_emb = l2_normalize(raw_embedding);
        let duration = (end_time - start_time).max(0.2) as f32;

        // 1. Evict oldest segment if capacity cap is reached
        if self.segments.len() >= MAX_STORED_SEGMENTS {
            self.segments.remove(0);
        }

        // 2. Insert or replace checkpoint for the same `start_time` (`|s.start_time - start_time| < 0.05`)
        //    so every speech segment participates in cluster-first AHC without double-counting checkpoints.
        let target_idx = if let Some(existing_idx) = self
            .segments
            .iter()
            .position(|s| (s.start_time - start_time).abs() < 0.05)
        {
            let existing_uid = self.segments[existing_idx].uid;
            self.segments[existing_idx] = SegmentEmbeddingRecord {
                uid: existing_uid,
                start_time,
                duration_weight: duration.min(3.0),
                embedding: norm_emb.clone(),
            };
            existing_idx
        } else {
            let uid = self.next_segment_uid;
            self.next_segment_uid += 1;
            self.segments.push(SegmentEmbeddingRecord {
                uid,
                start_time,
                duration_weight: duration.min(3.0),
                embedding: norm_emb.clone(),
            });
            self.segments.len() - 1
        };

        // 3. Run cluster-first AHC + competitive profile attribution
        let mut assignment = self.run_ahc_and_compete(target_idx);

        // 4. Gently adapt in-memory "NutzerIn" centroid on clean, high-confidence segments (>= 1.4 s)
        if assignment.speaker_id == "NutzerIn" && duration >= 1.4 {
            if let Some(centroid) = self.known_profiles.get_mut("NutzerIn") {
                if centroid.len() == norm_emb.len() {
                    let raw_sim = cosine_similarity_normalized(&norm_emb, centroid);
                    if raw_sim >= self.profile_recognition_threshold + 0.04 {
                        let blended: Vec<f32> = centroid
                            .iter()
                            .zip(norm_emb.iter())
                            .map(|(&c, &e)| 0.92 * c + 0.08 * e)
                            .collect();
                        *centroid = l2_normalize(&blended);
                    }
                }
            }
        }

        self.last_speaker_id = Some(assignment.speaker_id.clone());
        self.last_cluster_id = Some(assignment.cluster_id.clone());
        self.last_end_time = Some(end_time);

        // Ensure deterministic ordering of any emitted cluster relabels
        assignment
            .cluster_relabels
            .sort_by(|a, b| a.cluster_id.cmp(&b.cluster_id));
        assignment
    }

    /// Executes bottom-up Agglomerative Hierarchical Clustering across `self.segments`, applies
    /// the First-Two-Speakers Bayesian prior and competitive profile attribution across cluster
    /// centroids, and returns the [`SpeakerAssignment`] for `target_segment_idx`.
    fn run_ahc_and_compete(&mut self, target_segment_idx: usize) -> SpeakerAssignment {
        struct ActiveCluster {
            member_indices: Vec<usize>,
            earliest_time: f64,
            earliest_uid: u64,
            total_weight: f32,
            centroid: Vec<f32>,
        }

        let mut clusters: Vec<ActiveCluster> = self
            .segments
            .iter()
            .enumerate()
            .map(|(idx, seg)| ActiveCluster {
                member_indices: vec![idx],
                earliest_time: seg.start_time,
                earliest_uid: seg.uid,
                total_weight: seg.duration_weight,
                centroid: seg.embedding.clone(),
            })
            .collect();

        while clusters.len() > 1 {
            let mut best_pair: Option<(usize, usize, f32)> = None;
            for i in 0..clusters.len() {
                for j in (i + 1)..clusters.len() {
                    if clusters[i].centroid.len() != clusters[j].centroid.len() {
                        continue;
                    }
                    let sim =
                        cosine_similarity_normalized(&clusters[i].centroid, &clusters[j].centroid);
                    if best_pair.is_none_or(|(_, _, best_sim)| sim > best_sim) {
                        best_pair = Some((i, j, sim));
                    }
                }
            }

            let Some((i, j, max_sim)) = best_pair else {
                break;
            };
            if max_sim < self.cluster_similarity_threshold {
                break;
            }

            // Merge cluster j into cluster i using duration-weighted centroid averaging + L2 norm
            let removed = clusters.remove(j);
            let dst = &mut clusters[i];
            let new_weight = dst.total_weight + removed.total_weight;
            let mut merged_vec = vec![0.0_f32; dst.centroid.len()];
            for (k, slot) in merged_vec.iter_mut().enumerate() {
                *slot =
                    dst.centroid[k] * dst.total_weight + removed.centroid[k] * removed.total_weight;
            }
            dst.centroid = l2_normalize(&merged_vec);
            dst.total_weight = new_weight;
            dst.earliest_time = dst.earliest_time.min(removed.earliest_time);
            dst.earliest_uid = dst.earliest_uid.min(removed.earliest_uid);
            dst.member_indices.extend(removed.member_indices);
        }

        // Order clusters deterministically by earliest appearance
        clusters.sort_by(|a, b| {
            a.earliest_time
                .partial_cmp(&b.earliest_time)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let num_clusters = clusters.len();

        // Assign stable "cluster_N" IDs anchored by each cluster's `earliest_uid`
        let mut cluster_ids = Vec::with_capacity(num_clusters);
        for cluster in &clusters {
            let cid =
                if let Some(existing) = self.anchor_uid_to_cluster_id.get(&cluster.earliest_uid) {
                    existing.clone()
                } else {
                    let allocated = format!("cluster_{}", self.next_cluster_seq);
                    self.next_cluster_seq += 1;
                    self.anchor_uid_to_cluster_id
                        .insert(cluster.earliest_uid, allocated.clone());
                    allocated
                };
            cluster_ids.push(cid);
        }

        // Compute "NutzerIn" similarity and competitive posterior across all active clusters
        let mut cluster_user_sims: Vec<Option<f32>> = vec![None; num_clusters];
        let mut cluster_user_likelihoods: Vec<Option<f32>> = vec![None; num_clusters];
        let mut cluster_speaker_labels: Vec<Option<String>> = vec![None; num_clusters];

        if let Some(user_centroid) = self.known_profiles.get("NutzerIn") {
            let opening_relax = if self.profile_recognition_threshold <= 0.60 {
                0.14
            } else {
                0.04
            };
            let competitive_relax = if self.profile_recognition_threshold <= 0.60 {
                0.17
            } else {
                0.06
            };
            let opening_threshold = (self.profile_recognition_threshold - opening_relax).clamp(
                MIN_COMPETITIVE_USER_FLOOR,
                self.profile_recognition_threshold,
            );
            let competitive_floor = (self.profile_recognition_threshold - competitive_relax).clamp(
                MIN_COMPETITIVE_USER_FLOOR,
                self.profile_recognition_threshold,
            );

            for (k, cluster) in clusters.iter().enumerate() {
                if cluster.centroid.len() == user_centroid.len() {
                    cluster_user_sims[k] = Some(cosine_similarity_normalized(
                        &cluster.centroid,
                        user_centroid,
                    ));
                }
            }

            if num_clusters == 1 {
                if let Some(s0) = cluster_user_sims[0] {
                    let likelihood = cosine_similarity_to_likelihood(s0, opening_threshold);
                    cluster_user_likelihoods[0] = Some(likelihood);
                    if s0 >= opening_threshold {
                        cluster_speaker_labels[0] = Some("NutzerIn".to_string());
                    }
                }
            } else if num_clusters >= 2 {
                // Find the cluster k* with maximum user similarity and the runner-up similarity
                let mut best_k: Option<(usize, f32)> = None;
                for (k, maybe_sim) in cluster_user_sims.iter().copied().enumerate() {
                    if let Some(sim) = maybe_sim {
                        if best_k.is_none_or(|(_, best_sim)| sim > best_sim) {
                            best_k = Some((k, sim));
                        }
                    }
                }

                if let Some((k_star, s_star)) = best_k {
                    let s_runner_up = cluster_user_sims
                        .iter()
                        .copied()
                        .enumerate()
                        .filter(|&(k, _)| k != k_star)
                        .filter_map(|(_, s)| s)
                        .fold(f32::NEG_INFINITY, f32::max);

                    let margin = if s_runner_up.is_finite() {
                        s_star - s_runner_up
                    } else {
                        1.0
                    };

                    if s_star >= competitive_floor
                        && (s_star >= self.profile_recognition_threshold - 0.02
                            || margin >= COMPETITIVE_CLUSTER_MARGIN)
                    {
                        cluster_speaker_labels[k_star] = Some("NutzerIn".to_string());
                    }

                    for (k, maybe_sim) in cluster_user_sims.iter().copied().enumerate() {
                        if let Some(sk) = maybe_sim {
                            let other_sim = if k == k_star {
                                if s_runner_up.is_finite() {
                                    s_runner_up
                                } else {
                                    0.0
                                }
                            } else {
                                s_star
                            };
                            cluster_user_likelihoods[k] = Some(competitive_cluster_likelihood(
                                sk,
                                other_sim,
                                opening_threshold,
                            ));
                        }
                    }
                }
            }
        }

        // Match any additional non-"NutzerIn" known profiles (e.g. "Alice", "Partner")
        let mut other_profile_ids: Vec<&String> = self
            .known_profiles
            .keys()
            .filter(|id| id.as_str() != "NutzerIn")
            .collect();
        other_profile_ids.sort();

        for profile_id in other_profile_ids {
            let Some(profile_centroid) = self.known_profiles.get(profile_id) else {
                continue;
            };
            let effective_threshold =
                (self.profile_recognition_threshold - 0.06).max(self.cluster_similarity_threshold);
            let mut best_match: Option<(usize, f32)> = None;
            for (k, cluster) in clusters.iter().enumerate() {
                if cluster_speaker_labels[k].is_some()
                    || cluster.centroid.len() != profile_centroid.len()
                {
                    continue;
                }
                let sim = cosine_similarity_normalized(&cluster.centroid, profile_centroid);
                if sim >= effective_threshold
                    && best_match.is_none_or(|(_, best_sim)| sim > best_sim)
                {
                    best_match = Some((k, sim));
                }
            }
            if let Some((matched_k, _)) = best_match {
                cluster_speaker_labels[matched_k] = Some(profile_id.clone());
            }
        }

        // Number all remaining unassigned clusters sequentially: "Speaker 1", "Speaker 2", ...
        let mut generic_rank = 1usize;
        let final_labels: Vec<String> = cluster_speaker_labels
            .into_iter()
            .map(|maybe_label| match maybe_label {
                Some(named) => named,
                None => {
                    let lbl = format!("Speaker {generic_rank}");
                    generic_rank += 1;
                    lbl
                }
            })
            .collect();

        // Detect retroactive relabels for previously emitted clusters (including clusters that merged)
        let mut relabels_by_cid: HashMap<String, ClusterRelabel> = HashMap::new();
        let target_segment_uid = self.segments[target_segment_idx].uid;

        for (k, cluster) in clusters.iter().enumerate() {
            let cid = &cluster_ids[k];
            let spk = &final_labels[k];
            let like = cluster_user_likelihoods[k];

            // 1. Check if this cluster_id existed previously with a different speaker_id or likelihood
            if let Some((prev_spk, prev_like)) = self.prev_cluster_assignment.get(cid) {
                let like_changed = match (*prev_like, like) {
                    (Some(a), Some(b)) => (a - b).abs() >= 0.01,
                    (None, Some(_)) | (Some(_), None) => true,
                    (None, None) => false,
                };
                if prev_spk != spk || like_changed {
                    relabels_by_cid.insert(
                        cid.clone(),
                        ClusterRelabel {
                            cluster_id: cid.clone(),
                            speaker_id: spk.clone(),
                            user_likelihood: like,
                        },
                    );
                }
            }

            // 2. Check if any member segment previously belonged to a different cluster_id that merged into `cid`
            for &member_idx in &cluster.member_indices {
                let seg_uid = self.segments[member_idx].uid;
                if seg_uid == target_segment_uid {
                    continue;
                }
                if let Some(old_cid) = self.prev_segment_cluster_id.get(&seg_uid) {
                    if old_cid != cid {
                        relabels_by_cid.insert(
                            old_cid.clone(),
                            ClusterRelabel {
                                cluster_id: old_cid.clone(),
                                speaker_id: spk.clone(),
                                user_likelihood: like,
                            },
                        );
                    }
                }
            }
        }

        // Update state snapshots for the next pass and populate `latest_clusters`
        self.prev_cluster_assignment.clear();
        self.prev_segment_cluster_id.clear();
        self.latest_clusters.clear();

        let mut target_cluster_idx = 0usize;
        for (k, cluster) in clusters.iter().enumerate() {
            let cid = &cluster_ids[k];
            let spk = &final_labels[k];
            let like = cluster_user_likelihoods[k];

            self.prev_cluster_assignment
                .insert(cid.clone(), (spk.clone(), like));
            for &member_idx in &cluster.member_indices {
                let seg_uid = self.segments[member_idx].uid;
                self.prev_segment_cluster_id.insert(seg_uid, cid.clone());
                if member_idx == target_segment_idx {
                    target_cluster_idx = k;
                }
            }

            self.latest_clusters.push(SpeakerClusterSummary {
                cluster_id: cid.clone(),
                speaker_id: spk.clone(),
                segment_count: cluster.member_indices.len(),
                first_seen_time: cluster.earliest_time,
            });
        }

        SpeakerAssignment {
            cluster_id: cluster_ids[target_cluster_idx].clone(),
            speaker_id: final_labels[target_cluster_idx].clone(),
            user_similarity: cluster_user_sims[target_cluster_idx],
            user_likelihood: cluster_user_likelihoods[target_cluster_idx],
            cluster_relabels: relabels_by_cid.into_values().collect(),
        }
    }
}

impl Default for AhcClusterer {
    fn default() -> Self {
        Self::new(DEFAULT_CLUSTER_SIMILARITY_THRESHOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies L2 normalization and cosine similarity edge cases.
    #[test]
    fn test_l2_normalize_and_cosine_similarity() {
        let zero = l2_normalize(&[0.0, 0.0, 0.0]);
        assert_eq!(zero, vec![0.0, 0.0, 0.0]);

        let a = l2_normalize(&[3.0, 4.0, 0.0]);
        assert!((a[0] - 0.6).abs() < 1e-6);
        assert!((a[1] - 0.8).abs() < 1e-6);

        let b = l2_normalize(&[0.0, 0.0, 5.0]);
        assert!(cosine_similarity_normalized(&a, &b).abs() < 1e-6);
        assert!((cosine_similarity_normalized(&a, &a) - 1.0).abs() < 1e-6);
    }

    /// Verifies AHC separates distinct speakers into "Speaker 1" and "Speaker 2" while merging
    /// repeated turns from the same speaker and keeping temporal labels stable.
    #[test]
    fn test_ahc_clustering_multi_turn_dialog() {
        let mut clusterer = AhcClusterer::default();

        // Speaker A voice direction (around [1, 0, 0, 0])
        let spk_a_1 = [0.95, 0.10, 0.0, 0.0];
        let spk_a_2 = [0.92, -0.08, 0.05, 0.0];

        // Speaker B voice direction (around [0, 1, 0, 0])
        let spk_b_1 = [0.05, 0.96, 0.04, 0.0];
        let spk_b_2 = [-0.04, 0.94, 0.10, 0.0];

        // Turn 1: Speaker A (0.5s - 2.0s), first checkpoint at 1.4s then final at 2.0s
        assert_eq!(clusterer.assign_speaker(&spk_a_1, 0.5, 1.4), "Speaker 1");
        assert_eq!(clusterer.assign_speaker(&spk_a_1, 0.5, 2.0), "Speaker 1");
        assert_eq!(clusterer.current_clusters().len(), 1);
        assert_eq!(
            clusterer.current_clusters()[0].segment_count,
            1,
            "Checkpoint replacement should not duplicate segment"
        );

        // Turn 2: Speaker B (2.5s - 4.2s) -> new cluster "Speaker 2"
        assert_eq!(clusterer.assign_speaker(&spk_b_1, 2.5, 4.2), "Speaker 2");
        assert_eq!(clusterer.current_clusters().len(), 2);

        // Turn 3: Speaker A speaks again (4.8s - 6.3s) -> recognized as "Speaker 1"
        assert_eq!(clusterer.assign_speaker(&spk_a_2, 4.8, 6.3), "Speaker 1");

        // Turn 4: Speaker B speaks again (7.0s - 8.5s) -> recognized as "Speaker 2"
        assert_eq!(clusterer.assign_speaker(&spk_b_2, 7.0, 8.5), "Speaker 2");

        let summaries = clusterer.current_clusters();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].speaker_id, "Speaker 1");
        assert_eq!(summaries[0].segment_count, 2);
        assert_eq!(summaries[1].speaker_id, "Speaker 2");
        assert_eq!(summaries[1].segment_count, 2);
    }

    /// Verifies known profile similarity gate reserved for PRD-2 voice fingerprinting.
    #[test]
    fn test_known_profile_recognition_gate() {
        let mut clusterer = AhcClusterer::default();
        let mut profiles = HashMap::new();
        profiles.insert("Alice".to_string(), vec![0.0, 0.0, 1.0, 0.0]);
        clusterer.set_known_profiles(profiles);

        assert_eq!(
            clusterer.assign_speaker(&[0.02, -0.01, 0.99, 0.0], 0.0, 1.5),
            "Alice"
        );
        assert_eq!(
            clusterer.assign_speaker(&[1.0, 0.0, 0.0, 0.0], 2.0, 3.5),
            "Speaker 1"
        );
    }

    /// Verifies calculation of L2-normalized centroid from multiple embeddings.
    #[test]
    fn test_compute_centroid() {
        assert!(compute_centroid(&[]).is_empty());
        assert!(compute_centroid(&[vec![]]).is_empty());

        let v1 = vec![1.0, 0.0, 0.0];
        let v2 = vec![0.0, 1.0, 0.0];
        let centroid = compute_centroid(&[v1, v2]);
        assert_eq!(centroid.len(), 3);
        let expected_val = (0.5_f32).sqrt();
        assert!((centroid[0] - expected_val).abs() < 1e-6);
        assert!((centroid[1] - expected_val).abs() < 1e-6);
        assert_eq!(centroid[2], 0.0);
    }

    /// Verifies loading profiles from JSON array or JSON map, and setting threshold.
    #[test]
    fn test_load_profiles_json_and_threshold() {
        let mut clusterer = AhcClusterer::default();

        let json_array = r#"[
            {
                "id": "NutzerIn",
                "centroid": [0.0, 1.0, 0.0],
                "embeddingCount": 5,
                "speechSeconds": 5.2
            }
        ]"#;
        let count = clusterer.load_profiles_json(json_array).unwrap();
        assert_eq!(count, 1);

        // Near-match (cosine similarity ~0.8) matches with default 0.72 threshold
        assert_eq!(
            clusterer.assign_speaker(&[0.6, 0.8, 0.0], 0.0, 1.5),
            "NutzerIn"
        );

        // Raising threshold to 0.95 forces it to fall back to generic AHC
        clusterer.set_recognition_threshold(0.95);
        assert_eq!(
            clusterer.assign_speaker(&[0.6, 0.8, 0.0], 2.0, 3.5),
            "Speaker 1"
        );

        // JSON map loading
        let json_map = r#"{"Partner": [0.0, 0.0, 1.0]}"#;
        clusterer.load_profiles_json(json_map).unwrap();
        assert_eq!(
            clusterer.assign_speaker(&[0.0, 0.0, 1.0], 4.0, 5.5),
            "Partner"
        );
    }

    /// Verifies that when a tentative checkpoint was stored as generic Speaker 1, but the
    /// finalized segment matches a known voice profile, the cluster is upgraded to "NutzerIn".
    #[test]
    fn test_profile_checkpoint_upgrade_removes_generic_cluster() {
        let mut clusterer = AhcClusterer::default();

        // 1. Initial checkpoint at 0.5s with a generic/noisy embedding -> assigned Speaker 1
        assert_eq!(
            clusterer.assign_speaker(&[1.0, 0.0, 0.0, 0.0], 0.5, 1.4),
            "Speaker 1"
        );
        assert_eq!(clusterer.current_clusters().len(), 1);
        assert_eq!(clusterer.current_clusters()[0].speaker_id, "Speaker 1");

        // 2. Now load "NutzerIn" profile (pointing towards [0, 1, 0, 0])
        clusterer.upsert_known_profile("NutzerIn".to_string(), vec![0.0, 1.0, 0.0, 0.0]);

        // 3. Finalized segment for 0.5s with clear NutzerIn voice replaces the checkpoint in cluster_0
        let assignment = clusterer.assign_speaker_with_scores(&[0.0, 1.0, 0.0, 0.0], 0.5, 2.5);
        assert_eq!(assignment.speaker_id, "NutzerIn");
        assert_eq!(assignment.cluster_id, "cluster_0");

        // Generic "Speaker 1" label is replaced by "NutzerIn" on cluster_0
        assert_eq!(clusterer.current_clusters().len(), 1);
        assert_eq!(clusterer.current_clusters()[0].speaker_id, "NutzerIn");
    }

    /// Verifies that a short continuation segment right after a 3.0s split retains the calibrated
    /// profile identity even if its similarity is slightly below the strict standalone threshold.
    #[test]
    fn test_profile_continuation_margin_across_split_utterance() {
        let mut clusterer = AhcClusterer::default();
        clusterer.set_recognition_threshold(0.60);
        clusterer.upsert_known_profile("NutzerIn".to_string(), vec![0.0, 1.0, 0.0]);

        // 1. First 3.0s chunk matches NutzerIn cleanly (sim = 1.0)
        assert_eq!(
            clusterer.assign_speaker(&[0.0, 1.0, 0.0], 0.0, 3.0),
            "NutzerIn"
        );

        // 2. Overlapping 0.8s continuation chunk [2.0s, 2.8s] with louder/faster voice (sim ~ 0.54)
        //    Without continuation/short margin it would fall below 0.60, but because it directly
        //    continues NutzerIn's utterance, it stays assigned to NutzerIn.
        assert_eq!(
            clusterer.assign_speaker(&[0.84, 0.54, 0.0], 2.0, 2.8),
            "NutzerIn"
        );
    }

    /// Verifies that cosine_similarity_to_likelihood calibrates to 50% at threshold,
    /// scales monotonically, and clamps properly.
    #[test]
    fn test_cosine_similarity_to_likelihood_calibration() {
        let threshold = 0.52_f32;

        // Exactly at threshold -> 50%
        let at_threshold = cosine_similarity_to_likelihood(threshold, threshold);
        assert!((at_threshold - 0.50).abs() < 1e-4);

        // High similarity (0.75) -> > 90%
        let high = cosine_similarity_to_likelihood(0.75, threshold);
        assert!((0.90..=1.0).contains(&high));

        // Low similarity (0.25) -> < 10%
        let low = cosine_similarity_to_likelihood(0.25, threshold);
        assert!((0.0..0.10).contains(&low));

        // Extreme values clamped in [0.0, 1.0]
        assert_eq!(
            cosine_similarity_to_likelihood(1.0, threshold).clamp(0.0, 1.0),
            cosine_similarity_to_likelihood(1.0, threshold)
        );
        assert!(cosine_similarity_to_likelihood(-1.0, threshold) >= 0.0);
    }

    /// Verifies that assign_speaker_with_scores provides user similarity and calibrated likelihood
    /// when a profile is loaded, and returns None when no profile is present.
    #[test]
    fn test_assign_speaker_with_scores_returns_user_metrics() {
        let mut clusterer = AhcClusterer::default();

        // Without profile -> user_similarity and user_likelihood are None
        let initial = clusterer.assign_speaker_with_scores(&[1.0, 0.0, 0.0], 0.0, 1.0);
        assert_eq!(initial.speaker_id, "Speaker 1");
        assert!(initial.user_similarity.is_none());
        assert!(initial.user_likelihood.is_none());

        // Load "NutzerIn" profile
        clusterer.upsert_known_profile("NutzerIn".to_string(), vec![0.0, 1.0, 0.0]);

        // Strong match for NutzerIn ([0.0, 1.0, 0.0]) -> sim ~ 1.0, likelihood > 95%
        let user_turn = clusterer.assign_speaker_with_scores(&[0.0, 1.0, 0.0], 1.2, 2.5);
        assert_eq!(user_turn.speaker_id, "NutzerIn");
        assert!((user_turn.user_similarity.unwrap() - 1.0).abs() < 1e-4);
        assert!(user_turn.user_likelihood.unwrap() > 0.95);

        // Partner speaking ([1.0, 0.0, 0.0]) -> sim ~ 0.0, likelihood < 5%
        let partner_turn = clusterer.assign_speaker_with_scores(&[1.0, 0.0, 0.0], 2.8, 4.0);
        assert_ne!(partner_turn.speaker_id, "NutzerIn");
        assert!((partner_turn.user_similarity.unwrap() - 0.0).abs() < 1e-4);
        assert!(partner_turn.user_likelihood.unwrap() < 0.05);
    }

    /// Verifies PRD-3 First-Two-Speakers prior: when the user speaks first with a moderate
    /// opening similarity (`sim ≈ 0.445`, which scored ~29% under the old memoryless 0.52 gate),
    /// Turn 1 is immediately recognized as "NutzerIn" under the K=1 opening prior (~68%) and
    /// rises above 90% competitively once Partner speaks at `sim ≈ 0.18`.
    #[test]
    fn test_opening_turn_prior_and_competitive_two_speaker_posterior() {
        let mut clusterer = AhcClusterer::default();
        // Reference "NutzerIn" profile along [1, 0, 0]
        clusterer.upsert_known_profile("NutzerIn".to_string(), vec![1.0, 0.0, 0.0]);

        // Turn 1: User opening turn with cosine similarity 0.445 ([0.445, 0.8955, 0.0])
        let user_opening = [0.445_f32, (1.0_f32 - 0.445 * 0.445).sqrt(), 0.0];
        let turn1 = clusterer.assign_speaker_with_scores(&user_opening, 0.0, 1.1);
        assert_eq!(turn1.cluster_id, "cluster_0");
        assert_eq!(
            turn1.speaker_id, "NutzerIn",
            "Opening turn with sim=0.445 must be recognized as NutzerIn via K=1 opening prior"
        );
        assert!(
            turn1.user_likelihood.unwrap() > 0.65,
            "Opening likelihood should be > 65%, got {:?}",
            turn1.user_likelihood
        );

        // Turn 2: Partner speaks with cosine similarity 0.18 to User and orthogonal to Turn 1's y-component
        let partner_emb = [0.18_f32, 0.0, (1.0_f32 - 0.18 * 0.18).sqrt()];
        let turn2 = clusterer.assign_speaker_with_scores(&partner_emb, 1.8, 3.4);
        assert_eq!(turn2.cluster_id, "cluster_1");
        assert_eq!(turn2.speaker_id, "Speaker 1");
        assert!(
            turn2.user_likelihood.unwrap() < 0.05,
            "Partner likelihood should be < 5%, got {:?}",
            turn2.user_likelihood
        );

        // Moreover, cluster_0's competitive user likelihood should have risen to > 90%
        let relabel_c0 = turn2
            .cluster_relabels
            .iter()
            .find(|r| r.cluster_id == "cluster_0")
            .expect("Expected cluster_0 likelihood update when second speaker appears");
        assert_eq!(relabel_c0.speaker_id, "NutzerIn");
        assert!(relabel_c0.user_likelihood.unwrap() > 0.90);
    }

    /// Verifies retroactive cluster relabeling when Turn 1 is a borderline speaker (`sim = 0.39`,
    /// provisionally assigned "NutzerIn" under K=1) and Turn 2 is the actual user (`sim = 0.72`),
    /// flipping `cluster_0` retroactively to "Speaker 1" and assigning `cluster_1` to "NutzerIn".
    #[test]
    fn test_retroactive_cluster_relabel_when_second_speaker_is_stronger_user_match() {
        let mut clusterer = AhcClusterer::default();
        clusterer.upsert_known_profile("NutzerIn".to_string(), vec![1.0, 0.0, 0.0]);

        // Turn 1: Borderline speaker (sim = 0.39 >= 0.38 opening threshold) -> provisional NutzerIn
        let spk1_emb = [0.39_f32, (1.0_f32 - 0.39 * 0.39).sqrt(), 0.0];
        let turn1 = clusterer.assign_speaker_with_scores(&spk1_emb, 0.0, 1.2);
        assert_eq!(turn1.cluster_id, "cluster_0");
        assert_eq!(turn1.speaker_id, "NutzerIn");

        // Turn 2: Actual user speaks with strong similarity (sim = 0.75) in orthogonal y/z plane
        let spk2_emb = [0.75_f32, 0.0, (1.0_f32 - 0.75 * 0.75).sqrt()];
        let turn2 = clusterer.assign_speaker_with_scores(&spk2_emb, 2.0, 3.6);
        assert_eq!(turn2.cluster_id, "cluster_1");
        assert_eq!(turn2.speaker_id, "NutzerIn");
        assert!(turn2.user_likelihood.unwrap() > 0.95);

        // cluster_0 must be retroactively relabeled from "NutzerIn" to "Speaker 1"
        let relabel_c0 = turn2
            .cluster_relabels
            .iter()
            .find(|r| r.cluster_id == "cluster_0")
            .expect("Expected retroactive relabel for cluster_0");
        assert_eq!(relabel_c0.speaker_id, "Speaker 1");
        assert!(relabel_c0.user_likelihood.unwrap() < 0.15);
    }

    /// Verifies the sanity floor (`MIN_COMPETITIVE_USER_FLOOR = 0.35`): if two strangers speak
    /// while the user remains silent (`sim = 0.22` and `sim = 0.14`), neither is assigned "NutzerIn".
    #[test]
    fn test_two_strangers_below_sanity_floor_remain_partners() {
        let mut clusterer = AhcClusterer::default();
        clusterer.upsert_known_profile("NutzerIn".to_string(), vec![1.0, 0.0, 0.0]);

        let stranger1 = [0.22_f32, (1.0_f32 - 0.22 * 0.22).sqrt(), 0.0];
        let stranger2 = [0.14_f32, 0.0, (1.0_f32 - 0.14 * 0.14).sqrt()];

        let t1 = clusterer.assign_speaker_with_scores(&stranger1, 0.0, 1.5);
        assert_eq!(t1.speaker_id, "Speaker 1");
        assert!(t1.user_likelihood.unwrap() < 0.20);

        let t2 = clusterer.assign_speaker_with_scores(&stranger2, 2.0, 3.5);
        assert_eq!(t2.speaker_id, "Speaker 2");
        assert!(t2.user_likelihood.unwrap() < 0.10);
    }

    /// Verifies that `AhcClusterer` evicts the oldest segments once `MAX_STORED_SEGMENTS` (256)
    /// is exceeded so AHC clustering remains bounded in long sessions.
    #[test]
    fn test_max_stored_segments_sliding_window_eviction() {
        let mut clusterer = AhcClusterer::default();
        for i in 0..265 {
            let start = (i as f64) * 2.0;
            let end = start + 1.0;
            clusterer.assign_speaker(&[1.0, 0.0, 0.0], start, end);
        }

        let summaries = clusterer.current_clusters();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].segment_count, MAX_STORED_SEGMENTS);
        assert!((summaries[0].first_seen_time - 18.0).abs() < 1e-6);
    }

    /// Verifies that when a bridging utterance causes two previously distinct clusters (`cluster_0`
    /// and `cluster_1`) to merge under AHC average linkage, a `ClusterRelabel` is emitted for the
    /// absorbed cluster ID (`cluster_1`).
    #[test]
    fn test_cluster_merge_emits_retroactive_relabel_for_merged_cluster() {
        // Use a 0.75 threshold so two vectors with cos_sim = 0.70 start in separate clusters,
        // and a midpoint vector with cos_sim ~ 0.92 to both merges all three into cluster_0.
        let mut clusterer = AhcClusterer::new(0.75);

        // Segment 1: angle 0 deg -> [1.0, 0.0] (cluster_0)
        let s1 = clusterer.assign_speaker_with_scores(&[1.0, 0.0], 0.0, 1.5);
        assert_eq!(s1.cluster_id, "cluster_0");

        // Segment 2: angle ~45.5 deg -> [0.70, 0.71414] (cos_sim = 0.70 < 0.75 -> cluster_1)
        let s2 = clusterer.assign_speaker_with_scores(&[0.70, 0.71414284], 2.0, 3.5);
        assert_eq!(s2.cluster_id, "cluster_1");
        assert_eq!(clusterer.current_clusters().len(), 2);

        // Segment 3: midpoint angle ~22.7 deg -> [0.922, 0.387] (cos_sim > 0.92 to both -> bridges cluster_0 and cluster_1)
        let s3 = clusterer.assign_speaker_with_scores(&[0.922, 0.387], 4.0, 5.5);
        assert_eq!(s3.cluster_id, "cluster_0");
        assert_eq!(clusterer.current_clusters().len(), 1);

        let merged_relabel = s3
            .cluster_relabels
            .iter()
            .find(|r| r.cluster_id == "cluster_1")
            .expect("Expected ClusterRelabel for absorbed cluster_1");
        assert_eq!(merged_relabel.speaker_id, "Speaker 1");
    }

    /// Verifies that an empty `raw_embedding` slice is handled safely without panicking.
    #[test]
    fn test_assign_speaker_with_empty_embedding_fallback() {
        let mut clusterer = AhcClusterer::default();
        let res = clusterer.assign_speaker_with_scores(&[], 0.0, 1.0);
        assert_eq!(res.cluster_id, "cluster_0");
        assert_eq!(res.speaker_id, "Speaker 1");
        assert!(res.user_similarity.is_none());
        assert!(res.user_likelihood.is_none());
        assert!(res.cluster_relabels.is_empty());
    }
}
