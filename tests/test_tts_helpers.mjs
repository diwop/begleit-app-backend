import test from 'node:test';
import assert from 'node:assert/strict';

import {
  normalizeLeichteSpracheForSpeech,
  selectGermanVoice,
  TtsEchoGate,
  TTS_ECHO_GATE_TAIL_MS,
  TTS_DEFAULT_RATE,
} from '../static/transcript_alignment.js';

test('normalizeLeichteSpracheForSpeech strips bold, italic, headings and bullets', () => {
  const md = `## Wichtige Information
* Nehmen Sie **jeden Morgen 1 Tablette**.
* Die Tablette ist *sehr wichtig*.
Nehmen Sie die Tablette vor dem Essen.`;

  const expected = 'Wichtige Information. Nehmen Sie jeden Morgen 1 Tablette. Die Tablette ist sehr wichtig. Nehmen Sie die Tablette vor dem Essen.';
  assert.equal(normalizeLeichteSpracheForSpeech(md), expected);
});

test('normalizeLeichteSpracheForSpeech joins intra-compound hyphens and Mediopunkts while preserving acronyms', () => {
  const md = `Gehen Sie zur Kranken-Versicherung.
Messen Sie Ihren Blut·druck.
Schreiben Sie eine E-Mail an das Team.`;

  const expected = 'Gehen Sie zur Krankenversicherung. Messen Sie Ihren Blutdruck. Schreiben Sie eine E-Mail an das Team.';
  assert.equal(normalizeLeichteSpracheForSpeech(md), expected);
});

test('normalizeLeichteSpracheForSpeech handles empty or whitespace input gracefully', () => {
  assert.equal(normalizeLeichteSpracheForSpeech(''), '');
  assert.equal(normalizeLeichteSpracheForSpeech(null), '');
  assert.equal(normalizeLeichteSpracheForSpeech('   \n  \n  '), '');
});

test('selectGermanVoice chooses appropriate gender voice based on role type', () => {
  const mockVoices = [
    { name: 'Google Deutsch (Male - Martin)', lang: 'de-DE' },
    { name: 'Google Deutsch (Female - Marlene)', lang: 'de-DE' },
    { name: 'Alex', lang: 'en-US' },
  ];

  // Masculine role
  const maleVoice = selectGermanVoice(mockVoices, 'Arzt', 1);
  assert.equal(maleVoice?.name, 'Google Deutsch (Male - Martin)');

  // Feminine role
  const femaleVoice = selectGermanVoice(mockVoices, 'Ärztin', 1);
  assert.equal(femaleVoice?.name, 'Google Deutsch (Female - Marlene)');

  // Neutral role falls back to first German voice
  const neutralVoice = selectGermanVoice(mockVoices, 'Praxis-Team', 1);
  assert.equal(neutralVoice?.name, 'Google Deutsch (Male - Martin)');
});

test('TtsEchoGate enforces post-playback echo tail', () => {
  const gate = new TtsEchoGate(400);
  assert.equal(gate.isRecordingGated(1000), false);

  gate.startPlayback();
  assert.equal(gate.isRecordingGated(1050), true);

  gate.stopPlayback(2000);
  // Within 400ms tail:
  assert.equal(gate.isRecordingGated(2100), true);
  assert.equal(gate.isRecordingGated(2399), true);
  // After 400ms tail:
  assert.equal(gate.isRecordingGated(2401), false);
  assert.equal(gate.isRecordingGated(3000), false);
});
