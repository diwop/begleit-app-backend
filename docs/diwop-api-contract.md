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
- Roles are exclusively `user` (app user) and `partner` (conversation partner).
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

- The app only replaces the displayed text of the associated contribution with `message` from the response. The correction serves only for UI display.
- The originally recognized spoken text is kept separately and is used for `/translate`, including in subsequent conversation histories.
- The correction is independent of `/translate` and is not a prerequisite for calling it.
- In case of an error, the original text is retained. The correction can be retried.

## 2. Prepare Partner Contribution

### `POST /translate`

The endpoint prepares the last partner contribution. Earlier conversation contributions provide the context.

The request uses an object with `messages` instead of a bare array so that additional options can be added later.

### Request

```json
{
  "messages": [
    {
      "role": "user",
      "message": "wann muss ich den antrag abgeben"
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
| `messages` | `ConversationMessage[]` | Yes | Previous conversation history in chronological order |
| `messages[].role` | `"user" \| "partner"` | Yes | Speaker of the contribution |
| `messages[].message` | `string` | Yes | Originally recognized spoken contribution as plain text; non-empty |

### Validation and Processing Rules

- `messages` contains at least one contribution.
- The last message must have the role `partner`.
- Consecutive contributions with the same role are allowed; alternating roles is not required.
- For all contributions, the originally recognized spoken texts are transmitted, not the texts corrected for the UI.
- `/translate` does not wait for `/correct-transcript`. For a completed partner contribution, both endpoints are called in parallel.
- Generated preparations are not sent along as additional conversation contributions.
- The response refers to the last partner contribution and takes the previous history into account as context.

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
| `content` | `string` | Yes | Complete prepared response as non-empty Markdown text |
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
| `warning.revealable: false` | Warning and `content` are immediately visible. |
| `warning.revealable: true` | The warning with headline and explanation as well as the button **„Antwort anzeigen“** are visible. `content` is only displayed after pressing the button. |

- `revealable` refers to `content`, not to `warning.body`.
- The warning level does not automatically trigger hiding. Exclusively `revealable` controls this behavior.
- Even when `revealable: true`, `content` is completely included in the response. Revealing is a local display action without another API call.

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
    → retain originally recognized text for the conversation history
    ├─ POST /correct-transcript with the originally recognized text
    │   → replace displayed text of the contribution in the UI
    └─ if role is partner: in parallel POST /translate
        → pass conversation history with originally recognized texts
        → display prepared response and, if applicable, warning
```

- For a `user` contribution, transcript correction takes place, but no immediate `/translate` call is made.
- For a `partner` contribution, `/correct-transcript` and `/translate` start in parallel and are processed independently of each other. There is no prescribed order of responses.
- The app keeps the originally recognized text and the corrected display text separate. A UI correction does not change either an ongoing or a subsequent `/translate` request.
- If the correction fails, the original remains visible in the UI. The preparation continues independently; the correction can be retried separately.
- If the preparation fails, the UI correction continues independently. The preparation can be requested again with the originally recognized texts.
