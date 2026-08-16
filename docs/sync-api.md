# Sync API Contract

This document specifies the HTTP contract between the inkpaper firmware and the
`inkpaper-server` backend service. The firmware polls this endpoint periodically
to stay synchronized with server-hosted alarms and todos.

## Request

```
GET {server_url}
Authorization: Bearer {auth_token}
If-None-Match: {etag}  (optional, only if a previous sync returned an ETag)
```

### Headers

- **Authorization** (required): `Bearer {auth_token}` where `auth_token` is the
  authentication token configured on the device.
- **If-None-Match** (optional): The ETag value from the previous successful sync
  response, if any. Omitted on the first request or if the cached ETag is lost.

## Response

### HTTP 200 OK

Returned when the server has new or updated content. Body is JSON shaped as:

```json
{
  "alarms": [
    {
      "id": 0,
      "hour": 7,
      "minute": 30,
      "repeat": "Daily",
      "enabled": true,
      "label": "Morning"
    },
    {
      "id": 1,
      "hour": 22,
      "minute": 0,
      "repeat": {
        "Once": {
          "year": 2026,
          "month": 12,
          "day": 25
        }
      },
      "enabled": true,
      "label": "Christmas alarm"
    }
  ],
  "todos": [
    {
      "id": 0,
      "text": "Buy groceries",
      "done": false
    },
    {
      "id": 1,
      "text": "Call home",
      "done": true
    }
  ]
}
```

#### Schema Details

**`alarms`** (array): List of alarms to store on the device. May be empty.

Each alarm object:
- **`id`** (u8): Unique identifier for this alarm; must not conflict within the
  list. Used for deduplication and tracking edits across syncs.
- **`hour`** (u8): Hour component of the alarm time (0–23, 24-hour format).
- **`minute`** (u8): Minute component of the alarm time (0–59).
- **`repeat`** (enum): Recurrence pattern. Either:
  - `"Daily"` (string) — fires every day at this time.
  - An object `{ "Once": { "year": u16, "month": u8, "day": u8 } }` — fires
    once on the specified calendar date (after which the firmware may discard
    it).
- **`enabled`** (bool): Whether this alarm is currently active.
- **`label`** (string): Human-readable name or description (may be empty).

**`todos`** (array): List of todos to store on the device. May be empty.

Each todo object:
- **`id`** (u8): Unique identifier for this todo; must not conflict within the
  list.
- **`text`** (string): The todo's description or task text (may be empty, though
  that's not useful).
- **`done`** (bool): Whether this todo is marked complete.

#### ETag Header (optional)

If the response includes an **ETag** header, the firmware caches its value and
includes it in the `If-None-Match` header on the next sync request. This allows
the server to return HTTP 304 if the content has not changed, saving bandwidth.

The ETag value is opaque to the firmware; it is cached and passed back
verbatim (quotes included, if the server sent them) in the next request's
`If-None-Match` header.

### HTTP 304 Not Modified

Returned when:
- The client included an `If-None-Match` header with a cached ETag, and
- The server's current alarm/todo list matches that ETag (i.e., no changes have
  been made since the last sync).

No body is present. The firmware does not modify its local stores and continues
using the previously-synced data.

### Other Status Codes

Any other response (4xx, 5xx, etc.) is treated as an error. The firmware logs
the status code and displays an error message to the user; the sync is aborted
and the device's local data remains unchanged.

## Error Handling

Errors that prevent the sync request from completing (network timeout, TLS
handshake failure, invalid server URL, malformed JSON response, etc.) are logged
and reported to the user via the on-device menu but do not disrupt the device's
normal operation. The locally-stored alarms and todos remain valid and will
continue to ring/display as scheduled.

## Timing and Scheduling

The firmware itself does not dictate a sync schedule. Syncing is initiated
manually by the user selecting "SYNC NOW" from the on-device menu. Future
versions may add automatic background syncing (e.g., every 30 minutes when
Wi-Fi is available), but this is not yet implemented.

## Security

- The server URL must be HTTPS (http:// URLs are not validated by the firmware,
  but the TLS certificate bundle is built into the ESP32-S3 firmware image and
  verifies the server's certificate against public CAs).
- The auth token is a bearer token; it should be a long, random string
  (e.g. 32+ characters) and must be kept secret.
- The sync request and response traverse Wi-Fi; only start syncing on a trusted
  network.
