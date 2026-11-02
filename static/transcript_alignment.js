/**
 * @fileoverview Streaming Web Speech API (`SpeechRecognition`) to VAD Speaker Diarization
 * alignment state machine.
 *
 * Handles cross-browser `SpeechRecognition` differences between Safari/WebKit (one monotonic
 * `event.results[i]` slot per utterance) and Chrome/Chromium (which splits a single ongoing
 * utterance across multiple non-final `event.results[i..i+k]` stability slots and then shrinks
 * `event.results.length` when merging them into a finalized slot).
 */

/** Default acoustic + network latency offset (in seconds) subtracted from STT `onresult` arrival times. */
const STT_LATENCY_OFFSET_SEC = 0.28;

/** Minimum elapsed audio duration (in seconds) after a VAD onset required before an STT result can belong to it. */
const MIN_STT_TURN_REACTION_SEC = 0.12;

/** Approximate VAD pre-roll + frame lookback (in seconds) matching Rust `VadStateMachine` (`2 * 32ms + 32ms`). */
const VAD_ONSET_LOOKBACK_SEC = 0.096;

/** Acoustic silence duration (in seconds) required before triggering `/correct-transcript` on an active turn. */
export const CORRECT_TRANSCRIPT_SILENCE_SEC = 1.0;

/** Acoustic silence duration (in seconds) required before locking a turn and triggering `/translate`. */
export const TRANSLATE_LOCK_SILENCE_SEC = 2.0;

/** Minimum confidence score (`0.0–1.0`) required to promote an inferred `partnerRole.roleType` as card title. */
export const HIGH_ROLE_CONFIDENCE_THRESHOLD = 0.80;

/** Post-playback acoustic tail (in milliseconds) before live recording resumes ingestion. */
export const TTS_ECHO_GATE_TAIL_MS = 400;

/** Default speech synthesis rate for Leichte Sprache reading. */
export const TTS_DEFAULT_RATE = 0.90;

/**
 * Normalizes Leichte Sprache Markdown content into fluent, natural spoken German for speech synthesis.
 *
 * Strips formatting tokens (`**`, `*`, `##`, `* ` / `- `), joins intra-compound hyphens
 * (`Kranken-Versicherung` -> `Krankenversicherung`), replaces Mediopunkts (`Blut·druck` -> `Blutdruck`),
 * and ensures sentence-ending punctuation (`.` / `?` / `:`) on lines without punctuation.
 *
 * @param {string | null | undefined} markdown
 * @returns {string}
 */
