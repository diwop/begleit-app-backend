# Dialog Classification, EU AI Act Disclaimers & Topic Monitoring

Every `POST /translate` request executes a two-stage language-model pipeline before composing the HTTP response:

1. **Preparation (`translate`, `"medium"` reasoning)**: Simplifies or formats the latest `partner` contribution in German *Leichte Sprache* using [`prompts/translate-system.md`](../prompts/translate-system.md) and dynamic semantic few-shot examples ([`prompts/translate-dynamic-few-shots.md`](../prompts/translate-dynamic-few-shots.md)).
2. **Classification (`classify-dialog`, `"low"` reasoning + JSON Schema)**: Evaluates the conversation history, the original `partner` statement, and the prepared *Leichte Sprache* output using [`prompts/classify-dialog-system.md`](../prompts/classify-dialog-system.md), [`prompts/classify-dialog.md`](../prompts/classify-dialog.md), and [`prompts/classify-dialog-schema.json`](../prompts/classify-dialog-schema.json) (`responseMimeType: "application/json"` with `responseJsonSchema`).

For comparison, `POST /correct-transcript` runs independently with **`"minimal"` reasoning** for low-latency transcript cleanup.

If either the translation step or the classification step fails (or returns JSON violating the `(type, category, topic)` schema hierarchy), `POST /translate` **fails closed** with `503 SERVICE_UNAVAILABLE` so unreviewed translations are never delivered to end users with cognitive impairments.

---

## Regulatory & Ethical Rationale

### 1. EU AI Act & German RDG Domain Disclaimers (`warning`)

Purely linguistic simplification of what a human conversation partner literally said (for example replacing *Hypertonie* with *Blut-Hoch-Druck*) falls under accessibility and language translation (**Article 50(4) / Recital 132 EU AI Act**) and passes without a warning (`pass` / `ok`).

However, *Leichte Sprache* rules frequently require **explaining** unfamiliar domain terms. Whenever the AI introduces **medical, financial, legal, or public-authority explanations** that were **not explicitly stated** by the speaker, the response touches regulated high-impact domains under **Regulation (EU) 2024/1689 (EU AI Act)** and German national law:

- **`medical`** ([`prompts/warnings/medical.md`](../prompts/warnings/medical.md)): Borders medical-information boundaries (**EU AI Act Annex I** / MDR and **Annex III Point 5(a)** healthcare access). Clarifies that the AI-generated explanation does **not** replace medical advice.
- **`financial`** ([`prompts/warnings/financial.md`](../prompts/warnings/financial.md)): Covers explanations of money, loans, insurance, or financial contracts (**EU AI Act Annex III Point 5(b) & (c)** creditworthiness and insurance). Clarifies that the translation is **not** financial advice.
- **`juridical`** ([`prompts/warnings/juridical.md`](../prompts/warnings/juridical.md)): Covers explanations of laws, legal rights, obligations, or legal consequences (**EU AI Act Annex III Point 8** legal interpretation / administration of justice and **German Legal Services Act — § 2 RDG**). Clarifies that the AI explanation is **not** legal advice (*Rechts-Beratung*).
- **`authorities`** ([`prompts/warnings/authorities.md`](../prompts/warnings/authorities.md)): Covers explanations of public-administration terms, applications, official notices (*Bescheide*), or social benefits (**EU AI Act Annex III Point 5(a)** eligibility and access to essential public assistance services). Clarifies that the AI explanation is **not** legally binding and advises checking directly with the public office (*Amt*).

All four `warning` categories preserve the prepared `content` and set `revealable: true` so the user reads the *Leichte Sprache* disclaimer first and can reveal the translation locally via **„Antwort anzeigen“**.

### 2. Ethical, Safety & Prompt-Injection Protection (`danger`)

`danger` overrides `warning`, `info`, and `pass`:

- **Hard-blocked categories (`content: ""`, `revealable: false`)**:
  - **`safety`** ([`prompts/warnings/safety.md`](../prompts/warnings/safety.md)): Statements describing actions that could harm people or are illegal in Germany. The translation is discarded (`content: ""`) and `type` is forced to `"translation"` so the client never deletes the original partner transcript card.
  - **`youth-protection`** ([`prompts/warnings/youth-protection.md`](../prompts/warnings/youth-protection.md)): Pornographic content or content endangering children or adolescents (*Jugendschutz*). Blocked with `content: ""` and `revealable: false`.
- **Consent-gated categories (`content` preserved, `revealable: true`)**:
  - **`hate-speech`** ([`prompts/warnings/hate-speech.md`](../prompts/warnings/hate-speech.md)): Disrespectful, insulting, discriminatory, or hateful language by the conversation partner. Hidden initially behind **„Antwort anzeigen“** so the user retains agency over whether to read hurtful statements spoken to them.
  - **`injection`** ([`prompts/warnings/injection.md`](../prompts/warnings/injection.md)): Prompt-injection attempts trying to override the AI's role or rules. Hidden initially behind **„Antwort anzeigen“** with a warning that the translation may be unreliable.

### 3. Uncertain Recognition or Ambiguous Input (`info`)

