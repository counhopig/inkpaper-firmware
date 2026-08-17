# Control Protocol (USB/BLE)

This document specifies the command protocol for configuring the inkpaper firmware
over USB serial or BLE control channels. Transport-specific details (USB framing,
BLE characteristics) are handled by the transport layer; this document describes
the command/reply schema that both transports implement.

## Transport-Agnostic Schema

### Commands

Commands are sent as JSON objects, one per line. Each command has a `cmd` field
identifying the operation, plus command-specific fields.

#### `{"cmd":"set_wifi","ssid":"NETWORK_NAME","password":"PASSWORD"}`

Configure Wi-Fi credentials and verify the connection before saving.

**Fields:**
- **`cmd`** (string, required): `"set_wifi"`
- **`ssid`** (string, required): The Wi-Fi network name (SSID), typically 1–32 characters.
- **`password`** (string, required): The WPA2 passphrase, typically 8–63 characters.

**Behavior:**
- Attempts to connect to the specified network to verify the credentials work.
- Saves credentials to persistent storage (NVS) only if the connection succeeds.
- Disconnects after verification (does not hold a persistent connection).

**Reply:** `Ok` on success, or `Error { message }` if verification failed.

---

#### `{"cmd":"set_server","url":"HTTPS_URL","token":"AUTH_TOKEN"}`

Configure the server URL and authentication token for syncing alarms and todos.

**Fields:**
- **`cmd`** (string, required): `"set_server"`
- **`url`** (string, required): The HTTPS endpoint to fetch alarms/todos from
  (e.g., `https://example.com/api/sync`).
- **`token`** (string, required): Bearer token for authenticating to the server
  (e.g., a 32+ character random string).

**Behavior:**
- Saves the URL and token to persistent storage immediately.
- Does NOT verify the server is reachable (would require network activity;
  validation is deferred to `sync_now`).

**Reply:** `Ok` on success, or `Error { message }` if NVS save fails.

---

#### `{"cmd":"sync_now"}`

Fetch alarms and todos from the configured server and apply them to local stores.

**Fields:**
- **`cmd`** (string, required): `"sync_now"`
- (No other fields.)

**Behavior:**
- Loads server config from storage (URL + token). Checks that system time is
  available (needed to re-arm the RTC alarm after applying synced data).
- Connects Wi-Fi via the process's shared `WifiManager`, then performs the
  bidirectional HTTPS POST described in
  [`sync-api.md`](sync-api.md): uploads the device's local alarm `enabled` /
  todo `done` flags, and applies the server's merged, authoritative alarm/todo
  lists from the response body. Text, schedules, additions, and deletions are
  never uploaded by the device.
- On HTTP 200, parses the response JSON, saves alarms/todos to local stores,
  and re-arms the PCF8563 hardware alarm slot. Caches any returned ETag
  (currently informational only - the POST request does not send
  `If-None-Match`, so every sync gets a full response).
- On HTTP error or network failure, returns an error; local data is left
  unchanged.
- Disconnects Wi-Fi again once the request completes (success or failure).

**Reply:** `Ok` on success, or `Error { message }` on failure.

---

#### `{"cmd":"get_status"}`

Query the device's current configuration state.

**Fields:**
- **`cmd`** (string, required): `"get_status"`
- (No other fields.)

**Behavior:**
- Returns a snapshot of whether Wi-Fi and server credentials are configured.
- Includes a flag for live Wi-Fi connection state if readily available
  (current implementation: always `false` unless explicitly implemented).

**Reply:** `Status { wifi_configured, server_configured, wifi_connected }`
(see Replies below).

---

#### `{"cmd":"clear_alarms"}`

Delete every locally stored alarm and disarm the PCF8563 hardware alarm slot.
Wi-Fi credentials, server configuration, and todos are preserved.

**Reply:** `Ok` on success, or `Error { message }` on failure.

---

#### `{"cmd":"set_timezone","offset_minutes":480}`

