# Sync API Contract

This document specifies the HTTP contract between the inkpaper firmware and the
`inkpaper-server` backend service. The firmware uses this endpoint for
bidirectional synchronization of server-hosted alarms and todos.

## Request

```
POST {server_url}
Authorization: Bearer {auth_token}
Content-Type: application/json

{
  "alarms": [{"id": 0, "enabled": true}],
  "todos": [{"id": 0, "done": true}]
}
```

### Headers

- **Authorization** (required): `Bearer {auth_token}` where `auth_token` is the
  authentication token configured on the device.
- The device uploads only locally editable state: alarm `enabled` and todo
  `done`. It never uploads text, schedules, additions, or deletions.
- Unknown IDs are ignored, so stale device data cannot recreate content that
  Desktop or Server deleted.
- The server merges these flags and returns its complete authoritative lists.

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

The server includes an **ETag** header on the response, and the firmware
caches its value in NVS. Current (POST-based) firmware does **not** send it
back as `If-None-Match` - every sync uploads state and gets a full 200
response back, so there is no conditional-request round trip on this path.
The cached value is kept mainly for diagnostics and for compatibility with
the legacy GET flow described below.

`GET /api/sync` and conditional HTTP 304 remain available for older firmware
that still sends `If-None-Match`, but current firmware always uses `POST` so
local completion/enabled changes are never discarded before upload.

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
