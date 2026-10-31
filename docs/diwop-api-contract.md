# DiWoP Conversation API – Contract Draft

The API corrects spoken contributions and prepares contributions from the conversation partner in German simple language. It provides two endpoints:

| Method | Path | Purpose |
| --- | --- | --- |
| `POST` | `/correct-transcript` | Correct spoken text |
| `POST` | `/translate` | Prepare the latest partner contribution based on the conversation history |

The following specifications describe the proposed contract, not the current implementation status.

## Common Conventions

- The configured server URL serves as the base URL. The endpoint paths are appended below it. Example: `https://api.example.org/v1` → `https://api.example.org/v1/translate`.
- Request and response bodies use JSON and UTF-8.
- Requests send `Content-Type: application/json` and `Accept: application/json`.
- A configured API key is transmitted as `Authorization: Bearer <api-key>`.
- Additionally configured headers and the configured timeout apply to both endpoints.
- Speaker roles are `user` (app user) and `partner` (conversation partner). In `/translate` conversation histories, `partner_simple` represents a previously prepared simple-language partner contribution.
- A message field `message` is a string with at least one non-whitespace character.
- Errors use HTTP error status codes and the common error format below.

## 1. Correct Transcript

### `POST /correct-transcript`

The call is made after a spoken contribution is completed, both for `user` and for `partner`.

### Request

```json
{
  "role": "partner",
  "message": "sie müssen den antrag bis ende nächste woche abgeben"
}
```

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `role` | `"user" \| "partner"` | Yes | Speaker of the contribution |
| `message` | `string` | Yes | Spoken text to be corrected; non-empty |

Initially, no conversation history is transmitted. The correction processes the individual contribution independently.

### Successful Response: `200 OK`

```json
{
  "message": "Sie müssen den Antrag bis Ende nächster Woche abgeben."
}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `message` | `string` | Complete corrected text as plain text, no Markdown |

### Processing Rules

- Corrects recognition errors, spelling, capitalization, and punctuation.
- Preserves the meaning, language, and tone of the contribution.
- Does not add information and does not perform translation or linguistic simplification.
- Returns the text unchanged if no correction is required.
- Returns a non-empty text.

### App Behavior

- The app replaces the displayed text of the associated contribution with `message` from the response and updates that contribution in the stored conversation history so subsequent `/translate` requests use the corrected transcript.
- The correction is independent of an already running `/translate` call and is not a prerequisite for starting `/translate`: for the latest `partner` contribution, `/translate` starts immediately in parallel using the plain (uncorrected) transcript, while earlier contributions in `messages` use their corrected transcripts whenever available (falling back to the plain transcript if a correction is still pending or failed).
- In case of an error, the plain recognized text is retained in the UI and conversation history. The correction can be retried.

## 2. Prepare Partner Contribution

### `POST /translate`

The endpoint prepares the last partner contribution. Earlier conversation contributions (including previously prepared `partner_simple` responses) provide the context.

The request uses an object with `messages` instead of a bare array so that additional options can be added later.

### Request

```json
{
  "messages": [
    {
      "role": "partner",
      "message": "Guten Tag, wie kann ich Ihnen helfen?"
    },
    {
      "role": "partner_simple",
      "message": "Guten Tag.  \nWie kann ich Ihnen helfen?"
    },
    {
      "role": "user",
      "message": "Wann muss ich den Antrag abgeben?"
    },
    {
      "role": "partner",
      "message": "sie müssen den antrag bis ende nächste woche abgeben"
    }
  ]
}
```

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `messages` | `ConversationMessage[]` | Yes | Conversation history in chronological order ending with the partner contribution to prepare |
| `messages[].role` | `"user" \| "partner" \| "partner_simple"` | Yes | Speaker role (`user`, `partner`) or previously prepared simple-language partner contribution (`partner_simple`) |
| `messages[].message` | `string` | Yes | Corrected transcript if available (otherwise the plain recognized spoken text) for `user` and `partner`, or the prepared Markdown `content` for `partner_simple`; non-empty |

### Validation and Processing Rules

- `messages` contains at least one contribution.
- The last message must have the role `partner` (typically still as a plain, uncorrected transcript because `/translate` starts in parallel with `/correct-transcript`).
- Consecutive contributions with the same role are allowed; alternating roles is not required.
- For `user` and `partner` contributions, corrected transcripts from `/correct-transcript` are transmitted whenever already available; otherwise (such as for the latest `partner` contribution, or if a correction is still pending or failed), the plain recognized spoken text is transmitted.
- `/translate` does not wait for `/correct-transcript`. For a completed partner contribution, both endpoints are called in parallel.
- Completed `/translate` preparations (`content`, when non-empty) are recorded in the conversation history with role `partner_simple` immediately after the corresponding `partner` contribution and sent along in subsequent `/translate` requests.
- The response refers to the last partner contribution, takes the previous history into account as context, and runs a post-translation safety and EU AI Act classification step (see [Classification & Disclaimers](classification-and-disclaimers.md)).

### Successful Response without Warning: `200 OK`

```json
{
  "type": "translation",
  "content": "Geben Sie den Antrag **bis Ende nächster Woche** ab."
}
```

### Successful Response with Warning: `200 OK`

The following example illustrates the response format independently of the request above:

```json
{
  "type": "translation",
  "content": "Die aufbereitete Antwort steht hier.",
  "warning": {
    "headline": "Möglicherweise verletzender Inhalt",
    "body": "Diese Antwort enthält **abwertende Aussagen**.",
    "type": "warning",
    "revealable": true
  }
}
```

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `type` | `"formatting" \| "translation"` | Yes | Type of preparation. Use `%type=formatting%` or `%type=translation%` on staging environment to enforce. |
| `content` | `string` | Yes | Complete prepared response as Markdown text; empty (`""`) only when blocked by a `danger` classification (`safety` or `youth-protection`) |
| `warning` | `Warning` | No | Optional warning; omitted when there is no warning, not transmitted as `null` |
| `warning.headline` | `string` | If warning present | Short, non-empty headline as plain text |
| `warning.body` | `string` | If warning present | Non-empty explanation as Markdown |
| `warning.type` | `"danger" \| "warning" \| "info"` | If warning present | Warning level for display and icon. Use `%warning.type=danger%`, `%warning.type=warning%`, or `%warning.type=info%` on staging environment to enforce. |
| `warning.revealable` | `boolean` | If warning present | Determines whether the response text is initially hidden. Use `%warning.revealable%` on staging environment to enforce. |

### Meaning of the Response Type

- **`formatting`:** The content is structured or formatted. The wording is largely preserved.
- **`translation`:** The content is linguistically simplified or phrased more understandably. Here, the term refers to this linguistic preparation; a target language is not passed in this version.

### Warning and Revealing

| Situation | Display in the App |
| --- | --- |
| No `warning` | `content` is immediately visible. |
| `warning.revealable: false` and non-empty `content` | Warning and `content` are immediately visible (e.g., `info` / `uncertain`). |
| `warning.revealable: false` and `content: ""` | Only the `danger` warning is displayed; the translation was blocked (`safety` or `youth-protection`) and no reveal button is shown. |
| `warning.revealable: true` | The warning with headline and explanation as well as the button **„Antwort anzeigen“** are visible. `content` is only displayed after pressing the button (e.g., EU AI Act / RDG `warning` disclaimers or `danger` for `hate-speech` / `injection`). |

- `revealable` refers to `content`, not to `warning.body`.
- The warning level does not automatically trigger hiding. Exclusively `revealable` controls this behavior.
- Even when `revealable: true`, `content` is completely included in the response. Revealing is a local display action without another API call.
- Static *Leichte Sprache* headlines, bodies, and regulatory rationale for all categories are documented in [Classification & Disclaimers](classification-and-disclaimers.md).

### Markdown

`content` and `warning.body` support Markdown.

Embedded HTML is not part of the contract. The API must generate Markdown exclusively.

## 3. Common Error Format

Both endpoints use the same structure for errors:

```json
{
  "error": {
    "code": "LAST_MESSAGE_MUST_BE_PARTNER",
    "message": "The last message must come from the conversation partner."
  }
}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `error.code` | `string` | Stable, machine-readable error code |
| `error.message` | `string` | Human-readable error description |