Persist a fixed local UTC offset and immediately shift the hardware RTC by the
difference from the previous offset. Valid range: -720 (UTC-12:00) through 840
(UTC+14:00). NTP synchronization applies the stored offset before writing RTC.

**Reply:** `Ok` on success, or `Error { message }` on failure.

---

### Replies

Replies are sent as JSON objects, one per line. Each reply has a `status` field
indicating success or failure, plus status-specific fields.

#### `{"status":"ok"}`

Command succeeded. No additional data.

---

#### `{"status":"error","message":"ERROR_DESCRIPTION"}`

Command failed. The `message` field contains a human-readable description of
the failure (e.g., "Connection verification failed: timeout").

**Fields:**
- **`status`** (string, required): `"error"`
- **`message`** (string, required): Description of the error.

---

#### `{"status":"status","wifi_configured":BOOL,"server_configured":BOOL,"wifi_connected":BOOL}`

Current device configuration (in reply to `get_status`).

**Fields:**
- **`status`** (string, required): `"status"`
- **`wifi_configured`** (bool, required): `true` if Wi-Fi credentials are stored in NVS.
- **`server_configured`** (bool, required): `true` if server URL + token are stored in NVS.
- **`wifi_connected`** (bool, required): `true` if currently connected to a Wi-Fi network
  (may be `false` if not actively checked; interpretation depends on implementation).

---

## USB Serial Framing (Phase 4)

The USB serial channel shares the same physical port as firmware log output
(`CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG`). To distinguish control frames from
ordinary `log::info!`/`log::warn!` messages, each is prefixed with a sentinel:

**Command frame (PC → device):**
```
>>IP <JSON_COMMAND>\n
```

**Reply frame (device → PC):**
```
<<IP <JSON_REPLY>\n
```

The `>>IP ` and `<<IP ` prefixes (with trailing space) are literal and required.
Any line not starting with `>>IP ` is treated as log output and ignored by the
reader.

### Example USB Session

```
# Device boots and logs
[INFO] Inkpaper NOTE4 Rust bring-up starting
[INFO] Power latch is high; rendering home screen

# PC sends a command (e.g., set Wi-Fi credentials)
>>IP {"cmd":"set_wifi","ssid":"MyNetwork","password":"SecurePassword"}

# Device logs internal operations
[INFO] USB control: Wi-Fi credentials saved for 'MyNetwork'

# Device sends reply
<<IP {"status":"ok"}

# PC sends another command
>>IP {"cmd":"sync_now"}

[INFO] USB control sync completed: 3 alarms, 5 todos
<<IP {"status":"ok"}

# PC queries device status
>>IP {"cmd":"get_status"}
<<IP {"status":"status","wifi_configured":true,"server_configured":false,"wifi_connected":false}
```

---

## BLE Framing (Phase 5)

BLE control is implemented via a GATT service with two characteristics,
both carrying plain JSON with no additional framing (GATT writes and
notifications are already message-delimited at the link layer).

### Service and Characteristics

**Service UUID (UUID128):** `d2c25e50-5e22-48d8-a8b3-34f2f8e2c7d4`

**Command Characteristic (WRITE):**
- UUID (UUID128): `d2c25e51-5e22-48d8-a8b3-34f2f8e2c7d4`
- Properties: `WRITE`
- Payload: JSON command object (e.g., `{"cmd":"get_status"}`)
- Max size: 512 bytes

**Reply Characteristic (NOTIFY):**
- UUID (UUID128): `d2c25e52-5e22-48d8-a8b3-34f2f8e2c7d4`
- Properties: `READ | NOTIFY`
- Payload: JSON reply object (e.g., `{"status":"ok"}`)
- Max size: 512 bytes

### Example BLE Session

1. PC tool discovers the Inkpaper service (`d2c25e50-5e22-48d8-a8b3-34f2f8e2c7d4`).
2. PC tool enables notifications on the reply characteristic.
3. PC tool writes a command to the command characteristic:
   ```
   {"cmd":"get_status"}
   ```