export function normalizeLeichteSpracheForSpeech(markdown) {
  if (!markdown || typeof markdown !== 'string') {
    return '';
  }

  const lines = markdown.split(/\r?\n/);
  const spokenLines = [];

  for (let rawLine of lines) {
    let line = rawLine.trim();
    if (!line) {
      continue;
    }

    // Strip Markdown headings (e.g. `## Wichtig`)
    line = line.replace(/^#{1,6}\s+/, '');

    // Strip unordered list bullets (e.g. `* ` or `- `)
    line = line.replace(/^[*+-]\s+/, '');

    // Strip bold and italic markers (e.g. `**Wort**` -> `Wort`, `*Wort*` -> `Wort`)
    line = line.replace(/\*\*([^*]+)\*\*/g, '$1');
    line = line.replace(/\*([^*]+)\*/g, '$1');

    // Replace Mediopunkt (· / \u00B7) with standard compound word join
    line = line.replace(/(\p{L})·(\p{L})/gu, '$1$2');

    // Join Leichte Sprache intra-compound hyphens (<lowercase>-<Uppercase><lowercase>, e.g. Kranken-Versicherung -> Krankenversicherung)
    // while preserving acronym/prefix hyphens (e.g. E-Mail, LKW-Fahrer, UV-Licht)
    line = line.replace(/(\p{Ll})-([A-ZÄÖÜ][a-zäöüß])/gu, (_match, p1, p2) => {
      return p1 + p2.toLowerCase();
    });

    // Collapse multiple internal whitespaces
    line = line.replace(/\s+/g, ' ').trim();
    if (!line) {
      continue;
    }

    // Ensure line terminates with a punctuation boundary for natural breath pauses
    if (!/[.?!:;,]$/.test(line)) {
      line += '.';
    }

    spokenLines.push(line);
  }

  return spokenLines.join(' ');
}

/**
 * Selects an installed German SpeechSynthesis voice guided by inferred partner role and partner number.
 *
 * @param {SpeechSynthesisVoice[]} voices
 * @param {string | null | undefined} partnerRoleType Inferred role (e.g. "Arzt", "Ärztin")
 * @param {number | null | undefined} partnerNumber 1-based partner index
 * @returns {SpeechSynthesisVoice | null}
 */
export function selectGermanVoice(voices, partnerRoleType, partnerNumber) {
  if (!Array.isArray(voices) || voices.length === 0) {
    return null;
  }

  // Filter German voices (de-DE, de-AT, de-CH, de)
  const deVoices = voices.filter(v => v.lang && v.lang.toLowerCase().startsWith('de'));
  if (deVoices.length === 0) {
    // Fall back to default voice or first available
    return voices.find(v => v.default) || voices[0] || null;
  }

  // Check role gender hints
  let targetGender = 'neutral';
  if (partnerRoleType && typeof partnerRoleType === 'string') {
    const r = partnerRoleType.toLowerCase();
    if (r.endsWith('in') || r.endsWith('frau') || r.includes('mutter') || r.includes('tochter') || r.includes('ärztin')) {
      targetGender = 'female';
    } else if (r.includes('arzt') || r.includes('mann') || r.includes('vater') || r.includes('sohn') || r.includes('herr') || r.includes('doktor')) {
      targetGender = 'male';
    }
  }

  const femaleNamePatterns = [/female/i, /marlene/i, /katja/i, /anna/i, /helena/i, /petra/i, /vicki/i, /standard-a/i, /wavenet-a/i, /wavenet-c/i, /neural2-a/i, /neural2-c/i];
  const maleNamePatterns = [/male/i, /martin/i, /stefan/i, /yannick/i, /hans/i, /markus/i, /standard-b/i, /wavenet-b/i, /wavenet-d/i, /neural2-b/i, /neural2-d/i];

  if (targetGender === 'female') {
    const fMatch = deVoices.find(v => femaleNamePatterns.some(p => p.test(v.name)));
    if (fMatch) return fMatch;
  } else if (targetGender === 'male') {
    const mMatch = deVoices.find(v => maleNamePatterns.some(p => p.test(v.name)));
    if (mMatch) return mMatch;
  }

  // If partnerNumber is provided and > 1, cycle through distinct German voices if available
  const pNum = (typeof partnerNumber === 'number' && partnerNumber >= 1) ? partnerNumber : 1;
  const voiceIdx = (pNum - 1) % deVoices.length;
  return deVoices[voiceIdx] || deVoices[0];
}

/**
 * Tracks whether audio is currently playing and enforces a post-playback acoustic tail
 * before live microphone VAD / STT turn ingestion resumes.
 */
export class TtsEchoGate {
  /**
   * @param {number} [tailMs=400]
   */
  constructor(tailMs = TTS_ECHO_GATE_TAIL_MS) {
    this.tailMs = tailMs;
    this.isPlaying = false;
    this.lastPlaybackEndPerfMs = 0;
  }

  startPlayback() {
    this.isPlaying = true;
  }

  stopPlayback(nowPerfMs = performance.now()) {
    this.isPlaying = false;
    this.lastPlaybackEndPerfMs = nowPerfMs;
  }

  /**
   * Returns true if audio is actively playing or within the post-playback echo tail.
   *
   * @param {number} [nowPerfMs=performance.now()]
   * @returns {boolean}
   */
  isRecordingGated(nowPerfMs = performance.now()) {
    if (this.isPlaying) {
      return true;
    }
    return (nowPerfMs - this.lastPlaybackEndPerfMs) < this.tailMs;
  }
}

/**
 * Global single-playback controller for SpeechSynthesis.
 * Ensures only one card plays at any given moment and coordinates button UI states.
 */
export class TtsPlaybackController {
  /**
   * @param {object} [options]
   * @param {TtsEchoGate} [options.echoGate]
   */
  constructor(options = {}) {
    this.echoGate = options.echoGate || new TtsEchoGate();
    this.activeCardId = null;
    this.activeUtterance = null;
    /** @type {(() => void) | null} */
    this.activeStateChangeCb = null;
  }

  /**
   * Returns true if speech synthesis is currently active.
   */
  isSpeaking() {
    return this.activeCardId !== null;
  }

  /**
   * Returns true if recording should be gated to avoid acoustic echo.
   */
  isRecordingGated(nowPerfMs = performance.now()) {
    return this.echoGate.isRecordingGated(nowPerfMs);
  }

  /**
   * Stops any currently active speech synthesis utterance.
   */
  stop() {
    if (typeof window !== 'undefined' && window.speechSynthesis) {
      try {
        window.speechSynthesis.cancel();
      } catch (_e) {}
    }
    if (this.activeStateChangeCb) {
      const cb = this.activeStateChangeCb;
      this.activeStateChangeCb = null;
      try {
        cb();
      } catch (_e) {}
    }
    this.activeCardId = null;
    this.activeUtterance = null;
    this.echoGate.stopPlayback();
  }

  /**
   * Speaks normalized text for a given card ID using native window.speechSynthesis.
   *
   * @param {object} params
   * @param {string | number} params.cardId Unique card identifier
   * @param {string} params.text Markdown or plain text
   * @param {string} [params.partnerRole] Inferred partner role (e.g. "Arzt", "Ärztin")
   * @param {number} [params.partnerNumber] 1-based partner number
   * @param {() => void} [params.onStateChange] Called when playback starts or stops
   */
  speak(params) {
    if (typeof window === 'undefined' || !window.speechSynthesis) {
      console.warn('SpeechSynthesis is not supported in this browser.');
      return;
    }

    const { cardId, text, partnerRole, partnerNumber, onStateChange } = params;

    // If already speaking on this exact card, stop it
    if (this.activeCardId === cardId) {
      this.stop();
      return;
    }

    // Stop any existing playback on another card first
    this.stop();

    const spokenText = normalizeLeichteSpracheForSpeech(text);
    if (!spokenText.trim()) {
      return;
    }

    const utterance = new SpeechSynthesisUtterance(spokenText);
    utterance.lang = 'de-DE';
    utterance.rate = TTS_DEFAULT_RATE;

    const voices = window.speechSynthesis.getVoices();
    const chosenVoice = selectGermanVoice(voices, partnerRole, partnerNumber);
    if (chosenVoice) {
      utterance.voice = chosenVoice;
    }

    this.activeCardId = cardId;
    this.activeUtterance = utterance;
    this.activeStateChangeCb = onStateChange || null;
    this.echoGate.startPlayback();

    const cleanup = () => {
      if (this.activeCardId === cardId) {
        this.activeCardId = null;
        this.activeUtterance = null;
        this.echoGate.stopPlayback();
        if (this.activeStateChangeCb) {
          const cb = this.activeStateChangeCb;
          this.activeStateChangeCb = null;
          try {
            cb();
          } catch (_e) {}
        }
      }
    };

    utterance.onend = () => cleanup();
    utterance.onerror = (e) => {
      // Ignore user cancellation errors
      if (e.error !== 'canceled' && e.error !== 'interrupted') {
        console.warn('SpeechSynthesis error:', e);
      }
      cleanup();
    };

    if (this.activeStateChangeCb) {
      try {
        this.activeStateChangeCb();
      } catch (_e) {}
    }

    window.speechSynthesis.speak(utterance);
  }
}


/**
 * Extracts a positive integer partner number from a speaker identifier (e.g. `"Speaker 2"` -> `2`),
 * or returns `null` if not a numbered partner speaker.
 *
 * @param {string | null | undefined} speakerId
 * @returns {number | null}
 */
export function extractPartnerNumberFromSpeakerId(speakerId) {
  if (!speakerId || typeof speakerId !== 'string' || speakerId === 'NutzerIn') {
    return null;
  }
  const m = /(\d+)$/.exec(speakerId.trim());
  if (!m) {
    return null;
  }
  const num = Number(m[1]);
  return Number.isInteger(num) && num >= 1 ? num : null;
}

/**
 * Tracks active partner speakers, dynamic single- vs. multi-partner display numbering,
 * and sticky high-confidence (`>= 0.80`) contextual role titles (`roleType`) across turns.
 */
export class PartnerRoleTracker {
  constructor() {
    /**
     * Ordered map of active partner speaker IDs (`"Speaker 1"`, `"Speaker 2"`, …) to their
     * contiguous 1-based display number (`1, 2, ...`).
     * @type {Map<string, number>}
     */
    this.partnerNumbers = new Map();
    /**
     * Sticky high-confidence role assignments per partner speaker ID.
     * @type {Map<string, {roleType: string, confidence: number}>}
     */
    this.inferredRoles = new Map();
  }

  /**
   * Resets all tracked partner numbers and inferred roles (e.g. on `"Neuer Dialog"`).
   */
  reset() {
    this.partnerNumbers.clear();
    this.inferredRoles.clear();
  }

  /**
   * Synchronizes the set of active partner `speakerId`s present in the UI, preserving
   * first-appearance order for existing partners and assigning contiguous `1..K` numbers.
   *
   * @param {Iterable<string | null | undefined>} speakerIds
   */
  syncActiveSpeakers(speakerIds) {
    const activeSet = new Set();
    if (speakerIds) {
      for (const sid of speakerIds) {
        if (typeof sid === 'string' && sid.trim().length > 0 && sid !== 'NutzerIn') {
          activeSet.add(sid);
        }
      }
    }

    const ordered = [];
    for (const existingSid of this.partnerNumbers.keys()) {
      if (activeSet.has(existingSid)) {
        ordered.push(existingSid);
        activeSet.delete(existingSid);
      }
    }
    const remaining = Array.from(activeSet).sort((a, b) => {
      const na = extractPartnerNumberFromSpeakerId(a) ?? Number.MAX_SAFE_INTEGER;
      const nb = extractPartnerNumberFromSpeakerId(b) ?? Number.MAX_SAFE_INTEGER;
      return na !== nb ? na - nb : a.localeCompare(b);
    });
    for (const sid of remaining) {
      ordered.push(sid);
    }

    this.partnerNumbers.clear();
    ordered.forEach((sid, idx) => {
      this.partnerNumbers.set(sid, idx + 1);
    });

    for (const sid of Array.from(this.inferredRoles.keys())) {
      if (!this.partnerNumbers.has(sid)) {
        this.inferredRoles.delete(sid);
      }
    }
  }

  /**
   * Returns the total number of distinct partner speakers currently active in the UI.
   *
   * @returns {number}
   */
  get distinctPartnerCount() {
    return this.partnerNumbers.size;
  }

  /**
   * Returns the 1-based partner number for `speakerId` (registering it if needed),
   * or `null` if `speakerId` is `"NutzerIn"` or empty.
   *
   * @param {string | null | undefined} speakerId
   * @returns {number | null}
   */
  getPartnerNumber(speakerId) {
    if (!speakerId || typeof speakerId !== 'string' || speakerId === 'NutzerIn') {
      return null;
    }
    if (!this.partnerNumbers.has(speakerId)) {
      this.partnerNumbers.set(speakerId, this.partnerNumbers.size + 1);
    }
    return this.partnerNumbers.get(speakerId) ?? null;
  }

  /**
   * Evaluates a `/translate` `partnerRole` payload (`{ roleType, confidence }`) for `speakerId`.
   * Promotes and caches `roleType` when `confidence >= 0.80` and either no role is stored yet
   * or `confidence >= existing.confidence - 0.05`.
   *
   * @param {string | null | undefined} speakerId
   * @param {{roleType?: string | null, confidence?: number | null} | null | undefined} partnerRole
   * @returns {boolean} `true` if the speaker's display role title changed.
   */
   ingestPartnerRole(speakerId, partnerRole) {
    if (
      !speakerId ||
      speakerId === 'NutzerIn' ||
      !partnerRole ||
      typeof partnerRole.roleType !== 'string'
    ) {
      return false;
    }
    const roleType = partnerRole.roleType.trim();
    const confidence = Number(partnerRole.confidence);
    if (
      !roleType ||
      !Number.isFinite(confidence) ||
      confidence < HIGH_ROLE_CONFIDENCE_THRESHOLD
    ) {
      return false;
    }
    this.getPartnerNumber(speakerId);
    const existing = this.inferredRoles.get(speakerId);
    if (!existing || confidence >= existing.confidence - 0.05) {
      const changed = !existing || existing.roleType !== roleType;
      this.inferredRoles.set(speakerId, {
        roleType,
        confidence: Math.max(confidence, existing ? existing.confidence : confidence),
      });
      return changed;
    }
    return false;
  }

  /**
   * Formats the UI card header title for `speakerId`.
   *
   * - `null` / `undefined`: `"Sprecher wird erkannt…"`
   * - `"NutzerIn"`: `"NutzerIn"` (or `"NutzerIn"` when `isLeichte` is false)
   * - Partner with high-confidence inferred role: `"<roleType>"` or `"<roleType> (Leichte Sprache)"`
   * - Single active partner (`distinctPartnerCount <= 1`): `"GesprächspartnerIn"` or `"GesprächspartnerIn (Leichte Sprache)"`
   * - Multiple active partners (`distinctPartnerCount >= 2`): `"GesprächspartnerIn N"` or `"GesprächspartnerIn N (Leichte Sprache)"`
   *
   * @param {string | null | undefined} speakerId
   * @param {boolean} [isLeichte=false]
   * @returns {string}
   */
  formatCardTitle(speakerId, isLeichte = false) {
    if (!speakerId) {
      return 'Sprecher wird erkannt…';
    }
    if (speakerId === 'NutzerIn') {
      return 'NutzerIn';
    }
    const suffix = isLeichte ? ' (Leichte Sprache)' : '';
    const inferred = this.inferredRoles.get(speakerId);
    if (inferred && inferred.roleType) {
      return `${inferred.roleType}${suffix}`;
    }
    const num =
      this.partnerNumbers.get(speakerId) ?? extractPartnerNumberFromSpeakerId(speakerId) ?? 1;
    if (this.distinctPartnerCount <= 1) {
      return `GesprächspartnerIn${suffix}`;
    }
    return `GesprächspartnerIn ${num}${suffix}`;
  }
}

/**
 * Maps a diarized speaker identifier (`"NutzerIn"`, `"Speaker 1"`, `"Speaker 2"`, …) to the
 * backend conversation API role (`"user"` or `"partner"`).
 *
 * @param {string} speakerId
 * @returns {'user' | 'partner'}
 */
export function speakerIdToApiRole(speakerId) {
  return speakerId === 'NutzerIn' ? 'user' : 'partner';
}

/**
 * Normalizes a completed `/translate` API response `content` string for storage in a
 * `role: "partner_simple"` conversation history entry (matching `docs/diwop-api-contract.md`
 * and `templates/dialog_translation.html`). Returns `null` if `content` is blank.
 *
 * @param {string} content
 * @returns {string | null}
 */
export function normalizePartnerSimpleContent(content) {
  if (!content || typeof content !== 'string') {
    return null;
  }
  return content.trim().length > 0 ? content : null;
}

/**
 * Builds the chronological `messages` array for `POST /translate` up to and including `targetTurnId`.
 *
 * - Uses `correctedMessage` from `/correct-transcript` whenever already available for `user` and
 *   `partner` entries (including `targetTurnId` if its `1.0s` correction already completed before
 *   the `2.0s` translation trigger or before a retry), falling back to `rawMessage` otherwise.
 * - Includes `role: "partner_simple"` (`data.content`) immediately after each preceding `partner`
 *   turn whose `/translate` call has already completed.
 * - Guarantees that the final message in the returned array is the `partner` turn for `targetTurnId`.
 *
 * @param {Array<{
 *   turnId: number,
 *   role: 'user' | 'partner',
 *   rawMessage: string,
 *   correctedMessage?: string | null,
 *   partnerSimpleMessage?: string | null
 * }>} conversationTurns
 * @param {number} targetTurnId
 * @returns {Array<{role: 'user' | 'partner' | 'partner_simple', message: string}>}
 */
export function buildTranslateHistoryMessages(conversationTurns, targetTurnId) {
  if (!Array.isArray(conversationTurns) || conversationTurns.length === 0) {
    return [];
  }
  const messages = [];
  for (const entry of conversationTurns) {
    if (!entry) {
      continue;
    }
    const effectiveText =
      typeof entry.correctedMessage === 'string' && entry.correctedMessage.trim().length > 0
        ? entry.correctedMessage
        : entry.rawMessage;
    if (typeof effectiveText !== 'string' || !effectiveText.trim()) {
      continue;
    }
    /** @type {{role: 'user' | 'partner' | 'partner_simple', message: string, partnerNumber?: number}} */
    const msgItem = {
      role: entry.role,
      message: effectiveText,
    };
    const pNum =
      typeof entry.partnerNumber === 'number' &&
      Number.isInteger(entry.partnerNumber) &&
      entry.partnerNumber >= 1
        ? entry.partnerNumber
        : extractPartnerNumberFromSpeakerId(entry.speakerId);
    if (entry.role === 'partner' && pNum !== null) {
      msgItem.partnerNumber = pNum;
    }
    messages.push(msgItem);
    if (entry.turnId === targetTurnId) {
      break;
    }
    if (
      entry.role === 'partner' &&
      typeof entry.partnerSimpleMessage === 'string' &&
      entry.partnerSimpleMessage.trim().length > 0
    ) {
      messages.push({
        role: 'partner_simple',
        message: entry.partnerSimpleMessage,
      });
    }
  }
  return messages;
}

/**
 * Builds the chronological `history` array for `POST /correct-transcript` ending with `targetTurn`.
 *
 * - Preceding turns from `conversationTurns` (with `turnId !== targetTurn.turnId` and appearing
 *   before `targetTurn.turnId`) are included as read-only context using `correctedMessage` when
 *   available (falling back to `rawMessage`), along with optional `clusterId`, `userLikelihood`,
 *   `pauseMs`, and any completed `partner_simple` translations.
 * - The final item in the returned array is `targetTurn` (`role: 'user' | 'partner'` and its raw `message`).
 *
 * @param {Array<{
 *   turnId: number,
 *   role: 'user' | 'partner',
 *   rawMessage: string,
 *   correctedMessage?: string | null,
 *   partnerSimpleMessage?: string | null,
 *   clusterId?: string | null,
 *   userLikelihood?: number | null,
 *   pauseMs?: number | null
 * }>} conversationTurns
 * @param {{
 *   turnId?: number,
 *   role?: 'user' | 'partner',
 *   speakerId?: string | null,
 *   rawText?: string,
 *   rawMessage?: string,
 *   clusterId?: string | null,
 *   userLikelihood?: number | null,
 *   pauseMs?: number | null
 * }} targetTurn
 * @returns {Array<{
 *   role: 'user' | 'partner' | 'partner_simple',
 *   message: string,
 *   clusterId?: string,
 *   userLikelihood?: number,
 *   pauseMs?: number
 * }>}
 */
export function buildCorrectTranscriptHistory(conversationTurns, targetTurn) {
  const history = [];
  const targetId =
    targetTurn && typeof targetTurn.turnId === 'number' ? targetTurn.turnId : Infinity;

  if (Array.isArray(conversationTurns)) {
    for (const entry of conversationTurns) {
      if (!entry) {
        continue;
      }
      if (typeof entry.turnId === 'number' && entry.turnId >= targetId) {
        break;
      }
      const effectiveText =
        typeof entry.correctedMessage === 'string' && entry.correctedMessage.trim().length > 0
          ? entry.correctedMessage
          : entry.rawMessage;
      if (typeof effectiveText !== 'string' || !effectiveText.trim()) {
        continue;
      }
      /** @type {{role: 'user' | 'partner' | 'partner_simple', message: string, partnerNumber?: number, clusterId?: string, userLikelihood?: number, pauseMs?: number}} */
      const item = {
        role: entry.role,
        message: effectiveText,
      };
      const entryPartnerNum =
        typeof entry.partnerNumber === 'number' &&
        Number.isInteger(entry.partnerNumber) &&
        entry.partnerNumber >= 1
          ? entry.partnerNumber
          : extractPartnerNumberFromSpeakerId(entry.speakerId);
      if (entry.role === 'partner' && entryPartnerNum !== null) {
        item.partnerNumber = entryPartnerNum;
      }
      if (typeof entry.clusterId === 'string' && entry.clusterId.trim().length > 0) {
        item.clusterId = entry.clusterId.trim();
      }
      if (
        typeof entry.userLikelihood === 'number' &&
        Number.isFinite(entry.userLikelihood) &&
        entry.userLikelihood >= 0 &&
        entry.userLikelihood <= 1
      ) {
        item.userLikelihood = entry.userLikelihood;
      }
      if (
        typeof entry.pauseMs === 'number' &&
        Number.isFinite(entry.pauseMs) &&
        entry.pauseMs >= 0
      ) {
        item.pauseMs = Math.round(entry.pauseMs);
      }
      history.push(item);

      const partnerSimple =
        typeof entry.partnerSimpleMessage === 'string' &&
        entry.partnerSimpleMessage.trim().length > 0
          ? entry.partnerSimpleMessage
          : typeof entry.partnerSimpleText === 'string' &&
              entry.partnerSimpleText.trim().length > 0
            ? entry.partnerSimpleText
            : null;
      if (entry.role === 'partner' && partnerSimple !== null) {
        history.push({
          role: 'partner_simple',
          message: partnerSimple,
        });
      }
    }
  }

  if (targetTurn) {
    const rawMsg =
      typeof targetTurn.rawText === 'string'
        ? targetTurn.rawText
        : typeof targetTurn.rawMessage === 'string'
          ? targetTurn.rawMessage
          : '';
    if (rawMsg.trim().length > 0) {
      const targetRole =
        targetTurn.role === 'user' || targetTurn.role === 'partner'
          ? targetTurn.role
          : speakerIdToApiRole(targetTurn.speakerId || '');
      /** @type {{role: 'user' | 'partner', message: string, partnerNumber?: number, clusterId?: string, userLikelihood?: number, pauseMs?: number}} */
      const targetItem = {
        role: targetRole,
        message: rawMsg,
      };
      const targetPartnerNum =
        typeof targetTurn.partnerNumber === 'number' &&
        Number.isInteger(targetTurn.partnerNumber) &&
        targetTurn.partnerNumber >= 1
          ? targetTurn.partnerNumber
          : extractPartnerNumberFromSpeakerId(targetTurn.speakerId);
      if (targetRole === 'partner' && targetPartnerNum !== null) {
        targetItem.partnerNumber = targetPartnerNum;
      }
      if (typeof targetTurn.clusterId === 'string' && targetTurn.clusterId.trim().length > 0) {
        targetItem.clusterId = targetTurn.clusterId.trim();
      }
      if (
        typeof targetTurn.userLikelihood === 'number' &&
        Number.isFinite(targetTurn.userLikelihood) &&
        targetTurn.userLikelihood >= 0 &&
        targetTurn.userLikelihood <= 1
      ) {
        targetItem.userLikelihood = targetTurn.userLikelihood;
      }
      if (
        typeof targetTurn.pauseMs === 'number' &&
        Number.isFinite(targetTurn.pauseMs) &&
        targetTurn.pauseMs >= 0
      ) {
        targetItem.pauseMs = Math.round(targetTurn.pauseMs);
      }
      history.push(targetItem);
    }
  }

  return history;
}

/**
 * Splits a raw transcript string into whitespace-delimited display tokens.
 *
 * @param {string} rawText
 * @returns {string[]}
 */
export function tokenizeTranscript(rawText) {
  if (!rawText || typeof rawText !== 'string') {
    return [];
  }
  return rawText.trim().split(/\s+/).filter(Boolean);
}

/**
 * Lowercases a token and strips leading/trailing punctuation while keeping internal letters/digits.
 *
 * @param {string} token
 * @returns {string}
 */
export function normalizeToken(token) {
  if (!token) {
    return '';
  }
  return token
    .toLowerCase()
    .replace(/^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu, '');
}

/**
 * Removes internal apostrophes and hyphens from a normalized token for fuzzy stem comparison
 * (e.g. `"geht's"` and `"gehts"` both canonicalize to `"gehts"`).
 *
 * @param {string} normalized
 * @returns {string}
 */
function canonicalizeForMatch(normalized) {
  return normalized ? normalized.replace(/['’`-]/g, '') : '';
}

/**
 * Computes Hamming distance between two equal-length strings, short-circuiting above 1 difference.
 *
 * @param {string} a
 * @param {string} b
 * @returns {number}
 */
function boundedHammingDistance1(a, b) {
  let diff = 0;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) {
      diff += 1;
      if (diff > 1) {
        return diff;
      }
    }
  }
  return diff;
}

/**
 * Scores similarity between a previous interim token and a current token during streaming ASR.
 * Returns `> 0` when tokens represent the same spoken word (exact, prefix extension, or 1-char revision).
 *
 * @param {string} prevClean
 * @param {string} currClean
 * @param {boolean} sameSlot
 * @returns {number}
 */
function tokenMatchScore(prevClean, currClean, sameSlot) {
  if (!prevClean || !currClean) {
    return 0;
  }
  const slotBonus = sameSlot ? 0.05 : 0;
  if (prevClean === currClean) {
    return 2.0 + slotBonus;
  }
  if (
    prevClean.length >= 3 &&
    currClean.length >= 3 &&
    (prevClean.startsWith(currClean) || currClean.startsWith(prevClean))
  ) {
    return 1.0 + slotBonus;
  }
  if (
    prevClean.length >= 4 &&
    prevClean.length === currClean.length &&
    boundedHammingDistance1(prevClean, currClean) === 1
  ) {
    return 1.0 + slotBonus;
  }
  return 0;
}

/**
 * Computes an order-preserving sequence alignment from `prevWords` to `currTokens` using
 * common-prefix fast-path stripping followed by bottom-up dynamic programming (weighted LCS).
 *
 * @param {Array<{clean: string, slotIndex: number}>} prevWords
 * @param {Array<{clean: string, slotIndex: number}>} currTokens
 * @returns {Int32Array} Array of length `currTokens.length` mapping each current token index to a matched
 *   index in `prevWords`, or `-1` if unmatched.
 */
function matchTokenSequences(prevWords, currTokens) {
  const m = prevWords.length;
  const n = currTokens.length;
  const matchedPrevIdx = new Int32Array(n).fill(-1);
  if (m === 0 || n === 0) {
    return matchedPrevIdx;
  }

  let prefixLen = 0;
  while (
    prefixLen < m &&
    prefixLen < n &&
    prevWords[prefixLen].clean.length > 0 &&
    prevWords[prefixLen].clean === currTokens[prefixLen].clean
  ) {
    matchedPrevIdx[prefixLen] = prefixLen;
    prefixLen += 1;
  }

  const prevRem = m - prefixLen;
  const currRem = n - prefixLen;
  if (prevRem === 0 || currRem === 0) {
    return matchedPrevIdx;
  }

  const cols = currRem + 1;
  const dp = new Float64Array((prevRem + 1) * cols);

  for (let p = prevRem - 1; p >= 0; p--) {
    const prevItem = prevWords[prefixLen + p];
    const rowOffset = p * cols;
    const nextRowOffset = (p + 1) * cols;
    for (let c = currRem - 1; c >= 0; c--) {
      const currItem = currTokens[prefixLen + c];
      const s = tokenMatchScore(
        prevItem.clean,
        currItem.clean,
        prevItem.slotIndex === currItem.slotIndex
      );
      let best = Math.max(dp[nextRowOffset + c], dp[rowOffset + c + 1]);
      if (s > 0) {
        const cand = s + dp[nextRowOffset + c + 1];
        if (cand >= best) {
          best = cand;
        }
      }
      dp[rowOffset + c] = best;
    }
  }

  let p = 0;
  let c = 0;
  while (p < prevRem && c < currRem) {
    const prevItem = prevWords[prefixLen + p];
    const currItem = currTokens[prefixLen + c];
    const s = tokenMatchScore(
      prevItem.clean,
      currItem.clean,
      prevItem.slotIndex === currItem.slotIndex
    );
    const rowOffset = p * cols;
    const nextRowOffset = (p + 1) * cols;
    const matchVal = s > 0 ? s + dp[nextRowOffset + c + 1] : -1;

    if (
      s > 0 &&
      matchVal >= dp[nextRowOffset + c] - 1e-9 &&
      matchVal >= dp[rowOffset + c + 1] - 1e-9
    ) {
      matchedPrevIdx[prefixLen + c] = prefixLen + p;
      p += 1;
      c += 1;
    } else if (dp[nextRowOffset + c] >= dp[rowOffset + c + 1]) {
      p += 1;
    } else {
      c += 1;
    }
  }

  return matchedPrevIdx;
}

/**
 * Stateful aligner that reconciles streaming `SpeechRecognition` results across epochs with
 * asynchronous WASM VAD + speaker diarization intervals.
 */
export class TranscriptAligner {
  /**
   * Initializes an empty alignment session.
   */
  constructor() {
    /** @type {Array<{startTime: number, endTime: number, speakerId: string, isCheckpoint: boolean}>} */
    this.diarizedSegments = [];
    /**
     * Per-epoch recognition state preserving insertion order.
     * @type {Map<number, {
     *   finalizedByIndex: Map<number, {words: Array<{text: string, norm: string, clean: string, timeSec: number, slotIndex: number, lockedTurnId: number | null}>, isFinal: boolean}>,
     *   promotedUtterances: Array<{words: Array<{text: string, norm: string, clean: string, timeSec: number, slotIndex: number, lockedTurnId: number | null}>, isFinal: boolean}>,
     *   interimWords: Array<{text: string, norm: string, clean: string, timeSec: number, slotIndex: number, lockedTurnId: number | null}>
     * }>}
     */
    this.epochStates = new Map();
    /**
     * Immutable turns permanently locked after `>= 2.0s` acoustic silence, speaker switch, or recording stop.
     * @type {Array<{
     *   turnId: number,
     *   speakerId: string,
     *   parts: Array<{text: string, isInterim: boolean}>,
     *   rawText: string,
     *   hasInterim: boolean,
     *   shouldCorrect: boolean,
     *   locked: boolean,
     *   speechEndSec: number
     * }>}
     */
    this.lockedTurns = [];
    /**
     * Session-level `clusterId -> { role, speakerId, partnerSpeakerId }` bindings learned when
     * `/correct-transcript` resolves a borderline speaker (`0.10 < userLikelihood < 0.80`).
     * @type {Map<string, {role: 'user' | 'partner', speakerId: string, partnerSpeakerId: string}>}
     */
    this.clusterRoleBindings = new Map();
    this.nextTurnId = 1;
    this.lastSessionTimeSec = 0;
    this.isRecording = false;
    this.isVadSpeaking = false;
    /** @type {number | null} */
    this.activeVadStartSec = null;
    /** @type {number | null} */
    this.lastVadStartSec = null;
    /** @type {number | null} */
    this.lastVoicedVadSec = null;
  }

  /**
   * Clears all utterances, locked turns, diarized segments, and VAD turn markers for a fresh recording session.
   */
  reset() {
    this.diarizedSegments = [];
    this.epochStates.clear();
    this.lockedTurns = [];
    this.clusterRoleBindings.clear();
    this.nextTurnId = 1;
    this.lastSessionTimeSec = 0;
    this.isRecording = false;
    this.isVadSpeaking = false;
    this.activeVadStartSec = null;
    this.lastVadStartSec = null;
    this.lastVoicedVadSec = null;
  }

  /**
   * Returns `true` if any transcript words or diarized segments have been recorded in this session.
   *
   * @returns {boolean}
   */
  hasContent() {
    if (this.diarizedSegments.length > 0) {
      return true;
    }
    for (const state of this.epochStates.values()) {
      if (state.interimWords.length > 0 || state.promotedUtterances.length > 0) {
        return true;
      }
      for (const utt of state.finalizedByIndex.values()) {
        if (utt.words.length > 0) {
          return true;
        }
      }
    }
    return false;
  }

  /**
   * Updates whether live microphone capture is currently active.
   *
   * @param {boolean} recording
   */
  setRecording(recording) {
    this.isRecording = Boolean(recording);
    if (!this.isRecording) {
      this.isVadSpeaking = false;
      this.activeVadStartSec = null;
    }
  }

  /**
   * Resolves a stable `'Speaker N'` label for `clusterId` when a cluster is assigned to `'partner'`.
   *
   * @param {string | null} clusterId
   * @param {string | null} fallbackSpeakerId
   * @returns {string}
   */
  _resolvePartnerSpeakerIdForCluster(clusterId, fallbackSpeakerId) {
    if (fallbackSpeakerId && fallbackSpeakerId !== 'NutzerIn') {
      return fallbackSpeakerId;
    }
    if (clusterId) {
      const existingBinding = this.clusterRoleBindings.get(clusterId);
      if (existingBinding && existingBinding.partnerSpeakerId) {
        return existingBinding.partnerSpeakerId;
      }
      for (const seg of this.diarizedSegments) {
        if (seg.clusterId === clusterId && seg.speakerId && seg.speakerId !== 'NutzerIn') {
          return seg.speakerId;
        }
      }
    }
    const usedNumbers = new Set();
    for (const seg of this.diarizedSegments) {
      if (seg.clusterId !== clusterId && typeof seg.speakerId === 'string') {
        const m = /^Speaker\s+(\d+)$/.exec(seg.speakerId);
        if (m) {
          usedNumbers.add(Number(m[1]));
        }
      }
    }
    let n = 1;
    while (usedNumbers.has(n)) {
      n += 1;
    }
    return `Speaker ${n}`;
  }

  /**
   * Merges newly emitted or updated diarized speaker segments from the Rust WASM engine.
   * Replaces matching checkpoints (`|startTime - seg.startTime| < 0.08s`) in place and keeps
   * segments sorted chronologically.
   *
   * @param {Array<{startTime: number, endTime: number, speakerId: string, isCheckpoint?: boolean}>} newSegments
   */
  ingestDiarizedSegments(newSegments) {
    if (!Array.isArray(newSegments) || newSegments.length === 0) {
      return;
    }

    for (const rawSeg of newSegments) {
      const seg = {
        startTime: Number(rawSeg.startTime),
        endTime: Number(rawSeg.endTime),
        clusterId: typeof rawSeg.clusterId === 'string' ? rawSeg.clusterId : null,
        speakerId: rawSeg.speakerId,
        isCheckpoint: Boolean(rawSeg.isCheckpoint),
        userSimilarity:
          typeof rawSeg.userSimilarity === 'number' && Number.isFinite(rawSeg.userSimilarity)
            ? Number(rawSeg.userSimilarity)
            : null,
        userLikelihood:
          typeof rawSeg.userLikelihood === 'number' && Number.isFinite(rawSeg.userLikelihood)
            ? Number(rawSeg.userLikelihood)
            : null,
      };
      if (seg.clusterId && this.clusterRoleBindings.has(seg.clusterId)) {
        const binding = this.clusterRoleBindings.get(seg.clusterId);
        if (seg.userLikelihood !== null && seg.userLikelihood >= 0.80) {
          seg.speakerId = 'NutzerIn';
          this.clusterRoleBindings.set(seg.clusterId, {
            role: 'user',
            speakerId: 'NutzerIn',
            partnerSpeakerId: binding.partnerSpeakerId,
          });
        } else if (seg.userLikelihood !== null && seg.userLikelihood <= 0.10) {
          const partnerSpk = this._resolvePartnerSpeakerIdForCluster(
            seg.clusterId,
            seg.speakerId
          );
          seg.speakerId = partnerSpk;
          this.clusterRoleBindings.set(seg.clusterId, {
            role: 'partner',
            speakerId: partnerSpk,
            partnerSpeakerId: partnerSpk,
          });
        } else {
          seg.speakerId = binding.speakerId;
        }
      }
      if (Number.isFinite(seg.endTime) && seg.endTime > this.lastSessionTimeSec) {
        this.lastSessionTimeSec = seg.endTime;
      }
      const existingIdx = this.diarizedSegments.findIndex(
        (s) => Math.abs(s.startTime - seg.startTime) < 0.08
      );
      if (existingIdx !== -1) {
        this.diarizedSegments[existingIdx] = seg;
      } else {
        this.diarizedSegments.push(seg);
      }

      if (Array.isArray(rawSeg.clusterRelabels) && rawSeg.clusterRelabels.length > 0) {
        for (const relabel of rawSeg.clusterRelabels) {
          if (!relabel || typeof relabel.clusterId !== 'string' || !relabel.newSpeakerId) {
            continue;
          }
          const relabelLikelihood =
            typeof relabel.userLikelihood === 'number' && Number.isFinite(relabel.userLikelihood)
              ? Number(relabel.userLikelihood)
              : null;
          const existingBinding = this.clusterRoleBindings.get(relabel.clusterId);
          let effectiveSpeakerId = relabel.newSpeakerId;
          if (existingBinding) {
            if (relabelLikelihood !== null && relabelLikelihood >= 0.80) {
              effectiveSpeakerId = 'NutzerIn';
              this.clusterRoleBindings.set(relabel.clusterId, {
                role: 'user',
                speakerId: 'NutzerIn',
                partnerSpeakerId: existingBinding.partnerSpeakerId,
              });
            } else if (relabelLikelihood !== null && relabelLikelihood <= 0.10) {
              const partnerSpk = this._resolvePartnerSpeakerIdForCluster(
                relabel.clusterId,
                relabel.newSpeakerId
              );
              effectiveSpeakerId = partnerSpk;
              this.clusterRoleBindings.set(relabel.clusterId, {
                role: 'partner',
                speakerId: partnerSpk,
                partnerSpeakerId: partnerSpk,
              });
            } else {
              // Borderline acoustic relabel does not override an established LLM semantic cluster binding
              effectiveSpeakerId = existingBinding.speakerId;
            }
          }
          for (const existingSeg of this.diarizedSegments) {
            if (existingSeg.clusterId === relabel.clusterId) {
              existingSeg.speakerId = effectiveSpeakerId;
              if (relabelLikelihood !== null) {
                existingSeg.userLikelihood = relabelLikelihood;
              }
            }
          }
          for (const lockedTurn of this.lockedTurns) {
            if (lockedTurn.clusterId === relabel.clusterId) {
              lockedTurn.speakerId = effectiveSpeakerId;
              if (relabelLikelihood !== null) {
                lockedTurn.userLikelihood = relabelLikelihood;
              }
            }
          }
        }
      }
    }

    this.diarizedSegments.sort((a, b) => a.startTime - b.startTime);
  }

  /**
   * Tracks real-time VAD speech onset (`false -> true`) and offset (`true -> false`) transitions.
   * Must be called immediately after `ingestDiarizedSegments` for each processed audio chunk so
   * discarded `< 400 ms` noise blips are cleared cleanly once VAD returns to silence.
   *
   * @param {boolean} isSpeaking
   * @param {number} audioTimeSec
   */
  notifyVadState(isSpeaking, audioTimeSec) {
    if (typeof audioTimeSec === 'number' && Number.isFinite(audioTimeSec)) {
      this.lastSessionTimeSec = Math.max(this.lastSessionTimeSec, audioTimeSec);
    }
    const speaking = Boolean(isSpeaking);
    const latestSeg =
      this.diarizedSegments.length > 0
        ? this.diarizedSegments[this.diarizedSegments.length - 1]
        : null;

    if (speaking && !this.isVadSpeaking) {
      const floorSec = latestSeg ? latestSeg.endTime + 0.02 : 0;
      const onsetSec = Math.max(floorSec, audioTimeSec - VAD_ONSET_LOOKBACK_SEC);
      this.isVadSpeaking = true;
      this.activeVadStartSec = onsetSec;
      this.lastVadStartSec = onsetSec;
      this.lastVoicedVadSec = audioTimeSec;
    } else if (speaking && this.isVadSpeaking) {
      this.lastVoicedVadSec = audioTimeSec;
    } else if (!speaking && this.isVadSpeaking) {
      this.isVadSpeaking = false;
      this.activeVadStartSec = null;
      // If VAD returned to silence without emitting a segment covering `lastVadStartSec`,
      // the sound was shorter than `min_speech` (400 ms) and was discarded by VAD.
      if (
        this.lastVadStartSec !== null &&
        (!latestSeg || this.lastVadStartSec > latestSeg.endTime + 0.05)
      ) {
        this.lastVadStartSec = latestSeg ? latestSeg.startTime : null;
      }
    }
  }

  /**
   * Computes an audio-aligned session timestamp (in seconds) for a newly recognized word arriving
   * at `rawSec`, accounting for STT latency, new VAD turn onsets, and delayed silence finalizations.
   *
   * @param {number} rawSec
   * @param {boolean} isFirstWordAfterVadOnset
   * @returns {number}
   */
  anchorWordTimestamp(rawSec, isFirstWordAfterVadOnset = false) {
    let t = Math.max(0, rawSec - STT_LATENCY_OFFSET_SEC);
    const latestSeg =
      this.diarizedSegments.length > 0
        ? this.diarizedSegments[this.diarizedSegments.length - 1]
        : null;
    const latestEnd = latestSeg ? latestSeg.endTime : -1;
    const onset = this.activeVadStartSec ?? this.lastVadStartSec;
    const isNewTurnOnset = onset !== null && onset > latestEnd + 0.01;

    if (
      isFirstWordAfterVadOnset &&
      isNewTurnOnset &&
      rawSec >= onset + MIN_STT_TURN_REACTION_SEC
    ) {
      return Math.max(t, onset + 0.02);
    }

    if (
      !this.isVadSpeaking &&
      !isNewTurnOnset &&
      latestEnd >= 0 &&
      t > latestEnd &&
      rawSec - latestEnd <= 5.0
    ) {
      return Math.min(t, latestEnd);
    }

    return t;
  }

  /**
   * Ensures an epoch state record exists for `epoch` and returns it.
   *
   * @param {number} epoch
   */
  _getOrCreateEpochState(epoch) {
    let state = this.epochStates.get(epoch);
    if (!state) {
      state = {
        finalizedByIndex: new Map(),
        promotedUtterances: [],
        interimWords: [],
      };
      this.epochStates.set(epoch, state);
    }
    return state;
  }

  /**
   * Processes a `SpeechRecognitionEvent` (`event.results`) for `epoch` at session clock `nowSec`.
   *
   * Reconciles all unfinalized + newly-finalized result slots against the epoch's unified
   * `interimWords` buffer so Chrome's multi-slot interim splitting and `results.length` shrinking
   * never leave duplicate words or leak timestamps into the next speaker's turn.
   *
   * @param {number} epoch
   * @param {SpeechRecognitionResultList | Array<{isFinal: boolean, 0: {transcript: string}}>} results
   * @param {number} nowSec
   */
  handleRecognitionResults(epoch, results, nowSec) {
    if (!results) {
      return;
    }
    if (typeof nowSec === 'number' && Number.isFinite(nowSec)) {
      this.lastSessionTimeSec = Math.max(this.lastSessionTimeSec, nowSec);
    }
    const state = this._getOrCreateEpochState(epoch);
    const prevInterim = state.interimWords;

    /** @type {Array<{text: string, norm: string, clean: string, slotIndex: number, isFinal: boolean}>} */
    const currActive = [];
    /** @type {Array<{slotIndex: number, isFinal: boolean, start: number, count: number}>} */
    const activeSlots = [];

    for (let i = 0; i < results.length; i++) {
      if (state.finalizedByIndex.has(i)) {
        continue;
      }
      const res = results[i];
      if (!res) {
        continue;
      }
      const isFinal = Boolean(res.isFinal);
      const rawText = res[0] && typeof res[0].transcript === 'string' ? res[0].transcript : '';
      const tokens = tokenizeTranscript(rawText);

      if (tokens.length === 0) {
        if (isFinal) {
          state.finalizedByIndex.set(i, { words: [], isFinal: true });
        }
        continue;
      }

      const start = currActive.length;
      for (const text of tokens) {
        const norm = normalizeToken(text);
        currActive.push({
          text,
          norm,
          clean: canonicalizeForMatch(norm),
          slotIndex: i,
          isFinal,
        });
      }
      activeSlots.push({ slotIndex: i, isFinal, start, count: tokens.length });
    }

    if (currActive.length === 0) {
      state.interimWords = [];
      return;
    }

    const assignedTimes = this._assignAlignedTimestamps(prevInterim, currActive, nowSec);

    const nextInterim = [];
    for (const slot of activeSlots) {
      const slotWords = [];
      for (let k = 0; k < slot.count; k++) {
        const idx = slot.start + k;
        const item = currActive[idx];
        slotWords.push({
          text: item.text,
          norm: item.norm,
          clean: item.clean,
          timeSec: assignedTimes[idx],
          slotIndex: slot.slotIndex,
          lockedTurnId: null,
        });
      }
      if (slot.isFinal) {
        state.finalizedByIndex.set(slot.slotIndex, {
          words: slotWords,
          isFinal: true,
        });
      } else {
        for (const w of slotWords) {
          nextInterim.push(w);
        }
      }
    }

    state.interimWords = nextInterim;
  }

  /**
   * Assigns monotonic, turn-aware timestamps to `currActive` tokens by aligning them against `prevInterim`.
   *
   * @param {Array<{text: string, norm: string, clean: string, timeSec: number, slotIndex: number}>} prevInterim
   * @param {Array<{text: string, norm: string, clean: string, slotIndex: number, isFinal: boolean}>} currActive
   * @param {number} nowSec
   * @returns {Float64Array}
   */
  _assignAlignedTimestamps(prevInterim, currActive, nowSec) {
    const m = prevInterim.length;
    const n = currActive.length;
    const assigned = new Float64Array(n);
    const onset = this.activeVadStartSec ?? this.lastVadStartSec;

    if (m === 0) {
      const firstTime = this.anchorWordTimestamp(nowSec, true);
      const restTime = Math.max(firstTime, this.anchorWordTimestamp(nowSec, false));
      assigned[0] = firstTime;
      for (let c = 1; c < n; c++) {
        assigned[c] = restTime;
      }
      return assigned;
    }

    const matchedPrevIdx = matchTokenSequences(prevInterim, currActive);
    let firstMatchedC = -1;
    let lastMatchedC = -1;
    let totalMatches = 0;

    for (let c = 0; c < n; c++) {
      if (matchedPrevIdx[c] !== -1) {
        assigned[c] = prevInterim[matchedPrevIdx[c]].timeSec;
        if (firstMatchedC === -1) {
          firstMatchedC = c;
        }
        lastMatchedC = c;
        totalMatches += 1;
      }
    }

    // No tokens matched between `prevInterim` and `currActive`
    if (totalMatches === 0) {
      const prevLastTime = prevInterim[m - 1].timeSec;
      const crossedVadOnset = onset !== null && onset > prevLastTime + 0.05;
      const isDifferentSlotStale =
        currActive[0].slotIndex !== prevInterim[0].slotIndex &&
        nowSec - prevLastTime > 0.85;
      const isStale = crossedVadOnset || isDifferentSlotStale;

      if (isStale) {
        const firstTime = this.anchorWordTimestamp(nowSec, true);
        const restTime = Math.max(firstTime, this.anchorWordTimestamp(nowSec, false));
        assigned[0] = firstTime;
        for (let c = 1; c < n; c++) {
          assigned[c] = restTime;
        }
        return assigned;
      }

      for (let c = 0; c < n; c++) {
        if (c < m && currActive[c].slotIndex === prevInterim[c].slotIndex) {
          assigned[c] = prevInterim[c].timeSec;
        } else {
          const prevT = c > 0 ? assigned[c - 1] : -1;
          const isAfterOnset = c === 0 || (onset !== null && prevT < onset - 0.02);
          assigned[c] = Math.max(
            c > 0 ? assigned[c - 1] : 0,
            this.anchorWordTimestamp(nowSec, isAfterOnset)
          );
        }
      }
      return assigned;
    }

    // 1. Leading unmatched tokens before `firstMatchedC`
    const pFirst = matchedPrevIdx[firstMatchedC];
    for (let c = firstMatchedC - 1; c >= 0; c--) {
      const candP = pFirst - (firstMatchedC - c);
      if (
        candP >= 0 &&
        !(
          onset !== null &&
          prevInterim[candP].timeSec < onset - 0.02 &&
          assigned[firstMatchedC] >= onset - 0.02
        )
      ) {
        assigned[c] = Math.min(assigned[c + 1], prevInterim[candP].timeSec);
      } else {
        assigned[c] = assigned[c + 1];
      }
    }

    // 2. Interior unmatched tokens between consecutive matched anchors `(cL, cR)`
    let cL = firstMatchedC;
    while (cL < lastMatchedC) {
      let cR = cL + 1;
      while (cR <= lastMatchedC && matchedPrevIdx[cR] === -1) {
        cR += 1;
      }
      if (cR > cL + 1) {
        const pL = matchedPrevIdx[cL];
        const pR = matchedPrevIdx[cR];
        const isOneToOneReplacement = pR - pL === cR - cL;

        for (let c = cL + 1; c < cR; c++) {
          if (isOneToOneReplacement) {
            const candT = prevInterim[pL + (c - cL)].timeSec;
            assigned[c] = Math.max(assigned[cL], Math.min(assigned[cR], candT));
          } else if (
            currActive[c].slotIndex === currActive[cL].slotIndex &&
            currActive[c].slotIndex !== currActive[cR].slotIndex
          ) {
            assigned[c] = assigned[cL];
          } else if (
            currActive[c].slotIndex === currActive[cR].slotIndex &&
            currActive[c].slotIndex !== currActive[cL].slotIndex
          ) {
            assigned[c] = assigned[cR];
          } else {
            const frac = (c - cL) / (cR - cL);
            assigned[c] = assigned[cL] + frac * (assigned[cR] - assigned[cL]);
          }
        }
      }
      cL = cR;
    }

    // 3. Trailing unmatched tokens after `lastMatchedC`
    const pLast = matchedPrevIdx[lastMatchedC];
    const maxSlotIndex = currActive[n - 1].slotIndex;

    for (let c = lastMatchedC + 1; c < n; c++) {
      const candP = pLast + (c - lastMatchedC);
      const prevAssignedT = assigned[c - 1];

      if (
        candP < m &&
        currActive[c].slotIndex === prevInterim[candP].slotIndex &&
        !(onset !== null && onset > prevInterim[candP].timeSec + 0.05)
      ) {
        assigned[c] = Math.max(prevAssignedT, prevInterim[candP].timeSec);
      } else if (
        currActive[c].slotIndex === currActive[c - 1].slotIndex &&
        currActive[c].slotIndex < maxSlotIndex
      ) {
        // Expanded tail token inside an earlier slot while a newer slot already exists
        assigned[c] = prevAssignedT;
      } else {
        const isAfterOnset = onset !== null && prevAssignedT < onset - 0.02;
        assigned[c] = Math.max(
          prevAssignedT,
          this.anchorWordTimestamp(nowSec, isAfterOnset)
        );
      }
    }

    return assigned;
  }

  /**
   * Promotes any remaining unfinalized interim words of `epoch` into a finalized utterance when
   * `SpeechRecognition.onend` fires (e.g. automatic restart or recording stop).
   *
   * @param {number} epoch
   */
  finalizeEpoch(epoch) {
    const state = this.epochStates.get(epoch);
    if (!state || state.interimWords.length === 0) {
      return;
    }
    state.promotedUtterances.push({
      words: state.interimWords,
      isFinal: true,
    });
    state.interimWords = [];
  }

  /**
   * Promotes any remaining unfinalized interim words across all epochs into finalized utterances.
   * Called synchronously when stopping recording so trailing speech is finalized even before async
   * `SpeechRecognition.onend` fires.
   */
  finalizeAllEpochs() {
    for (const epoch of this.epochStates.keys()) {
      this.finalizeEpoch(epoch);
    }
  }

  /**
   * Returns `true` if there is at least one unlocked transcript word waiting for silence thresholds
   * (`1.0s` `/correct-transcript` or `2.0s` `/translate` + turn lock) or speaker identification.
   *
   * @returns {boolean}
   */
  hasUnlockedTurns() {
    for (const state of this.epochStates.values()) {
      if (state.interimWords.length > 0) {
        return true;
      }
      for (const utt of state.finalizedByIndex.values()) {
        for (const w of utt.words) {
          if (w.lockedTurnId === null) {
            return true;
          }
        }
      }
      for (const promoted of state.promotedUtterances) {
        for (const w of promoted.words) {
          if (w.lockedTurnId === null) {
            return true;
          }
        }
      }
    }
    return false;
  }

  /**
   * Returns all utterances across epochs in chronological order for rendering.
   *
   * @returns {Array<{words: Array<{text: string, timeSec: number, lockedTurnId: number | null}>, isFinal: boolean}>}
   */
  getOrderedUtterances() {
    /** @type {Array<{words: Array<{text: string, timeSec: number, lockedTurnId: number | null}>, isFinal: boolean}>} */
    const list = [];
    for (const state of this.epochStates.values()) {
      if (state.finalizedByIndex.size > 0) {
        const sortedIndices = Array.from(state.finalizedByIndex.keys()).sort((a, b) => a - b);
        for (const idx of sortedIndices) {
          const utt = state.finalizedByIndex.get(idx);
          if (utt && utt.words.length > 0) {
            list.push(utt);
          }
        }
      }
      for (const promoted of state.promotedUtterances) {
        if (promoted.words.length > 0) {
          list.push(promoted);
        }
      }
      if (state.interimWords.length > 0) {
        list.push({
          words: state.interimWords,
          isFinal: false,
        });
      }
    }
    return list;
  }

  /**
   * Checks whether `timeSec` falls strictly within `[seg.startTime, seg.endTime]` of any diarized segment.
   *
   * @param {number} timeSec
   * @returns {boolean}
   */
  isStrictlyInsideAnySegment(timeSec) {
    for (const seg of this.diarizedSegments) {
      if (timeSec >= seg.startTime && timeSec <= seg.endTime) {
        return true;
      }
    }
    return false;
  }

  /**
   * Resolves the speaker ID (`"NutzerIn"`, `"Speaker N"`, or `null` while pending identification)
   * for a word stamped at `timeSec`.
   *
   * Returns `null` (`"Sprecher wird erkannt…"`) only when no segment exists yet or when Silero VAD
   * has actively detected a new un-emitted speech turn after `latestSeg.endTime`. Otherwise prioritizes
   * strict interval containment `[seg.startTime, seg.endTime]`, active unbroken turn continuation,
   * and minimum boundary/edge distance.
   *
   * @param {number} timeSec
   * @returns {string | null}
   */
  resolveSpeakerForTime(timeSec) {
    if (this.diarizedSegments.length === 0) {
      return null;
    }

    const latestSeg = this.diarizedSegments[this.diarizedSegments.length - 1];
    const onset = this.activeVadStartSec ?? this.lastVadStartSec;
    const hasUnemittedVadTurn = onset !== null && onset > latestSeg.endTime + 0.05;

    if (this.isRecording && hasUnemittedVadTurn) {
      const silenceMidpoint = 0.5 * (latestSeg.endTime + onset);
      if (timeSec >= silenceMidpoint) {
        return null;
      }
    }

    // 1. Strict containment check (preferring later refined chunks on overlap)
    for (let i = this.diarizedSegments.length - 1; i >= 0; i--) {
      const seg = this.diarizedSegments[i];
      if (timeSec >= seg.startTime && timeSec <= seg.endTime) {
        return seg.speakerId;
      }
    }

    // 2. Continuing an already-checkpointed unbroken VAD turn before the next window arrives
    if (
      this.isRecording &&
      this.isVadSpeaking &&
      !hasUnemittedVadTurn &&
      timeSec > latestSeg.endTime
    ) {
      return latestSeg.speakerId;
    }

    // 3. Nearest segment boundary (edge distance, breaking ties toward the newer segment)
    let nearestSeg = null;
    let nearestDist = Infinity;
    for (const seg of this.diarizedSegments) {
      const dist =
        timeSec < seg.startTime ? seg.startTime - timeSec : timeSec - seg.endTime;
      if (dist <= nearestDist) {
        nearestDist = dist;
        nearestSeg = seg;
      }
    }

    return nearestSeg ? nearestSeg.speakerId : latestSeg.speakerId;
  }

  /**
   * Groups all currently unlocked (`word.lockedTurnId === null`) words across utterances into
   * contiguous speaker card groups while preserving word object references and timestamp bounds.
   *
   * @returns {Array<{
   *   speakerId: string | null,
   *   parts: Array<{text: string, isInterim: boolean}>,
   *   wordRefs: Array<{text: string, timeSec: number, lockedTurnId: number | null}>,
   *   rawText: string,
   *   hasInterim: boolean,
   *   minTimeSec: number,
   *   maxTimeSec: number
   * }>}
   */
  _buildUnlockedCardGroups() {
    const unlockedCards = [];
    const utterances = this.getOrderedUtterances();

    for (const utt of utterances) {
      if (!utt.words || utt.words.length === 0) {
        continue;
      }

      const speakers = utt.words.map((w) => this.resolveSpeakerForTime(w.timeSec));

      // If the opening word of an STT utterance landed in a silence gap (not strictly inside any
      // diarized segment) and the second word within 0.85s belongs to a resolved speaker, snap
      // the opening word to that speaker so quick replies never cling to the prior speaker's tail.
      if (
        utt.words.length >= 2 &&
        speakers[1] !== null &&
        speakers[0] !== speakers[1] &&
        !this.isStrictlyInsideAnySegment(utt.words[0].timeSec) &&
        utt.words[1].timeSec - utt.words[0].timeSec <= 0.85
      ) {
        speakers[0] = speakers[1];
      }

      let currentRunSpeaker = undefined;
      let currentRunWordObjs = [];

      const flushRun = () => {
        if (currentRunWordObjs.length === 0) {
          return;
        }
        const text = currentRunWordObjs.map((w) => w.text).join(' ');
        const isInterim = !utt.isFinal;
        const minT = currentRunWordObjs[0].timeSec;
        const maxT = currentRunWordObjs[currentRunWordObjs.length - 1].timeSec;
        const lastCard =
          unlockedCards.length > 0 ? unlockedCards[unlockedCards.length - 1] : null;

        if (lastCard && lastCard.speakerId === currentRunSpeaker) {
          lastCard.parts.push({ text, isInterim });
          for (const w of currentRunWordObjs) {
            lastCard.wordRefs.push(w);
          }
          lastCard.hasInterim = lastCard.hasInterim || isInterim;
          lastCard.minTimeSec = Math.min(lastCard.minTimeSec, minT);
          lastCard.maxTimeSec = Math.max(lastCard.maxTimeSec, maxT);
        } else {
          unlockedCards.push({
            speakerId: currentRunSpeaker,
            parts: [{ text, isInterim }],
            wordRefs: currentRunWordObjs.slice(),
            rawText: '',
            hasInterim: isInterim,
            minTimeSec: minT,
            maxTimeSec: maxT,
          });
        }
        currentRunWordObjs = [];
      };

      for (let k = 0; k < utt.words.length; k++) {
        const wordObj = utt.words[k];
        if (wordObj.lockedTurnId !== null) {
          continue;
        }
        const spk = speakers[k];
        if (currentRunSpeaker === undefined) {
          currentRunSpeaker = spk;
        } else if (spk !== currentRunSpeaker) {
          flushRun();
          currentRunSpeaker = spk;
        }
        currentRunWordObjs.push(wordObj);
      }
      flushRun();
    }

    for (const card of unlockedCards) {
      card.rawText = card.parts
        .map((p) => p.text)
        .join(' ')
        .trim();
    }

    return unlockedCards;
  }

  /**
   * Inspects diarized segments associated with the leading unlocked card `card0` to determine:
   * - whether its speaker attribution is finalized (no active `isCheckpoint: true` segment),
   * - the acoustic start and end timestamps (`speechStartSec`, `speechEndSec`) of that turn's voiced speech, and
   * - whether a confirmed different speaker has already spoken after `card0`.
   *
   * @param {{speakerId: string, minTimeSec: number, maxTimeSec: number}} card0
   * @param {Array<{speakerId: string | null}>} unlockedCards
   * @returns {{isSpeakerFinalized: boolean, speechStartSec: number, speechEndSec: number, hasConfirmedSpeakerSwitch: boolean}}
   */
  _inspectLeadingCardLifecycle(card0, unlockedCards) {
    const prevLocked =
      this.lockedTurns.length > 0 ? this.lockedTurns[this.lockedTurns.length - 1] : null;
    const floorEndSec = prevLocked ? prevLocked.speechEndSec : -Infinity;

    const matchingSegs = [];
    let hasSubsequentOtherSpeakerSeg = false;

    for (const seg of this.diarizedSegments) {
      if (seg.endTime <= floorEndSec + 0.02) {
        continue;
      }
      if (seg.speakerId === card0.speakerId) {
        matchingSegs.push(seg);
      } else if (
        seg.endTime > card0.maxTimeSec + 0.05 &&
        (matchingSegs.length > 0 || seg.startTime >= card0.maxTimeSec - 0.05)
      ) {
        hasSubsequentOtherSpeakerSeg = true;
        break;
      }
    }

    if (matchingSegs.length === 0) {
      for (const seg of this.diarizedSegments) {
        if (
          seg.speakerId === card0.speakerId &&
          seg.endTime >= card0.minTimeSec - 0.5 &&
          seg.startTime <= card0.maxTimeSec + 0.5
        ) {
          matchingSegs.push(seg);
        }
      }
    }

    const isSpeakerFinalized =
      matchingSegs.length > 0 && matchingSegs.every((s) => !s.isCheckpoint);
    const minSegStartSec =
      matchingSegs.length > 0 ? matchingSegs[0].startTime : card0.minTimeSec;
    const maxSegEndSec =
      matchingSegs.length > 0
        ? matchingSegs[matchingSegs.length - 1].endTime
        : card0.maxTimeSec;
    const speechStartSec = Math.min(card0.minTimeSec, minSegStartSec);
    const speechEndSec = Math.max(card0.maxTimeSec, maxSegEndSec);
    const hasNextResolvedOtherCard = unlockedCards
      .slice(1)
      .some((c) => c.speakerId !== null && c.speakerId !== card0.speakerId);

    return {
      isSpeakerFinalized,
      speechStartSec,
      speechEndSec,
      hasConfirmedSpeakerSwitch: hasSubsequentOtherSpeakerSeg || hasNextResolvedOtherCard,
    };
  }

  /**
   * Computes the acoustic `[speechStartSec, speechEndSec]` bounds for a span `[minTimeSec, maxTimeSec]`
   * belonging to `speakerId` after `floorEndSec`.
   *
   * @param {string | null} speakerId
   * @param {number} minTimeSec
   * @param {number} maxTimeSec
   * @param {number} [floorEndSec]
   * @returns {{speechStartSec: number, speechEndSec: number}}
   */
  getSpeechBoundsForSpan(speakerId, minTimeSec, maxTimeSec, floorEndSec = -Infinity) {
    if (!speakerId || this.diarizedSegments.length === 0) {
      return { speechStartSec: minTimeSec, speechEndSec: maxTimeSec };
    }
    const matching = [];
    for (const seg of this.diarizedSegments) {
      if (seg.endTime <= floorEndSec + 0.02) {
        continue;
      }
      if (
        seg.speakerId === speakerId &&
        seg.endTime >= minTimeSec - 0.5 &&
        seg.startTime <= maxTimeSec + 0.5
      ) {
        matching.push(seg);
      }
    }
    if (matching.length === 0) {
      return { speechStartSec: minTimeSec, speechEndSec: maxTimeSec };
    }
    return {
      speechStartSec: Math.min(minTimeSec, matching[0].startTime),
      speechEndSec: Math.max(maxTimeSec, matching[matching.length - 1].endTime),
    };
  }

  /**
   * Computes the duration-weighted user likelihood in `[0.0, 1.0]` across diarized segments
   * matching `speakerId` that cover or overlap `[minTimeSec, maxTimeSec]`. Returns `null` if
   * no segments with calibrated user likelihood are present.
   *
   * @param {string | null} speakerId
   * @param {number} minTimeSec
   * @param {number} maxTimeSec
   * @returns {number | null}
   */
  getUserLikelihoodForSpan(speakerId, minTimeSec, maxTimeSec) {
    if (!speakerId || this.diarizedSegments.length === 0) {
      return null;
    }
    const matching = [];
    for (const seg of this.diarizedSegments) {
      if (
        seg.speakerId === speakerId &&
        typeof seg.userLikelihood === 'number' &&
        seg.endTime >= minTimeSec - 0.5 &&
        seg.startTime <= maxTimeSec + 0.5
      ) {
        matching.push(seg);
      }
    }
    if (matching.length === 0) {
      return null;
    }
    let totalWeight = 0;
    let weightedSum = 0;
    for (const seg of matching) {
      const dur = Math.max(0.1, seg.endTime - seg.startTime);
      totalWeight += dur;
      weightedSum += dur * seg.userLikelihood;
    }
    return totalWeight > 0 ? weightedSum / totalWeight : null;
  }

  /**
   * Resolves the dominant `clusterId` across diarized segments matching `speakerId` that cover
   * or overlap `[minTimeSec, maxTimeSec]`.
   *
   * @param {string | null} speakerId
   * @param {number} minTimeSec
   * @param {number} maxTimeSec
   * @returns {string | null}
   */
  getClusterIdForSpan(speakerId, minTimeSec, maxTimeSec) {
    if (!speakerId || this.diarizedSegments.length === 0) {
      return null;
    }
    for (let i = this.diarizedSegments.length - 1; i >= 0; i--) {
      const seg = this.diarizedSegments[i];
      if (
        seg.speakerId === speakerId &&
        seg.clusterId &&
        seg.endTime >= minTimeSec - 0.5 &&
        seg.startTime <= maxTimeSec + 0.5
      ) {
        return seg.clusterId;
      }
    }
    return null;
  }

  /**
   * Permanently locks a resolved unlocked card into `this.lockedTurns` and stamps its words.
   *
   * @param {{
   *   speakerId: string,
   *   parts: Array<{text: string, isInterim: boolean}>,
   *   wordRefs: Array<{lockedTurnId: number | null}>,
   *   rawText: string,
   *   minTimeSec: number,
   *   maxTimeSec: number
   * }} card
   * @param {number} speechEndSec
   * @param {number} [speechStartSec]
   */
  _commitLockedCard(card, speechEndSec, speechStartSec) {
    const turnId = this.nextTurnId;
    this.nextTurnId += 1;
    for (const w of card.wordRefs) {
      w.lockedTurnId = turnId;
    }
    const prevLocked =
      this.lockedTurns.length > 0 ? this.lockedTurns[this.lockedTurns.length - 1] : null;
    const effectiveStartSec =
      typeof speechStartSec === 'number' && Number.isFinite(speechStartSec)
        ? speechStartSec
        : card.minTimeSec;
    const pauseMs = prevLocked
      ? Math.max(0, Math.round((effectiveStartSec - prevLocked.speechEndSec) * 1000))
      : null;
    const userLikelihood = this.getUserLikelihoodForSpan(
      card.speakerId,
      card.minTimeSec,
      card.maxTimeSec
    );
    const clusterId = this.getClusterIdForSpan(
      card.speakerId,
      card.minTimeSec,
      card.maxTimeSec
    );
    this.lockedTurns.push({
      turnId,
      clusterId,
      speakerId: card.speakerId,
      parts: card.parts.map((p) => ({ text: p.text, isInterim: false })),
      rawText: card.rawText,
      hasInterim: false,
      shouldCorrect: true,
      locked: true,
      speechStartSec: effectiveStartSec,
      speechEndSec,
      pauseMs,
      userLikelihood,
    });
  }

  /**
   * Applies an LLM-verified speaker role (`"user"` or `"partner"`) from `/correct-transcript` to `turnId`.
   *
   * - Enforces the acoustic lock guardrail (`userLikelihood >= 0.80` -> `'user'`, `<= 0.10` -> `'partner'`).
   * - Updates `speakerId` (`'NutzerIn'` or `'Speaker N'`) on the target turn.
   * - When the turn's `clusterId` is known and its acoustic score lies in the uncertain zone
   *   (`0.10 < userLikelihood < 0.80` or `null`), records a session binding in `this.clusterRoleBindings`
   *   and propagates that binding to existing and future uncertain segments of the same `clusterId`.
   *
   * @param {number} turnId
   * @param {'user' | 'partner'} resolvedRole
   * @returns {string | null} The effective `speakerId` (`'NutzerIn'` or `'Speaker N'`), or `null` if `turnId` was not found.
   */
  applyTurnRoleResolution(turnId, resolvedRole) {
    const turn = this.lockedTurns.find((t) => t.turnId === turnId);
    if (!turn) {
      return null;
    }
    let effectiveRole = resolvedRole === 'user' ? 'user' : 'partner';
    const p = typeof turn.userLikelihood === 'number' ? turn.userLikelihood : null;
    if (p !== null && p >= 0.80) {
      effectiveRole = 'user';
    } else if (p !== null && p <= 0.10) {
      effectiveRole = 'partner';
    }

    const partnerSpk = this._resolvePartnerSpeakerIdForCluster(turn.clusterId, turn.speakerId);
    const newSpeakerId = effectiveRole === 'user' ? 'NutzerIn' : partnerSpk;
    turn.speakerId = newSpeakerId;

    const isUncertainZone = p === null || (p > 0.10 && p < 0.80);
    if (turn.clusterId && isUncertainZone) {
      this.clusterRoleBindings.set(turn.clusterId, {
        role: effectiveRole,
        speakerId: newSpeakerId,
        partnerSpeakerId: partnerSpk,
      });

      for (const seg of this.diarizedSegments) {
        if (seg.clusterId !== turn.clusterId) {
          continue;
        }
        const segP = typeof seg.userLikelihood === 'number' ? seg.userLikelihood : null;
        if (segP !== null && (segP >= 0.80 || segP <= 0.10)) {
          continue;
        }
        seg.speakerId = newSpeakerId;
      }

      for (const otherTurn of this.lockedTurns) {
        if (otherTurn.clusterId !== turn.clusterId) {
          continue;
        }
        const otherP =
          typeof otherTurn.userLikelihood === 'number' ? otherTurn.userLikelihood : null;
        if (otherP !== null && (otherP >= 0.80 || otherP <= 0.10)) {
          continue;
        }
        otherTurn.speakerId = newSpeakerId;
      }
    }

    return newSpeakerId;
  }

  /**
   * Merges a locked turn (`turnId`) into the immediately preceding locked turn when `/correct-transcript`
   * returns `mergeWithPrevious: true`, re-stamping all underlying word `lockedTurnId`s so subsequent
   * `syncTurns()` calls keep the unified turn intact.
   *
   * @param {number} turnId
   * @param {'user' | 'partner'} [resolvedRole]
   * @returns {{
   *   targetTurnId: number,
   *   speakerId: string,
   *   role: 'user' | 'partner',
   *   rawText: string
   * } | null}
   */
  mergeLockedTurnIntoPrevious(turnId, resolvedRole) {
    const idx = this.lockedTurns.findIndex((t) => t.turnId === turnId);
    if (idx <= 0) {
      return null;
    }
    const prevTurn = this.lockedTurns[idx - 1];
    const currTurn = this.lockedTurns[idx];

    const prevP = typeof prevTurn.userLikelihood === 'number' ? prevTurn.userLikelihood : null;
    const currP = typeof currTurn.userLikelihood === 'number' ? currTurn.userLikelihood : null;

    // Never merge two turns with contradictory acoustic locks (>= 0.80 vs <= 0.10)
    if (
      prevP !== null &&
      currP !== null &&
      ((prevP >= 0.80 && currP <= 0.10) || (prevP <= 0.10 && currP >= 0.80))
    ) {
      return null;
    }

    let effectiveRole =
      resolvedRole === 'user' || resolvedRole === 'partner'
        ? resolvedRole
        : speakerIdToApiRole(prevTurn.speakerId);
    if ((prevP !== null && prevP >= 0.80) || (currP !== null && currP >= 0.80)) {
      effectiveRole = 'user';
    } else if ((prevP !== null && prevP <= 0.10) || (currP !== null && currP <= 0.10)) {
      effectiveRole = 'partner';
    }

    // Re-stamp all word objects belonging to `currTurn.turnId` so they belong to `prevTurn.turnId`
    for (const state of this.epochStates.values()) {
      for (const utt of state.finalizedByIndex.values()) {
        for (const w of utt.words) {
          if (w.lockedTurnId === currTurn.turnId) {
            w.lockedTurnId = prevTurn.turnId;
          }
        }
      }
      for (const promoted of state.promotedUtterances) {
        for (const w of promoted.words) {
          if (w.lockedTurnId === currTurn.turnId) {
            w.lockedTurnId = prevTurn.turnId;
          }
        }
      }
      for (const w of state.interimWords) {
        if (w.lockedTurnId === currTurn.turnId) {
          w.lockedTurnId = prevTurn.turnId;
        }
      }
    }

    prevTurn.parts.push(...currTurn.parts);
    prevTurn.rawText = `${prevTurn.rawText} ${currTurn.rawText}`.trim();
    prevTurn.speechEndSec = Math.max(prevTurn.speechEndSec, currTurn.speechEndSec);

    // Remove the split turn before applying role resolution
    this.lockedTurns.splice(idx, 1);

    const unifiedSpeakerId =
      this.applyTurnRoleResolution(prevTurn.turnId, effectiveRole) || prevTurn.speakerId;

    // If `currTurn` had a distinct uncertain clusterId, bind it to the unified speaker as well
    if (
      currTurn.clusterId &&
      currTurn.clusterId !== prevTurn.clusterId &&
      (currP === null || (currP > 0.10 && currP < 0.80))
    ) {
      const partnerSpk = this._resolvePartnerSpeakerIdForCluster(
        currTurn.clusterId,
        unifiedSpeakerId
      );
      this.clusterRoleBindings.set(currTurn.clusterId, {
        role: effectiveRole,
        speakerId: unifiedSpeakerId,
        partnerSpeakerId: partnerSpk,
      });
      for (const seg of this.diarizedSegments) {
        if (seg.clusterId === currTurn.clusterId) {
          const segP = typeof seg.userLikelihood === 'number' ? seg.userLikelihood : null;
          if (segP === null || (segP > 0.10 && segP < 0.80)) {
            seg.speakerId = unifiedSpeakerId;
          }
        }
      }
    }

    return {
      targetTurnId: prevTurn.turnId,
      speakerId: unifiedSpeakerId,
      role: speakerIdToApiRole(unifiedSpeakerId),
      rawText: prevTurn.rawText,
    };
  }

  /**
   * Immediately finalizes all epochs and locks every remaining unlocked turn that has a resolved
   * `speakerId !== null`. Called when the user stops recording (`Stopp`) so `/correct-transcript`
   * and `/translate` run immediately without waiting for `1.0s` / `2.0s` silence timers.
   *
   * @returns {Array<{
   *   turnId: number,
   *   speakerId: string | null,
   *   parts: Array<{text: string, isInterim: boolean}>,
   *   rawText: string,
   *   hasInterim: boolean,
   *   shouldCorrect: boolean,
   *   locked: boolean
   * }>}
   */
  lockAllResolvedTurns() {
    this.finalizeAllEpochs();
    let unlockedCards = this._buildUnlockedCardGroups();
    while (
      unlockedCards.length > 0 &&
      unlockedCards[0].speakerId !== null &&
      !unlockedCards[0].hasInterim &&
      unlockedCards[0].rawText.length > 0
    ) {
      const card0 = unlockedCards[0];
      const { speechStartSec, speechEndSec } = this._inspectLeadingCardLifecycle(
        card0,
        unlockedCards
      );
      this._commitLockedCard(card0, speechEndSec, speechStartSec);
      unlockedCards = this._buildUnlockedCardGroups();
    }
    return this.syncTurns(this.lastSessionTimeSec);
  }

  /**
   * Evaluates the `1.0s` (`/correct-transcript`) and `2.0s` (`/translate` + turn lock) silence
   * thresholds as well as speaker-switch boundaries at `nowSec`, returning all locked and unlocked
   * conversation turns in chronological order with stable `turnId`s.
   *
   * - Pauses `< 2.0s` by the same speaker stay merged in the same unlocked turn (re-triggering
   *   `/correct-transcript` once `1.0s` of silence elapses after the combined speech).
   * - At `>= 2.0s` silence (or immediately upon a confirmed speaker switch), the turn locks so any
   *   subsequent speech opens a new turn card below.
   *
   * @param {number} [nowSec]
   * @returns {Array<{
   *   turnId: number,
   *   clusterId: string | null,
   *   speakerId: string | null,
   *   parts: Array<{text: string, isInterim: boolean}>,
   *   rawText: string,
   *   hasInterim: boolean,
   *   shouldCorrect: boolean,
   *   locked: boolean,
   *   userLikelihood: number | null,
   *   pauseMs: number | null
   * }>}
   */
  syncTurns(nowSec) {
    if (typeof nowSec === 'number' && Number.isFinite(nowSec)) {
      this.lastSessionTimeSec = Math.max(this.lastSessionTimeSec, nowSec);
    }
    const effectiveNowSec = this.lastSessionTimeSec;

    let unlockedCards = this._buildUnlockedCardGroups();
    let leadingShouldCorrect = false;

    while (unlockedCards.length > 0) {
      const card0 = unlockedCards[0];
      if (card0.speakerId === null || card0.hasInterim || card0.rawText.length === 0) {
        leadingShouldCorrect = false;
        break;
      }

      const { isSpeakerFinalized, speechStartSec, speechEndSec, hasConfirmedSpeakerSwitch } =
        this._inspectLeadingCardLifecycle(card0, unlockedCards);

      if (!isSpeakerFinalized) {
        leadingShouldCorrect = false;
        break;
      }

      if (hasConfirmedSpeakerSwitch) {
        this._commitLockedCard(card0, speechEndSec, speechStartSec);
        unlockedCards = this._buildUnlockedCardGroups();
        continue;
      }

      const latestSeg =
        this.diarizedSegments.length > 0
          ? this.diarizedSegments[this.diarizedSegments.length - 1]
          : null;
      const onset = this.activeVadStartSec ?? this.lastVadStartSec;
      const hasUnemittedVadTurn =
        latestSeg !== null && onset !== null && onset > latestSeg.endTime + 0.05;
      const isQuiet =
        !this.isVadSpeaking && !hasUnemittedVadTurn && unlockedCards.length === 1;
      const silenceSec =
        isQuiet && effectiveNowSec >= speechEndSec ? effectiveNowSec - speechEndSec : 0;

      if (silenceSec + 1e-9 >= TRANSLATE_LOCK_SILENCE_SEC) {
        this._commitLockedCard(card0, speechEndSec, speechStartSec);
        unlockedCards = this._buildUnlockedCardGroups();
        continue;
      }

      leadingShouldCorrect = silenceSec + 1e-9 >= CORRECT_TRANSCRIPT_SILENCE_SEC;
      break;
    }

    const result = this.lockedTurns.map((t) => ({
      turnId: t.turnId,
      clusterId: t.clusterId ?? null,
      speakerId: t.speakerId,
      parts: t.parts,
      rawText: t.rawText,
      hasInterim: false,
      shouldCorrect: true,
      locked: true,
      userLikelihood: t.userLikelihood ?? null,
      pauseMs: t.pauseMs ?? null,
    }));

    let prevSpeechEndSec =
      this.lockedTurns.length > 0
        ? this.lockedTurns[this.lockedTurns.length - 1].speechEndSec
        : null;

    for (let i = 0; i < unlockedCards.length; i++) {
      const card = unlockedCards[i];
      const { speechStartSec, speechEndSec } = this.getSpeechBoundsForSpan(
        card.speakerId,
        card.minTimeSec,
        card.maxTimeSec,
        prevSpeechEndSec ?? -Infinity
      );
      const pauseMs =
        prevSpeechEndSec !== null
          ? Math.max(0, Math.round((speechStartSec - prevSpeechEndSec) * 1000))
          : null;
      prevSpeechEndSec = speechEndSec;

      result.push({
        turnId: this.nextTurnId + i,
        clusterId: this.getClusterIdForSpan(
          card.speakerId,
          card.minTimeSec,
          card.maxTimeSec
        ),
        speakerId: card.speakerId,
        parts: card.parts,
        rawText: card.rawText,
        hasInterim: card.hasInterim,
        shouldCorrect: i === 0 ? leadingShouldCorrect : false,
        locked: false,
        userLikelihood: this.getUserLikelihoodForSpan(
          card.speakerId,
          card.minTimeSec,
          card.maxTimeSec
        ),
        pauseMs,
      });
    }

    return result;
  }

  /**
   * Builds merged speaker dialog card groups ready for incremental DOM rendering, respecting
   * permanently locked turns so subsequent speech after a `>= 2.0s` lock opens a new card.
   *
   * @returns {Array<{
   *   turnId: number,
   *   speakerId: string | null,
   *   parts: Array<{text: string, isInterim: boolean}>,
   *   rawText: string,
   *   hasInterim: boolean,
   *   shouldCorrect: boolean,
   *   locked: boolean
   * }>}
   */
  buildGroupedCards() {
    return this.syncTurns(this.lastSessionTimeSec);
  }
}