- **`uncertain`** ([`prompts/warnings/uncertain.md`](../prompts/warnings/uncertain.md)): Displayed when the spoken input is unclear or fragmented and the translation is uncertain. Shown immediately alongside `content` (`revealable: false`), prompting the user to ask their conversation partner for clarification.

---

## Classification Decision Matrix & Static Warning Texts

All static warning texts are authored in German *Leichte Sprache* (`Sie`-Ansprache, one thought per line with Markdown double-space line breaks, bold negations, hyphenated compounds, no exclamation marks) under [`prompts/warnings/`](../prompts/warnings/) and embedded into the binary at compile time via `include_str!`. Every file uses the format `<headline>\n\n<body>`.

| `type` | `category` | API `warning.type` | `warning.revealable` | `content` Behavior | Static Warning File |
| --- | --- | --- | --- | --- | --- |
| `pass` | `ok` | *(omitted)* | — | Shown immediately | — |
| `info` | `uncertain` | `"info"` | `false` | Shown immediately | [`prompts/warnings/uncertain.md`](../prompts/warnings/uncertain.md) |
| `warning` | `medical` | `"warning"` | `true` | Hidden behind „Antwort anzeigen“ | [`prompts/warnings/medical.md`](../prompts/warnings/medical.md) |
| `warning` | `financial` | `"warning"` | `true` | Hidden behind „Antwort anzeigen“ | [`prompts/warnings/financial.md`](../prompts/warnings/financial.md) |
| `warning` | `juridical` | `"warning"` | `true` | Hidden behind „Antwort anzeigen“ | [`prompts/warnings/juridical.md`](../prompts/warnings/juridical.md) |
| `warning` | `authorities` | `"warning"` | `true` | Hidden behind „Antwort anzeigen“ | [`prompts/warnings/authorities.md`](../prompts/warnings/authorities.md) |
| `danger` | `hate-speech` | `"danger"` | `true` | Hidden behind „Antwort anzeigen“ | [`prompts/warnings/hate-speech.md`](../prompts/warnings/hate-speech.md) |
| `danger` | `injection` | `"danger"` | `true` | Hidden behind „Antwort anzeigen“ | [`prompts/warnings/injection.md`](../prompts/warnings/injection.md) |
| `danger` | `safety` | `"danger"` | `false` | Blocked (`content: ""`, `type: "translation"`) | [`prompts/warnings/safety.md`](../prompts/warnings/safety.md) |
| `danger` | `youth-protection` | `"danger"` | `false` | Blocked (`content: ""`, `type: "translation"`) | [`prompts/warnings/youth-protection.md`](../prompts/warnings/youth-protection.md) |

> **Staging overrides (`ENVIRONMENT=staging`)**: Inline `%type=...%` and `%warning...%` test directives continue to override `type` and `warning` when explicitly present in a staging test message, while the classification step still executes so monitoring and admin `debugInfo` remain realistic.

---

## Dialog Topic Extraction & Monitoring

In the same structured JSON call, the classifier assigns the dialog (`Historie` + `Aussage`) to one of ten `topic` values defined in [`prompts/classify-dialog-schema.json`](../prompts/classify-dialog-schema.json):

- `medical` — Health, doctor visits, nursing, therapy, medication, hospital
- `shopping` — Shopping, supermarket, retail, gastronomy, everyday services
- `social` — Family, friends, leisure, housing, work, social interaction
- `education` — School, vocational training, university, courses, continuing education
- `emergency` — Emergencies, accidents, police, fire department, ambulance, acute danger
- `authorities` — Public offices, applications, IDs, official notices, social benefits
- `financial` — Banking, money, accounts, payments, loans, insurance, contracts
- `mobility` — Bus, train, public transit, traffic, travel, tickets, directions
- `juridical` — Law, courts, legal disputes, legal counsel, police questioning
- `other` — Fallback whenever none of the specific topics clearly applies

No raw user transcripts or personally identifiable dialog contents are exported in metrics or `INFO` logs:

1. **Structured `stdout` Logs & OpenTelemetry Span Events**: Each completed `POST /translate` request emits a `tracing::info!` event containing `topic`, `classification_type`, `classification_category`, `history_length`, translation token counts (`input_tokens`, `reasoning_tokens`, `output_tokens`), and classification token counts (`classify_input_tokens`, `classify_reasoning_tokens`, `classify_output_tokens`).
2. **OpenTelemetry Metrics**:
   - `diwop.translate.classification.count` (`Counter<u64>`): Incremented on every classified `/translate` request with attributes `topic`, `classification_type`, and `classification_category`.
   - `gen_ai.client.token.usage` (`Histogram<u64>`): Recorded separately for `endpoint="/translate"` and `endpoint="/translate/classify"` across `token_type` in `["input", "reasoning", "output"]`.
3. **Admin Diagnostics (`debugInfo.classification`)**: When authenticated as admin (for example in the `/dialog-translation` test interface), `TranslationResponse.debugInfo.classification` exposes `topic`, `classificationType`, `category`, prompts, token counts, reasoning thoughts, and raw JSON output, and the UI header aggregates token usage and EUR cost across both pipeline steps.