4. Once the pairing screen has been exited back to Home (see Limitations
   below - commands aren't dispatched while any menu screen is showing),
   the device processes the command and sends a reply via notification:
   ```
   {"status":"status","wifi_configured":true,"server_configured":false,"wifi_connected":false}
   ```

### Lifecycle

- BLE is **not** active by default; it costs ~150KB RAM.
- User enters "BLE PAIRING" menu item from Home to start advertising.
- GATT service is available to any BLE client that connects.
- On screen exit (HOLD), advertising stops and the GATT service is torn down.
- The same `control::dispatch` logic handles both USB and BLE commands,
  so the command/reply contract is identical between transports.

### Limitations (same as USB)

- Commands are only dispatched from the Home screen's main loop.
- While in any menu screen (including the pairing screen), commands queue
  up in an internal channel but aren't executed until the user backs out
  to Home.

---

## Error Handling

- **Malformed JSON:** Logged as a warning; command is dropped; no reply is sent.
- **Unknown command field:** JSON parse error; handled as above.
- **Missing required field:** JSON parse error; handled as above.
- **Command execution failure:** Command completes but returns `Error { message }`.

---

## Security

- Commands traverse the USB serial port or BLE link unencrypted. Assume the
  device is physically accessible when these channels are used.
- Wi-Fi and server credentials are stored unencrypted in NVS. An attacker with
  physical access to the device can extract them.
- Bearer tokens should be long, random, and kept confidential (treat like a
  password).

---

## Timing and Scheduling

Commands execute synchronously during the main loop's 20 ms poll cycle. Most
commands complete within that cycle; `sync_now` may block for several seconds
(network latency). The main loop feeds the Task Watchdog Timer during and after
command execution, so a hung sync will eventually reboot the device.

---

## Limitations (Current Implementation)

- Six commands are implemented: `set_wifi`, `set_server`, `sync_now`,
  `get_status`, `clear_alarms`, and `set_timezone`.
- `wifi_connected` flag in `get_status` always returns `false` (live Wi-Fi state
  checking is not yet wired in).
- No rate limiting or command queueing; commands are dispatched as they arrive.
- **Commands are only polled from the Home screen's main loop.** `UsbConsole`
  has no dedicated reader thread or queue - the main loop calls
  `poll_command()` once per 20 ms iteration, which does a non-blocking read
  of whatever bytes are currently available. Any screen reached via the
  navigation drawer (`screens::open_navigation` - Calendar/Alarms/Todos/
  Settings, and everything under Settings: Sync Now/BLE Pairing/Sleep) runs
  its own nested blocking button-poll loop and never calls `poll_command()`
  at all. If the device happens to be sitting in one of those screens (e.g.
  because someone is using it, or because opening the serial port triggered
  a spurious ENTER - see next point), bytes simply accumulate unread in the
  USB-Serial-JTAG driver's own buffer (capacity/behavior not verified by
  this firmware) until the device returns to Home and polling resumes. A PC
  tool that needs guaranteed responsiveness should treat "no reply within a
  few seconds" as "device is probably in a menu," not as a transport
  failure.
- **Opening the serial port can itself trigger a spurious ENTER press.**
  This board's USB-Serial-JTAG auto-reset circuitry (the same one `espflash`
  uses to enter the bootloader) wires the DTR/RTS control lines to GPIO0 -
  which is also the ENTER button. A serial library that asserts DTR/RTS on
  open (many do, by default) will physically pull GPIO0 low, which the
  firmware can't distinguish from a real button press. This was observed
  directly during Phase 4 hardware testing: opening the port and asserting
  DTR+RTS caused the device to open its on-device menu, which then (per the
  point above) stopped responding to further commands until backed out of.
  A PC tool implementation should either avoid asserting DTR/RTS when
  opening the port, or account for the device possibly landing on a menu
  screen immediately after connecting.