Clients evaluate `code` and should not rely on the exact wording of `message`.

### HTTP Status Codes and Proposed Error Codes

| HTTP Status | Error Code | Usage |
| --- | --- | --- |
| `400` | `INVALID_JSON` | Request body is not valid JSON. |
| `401` | `UNAUTHORIZED` | Required authentication is missing or invalid. |
| `403` | `FORBIDDEN` | Access is not permitted. |
| `422` | `VALIDATION_ERROR` | Required field is missing, type or role is invalid, text is empty, or message list is empty. |
| `422` | `LAST_MESSAGE_MUST_BE_PARTNER` | Last contribution in `/translate` does not come from `partner`. |
| `429` | `RATE_LIMITED` | Too many requests. |
| `500` | `INTERNAL_ERROR` | Processing failed. |
| `503` | `SERVICE_UNAVAILABLE` | Service is temporarily unavailable. |

Content-related warnings are transmitted as `warning` in a successful `/translate` response. Technical errors use the error format and not a success status.

## 4. Intended Flow in the App

```text
Spoken contribution by user or partner completed
    → append plain recognized text to the conversation history
    ├─ POST /correct-transcript with the plain recognized text
    │   → replace displayed text of the contribution in the UI
    │   → update this contribution in the conversation history with the corrected text
    └─ if role is partner: in parallel POST /translate
        → pass conversation history (using corrected texts and partner_simple entries where available, and plain text for the latest partner contribution)
        → display prepared response and, if applicable, warning
        → insert prepared content as partner_simple right after this partner contribution in the conversation history
```

- For a `user` contribution, transcript correction takes place and updates both the UI and the stored conversation history entry when completed, without triggering an immediate `/translate` call.
- For a `partner` contribution, `/correct-transcript` and `/translate` start in parallel and are processed independently of each other. There is no prescribed order of responses.
- When `/correct-transcript` completes, the contribution's entry in the conversation history is updated in place so subsequent `/translate` requests receive the corrected transcript. When `/translate` completes, its `content` is inserted into the conversation history with role `partner_simple` immediately after that `partner` entry.
- If the correction fails, the plain recognized text remains visible in the UI and in the conversation history. The preparation continues independently; the correction can be retried separately.
- If the preparation fails, the UI correction continues independently. The preparation can be requested again using the current conversation history up to that `partner` contribution (including any transcript correction that completed in the meantime).
