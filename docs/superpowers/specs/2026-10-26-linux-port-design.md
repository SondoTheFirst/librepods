# LibrePods Linux port: design

Date: 2026-10-26. Branch: `linux-port` (worktree `~/librepods-linux-rust`, based on `origin/linux/rust` 672e65a).

## Intent

The user runs LibrePods' Rust Linux app as an AppImage, autostarted, with AirPods Pro 1st gen (model A2084). The system is Bazzite / Fedora 44 with KDE Plasma 6 on Wayland and PipeWire + WirePlumber. Everything new in LibrePods since May 2026 has been Android-only: v1.0.4 on `main` and the v2 rewrite on `android/rewrite` (v2.0.0-alpha.4). The user asked: "go ahead and just look at the code this time. Bring it all over to linux!"

The user skipped the design questions, so **every decision below is Claude's call**. They are flagged here so the user can overrule any of them.

The immediate pain is audio dropping out. On 2026-10-26 at 09:53, three stale device instances each restarted WirePlumber. The AirPods vanished for about 40 seconds and SoundCloud ended up paused. Reliability comes before features.

## Goals

- Port every Android feature that can run over BlueZ L2CAP/BLE, D-Bus, MPRIS or PipeWire from Linux userland. Gate features per AirPods model, as Android does.
- Make the app reliable:
  - one live device instance per AirPods;
  - no WirePlumber or PipeWire restarts;
  - no deadlocks;
  - no panics on malformed packets (the release profile is `panic = "abort"`).
- Ship each stage as a working AppImage that can run alongside the user's current one.

## Non-goals

- Android-only plumbing: Xposed/native hooks, root, billing and premium gating, Play/FOSS banners, Health Connect, telephony call answering, QS tiles, home-screen widgets.
- Upstreaming. Everything stays local unless the user asks.

## Anti-goals (these count as failure even if the code works)

- Making audio less reliable than today. That includes:
  - restarting audio services;
  - overriding a profile the user picked by hand while one is already active;
  - touching any card that isn't the AirPods (HDMI, built-in).
- Silently changing system config: `/etc/bluetooth/main.conf`, the active icon theme, or the user's autostart entry. These need explicit user approval each time.
- Killing or replacing the user's running LibrePods or any other app without asking first.

## Decisions (Claude's calls)

1. **Base:** the existing Rust app (iced 0.14, ksni tray, bluer 0.17, libpulse-binding, dbus 0.9). Keep its architecture and idioms. Split files only where a stage needs it.
2. **Build:** in distrobox `librepods-dev` (Fedora 44, rustc 1.99, all -devel libs installed). The host has no Rust toolchain.
3. **Capability gating:** use the rewrite's static model table, including the Max models and a permissive UNKNOWN fallback. Log the parsed 0x02 capabilities, but don't gate on them yet. Show "Adaptive" only with ADAPTIVE_AUDIO; this fixes an Android bug, since Pro 1 has no Adaptive mode.
4. **ATT features** (Transparency customization, Loud Sound Reduction, Hearing Aid):
   - They need BlueZ `DeviceID = bluetooth:004C:0000:0000`, a root edit to main.conf; the upstream README warns it can cause disconnects.
   - The app detects whether the ATT channel works, and if not, shows the setup steps.
   - Claude never applies that config itself.
5. **Android UI mapped to Linux equivalents:**

   | Android | Linux |
   |---|---|
   | QS tile, widget, control-centre dialog | tray menu items, plus a `librepods-ctl` CLI over a session D-Bus interface |
   | Island and proximity card | desktop notifications first; a layer-shell overlay is optional, in the final stage |
   | Head-gesture call answering and the assistant action | user-configurable shell commands |

6. **AirPods mic over AACP (0x58):** an experimental PipeWire virtual source, gated by firmware version. Last stage, because Pro 1 support is unknown.
7. **Install alongside:** the new AppImage gets a new filename in `~/Applications`. The old AppImage and its autostart stay until the user approves a switch.
8. **A2DP recovery:** BlueZ `Device1.ConnectProfile(0000110b-0000-1000-8000-00805f9b34fb)` replaces WirePlumber restarts.

## Roadmap (one implementation plan per stage, each ending in a working AppImage)

1. **Stability core.**
   - Device lifecycle: one instance per MAC, with teardown on disconnect.
   - No WirePlumber restarts; the card is looked up by MAC on every operation.
   - Fix deadlocks; make parsers panic-safe.
   - Persist devices.json safely on first connect.
2. **Protocol and device model.**
   - Full opcode and control-ID catalog; parsers for every opcode in the reference below.
   - Rewrite init sequence; bud-role-aware ear detection.
   - Model and capability table.
   - BLE buds and case adverts: decrypt, battery, lid, in-ear, connection state.
   - Fix LE auto-connect.
   - Battery reported to BlueZ `BatteryProviderManager1`.
3. **Settings and controls.**
   - AirPods page rebuilt per capability, covering every control command in the reference.
   - Listening-mode cycle; ear-detection behaviour settings.
   - Conversation-awareness ducking settings.
   - Custom EQ (0x63) and headphone accommodation (0x53).
   - Stem actions; fixed rename.
   - Takeover preferences.
   - Tray quick controls, notifications, `librepods-ctl`.
4. **ATT features.** Transparency customization, LSR, Hearing Aid and audiogram, plus DeviceID setup guidance.
5. **Sensors.** RTBuddy protobuf head tracking; head gestures mapped to actions; heart rate (Pro 3).
6. **Mic over AACP.** 0x58 stream → AAC-ELD decode → PipeWire source. Experimental.
7. **Polish and ship.**
   - Overlays (proximity card / island equivalent).
   - Packet log / debug view; theming.
   - Final AppImage and autostart switch, with the user's approval.

## Hardware notes for the user's AirPods (A2084)

When connected, the buds report only these control IDs:
- 0x0D listening mode (02/03)
- 0x1B one-bud ANC (02)
- 0x24 call management (00 03)
- 0x18 click-hold interval (00)
- 0x17 double-click interval (00)
- 0x1F chime volume (50 50)

Battery status byte 0x03 occurs for the left bud. The current app drops that entry.

Info packets report firmware "51.9.6" and "65.1868046005000000.52"; version3 is "3344646" or "6619472".

BLE is currently dead on this system. The running app logs `LE monitor error: ... Method "RegisterMonitor" ... on interface "org.bluez.AdvertisementMonitorManager1" doesn't exist`, which means BlueZ's experimental interfaces are off. Stage 2 must not depend on AdvertisementMonitor. Use plain LE discovery (`Adapter1.SetDiscoveryFilter` + `StartDiscovery`, then read `ManufacturerData` from `PropertiesChanged`), or offer the main.conf `Experimental = true` change for the user to apply.

Pro 1 capability per the model table: LISTENING_MODE only. Generic controls (mic, call controls, press speed/duration, one-bud ANC, tone volume, ear detection, automatic connection, optimized charging, rename) are shown for every model and are hardware-testable.

---

## Protocol reference (condensed from the Android main and rewrite sources)

Main paths are `~/librepods/android/app/src/main/java/me/kavishdevar/librepods/`. The rewrite core path is `~/librepods-android-rewrite/android/core/src/main/kotlin/me/kavishdevar/librepods/core/`.

### AACP framing

- Transport: L2CAP BR/EDR SEQPACKET, PSM 0x1001.
- Message framing: `04 00 04 00 [opcode u8] 00 [payload]`.
- Handshake: raw 16 bytes, `00 00 04 00 01 00 02 00` + 8×`00`. The rewrite uses `... 01 00 03 00 ...`.
- In this codebase, `payload = packet[4..]`. So `payload[0]` is the opcode, `payload[1]` is 0x00, and the fields start at `payload[2]`.
- Init sequence (rewrite, 200 ms apart, repeated after 3 s):
  1. handshake
  2. 0x4D feature flags `D7 00 00 00 00 00 00 00` (the current Linux app sends `FF ...`)
  3. 0x0F notifications `FF FF FF FF`
  4. 0x29 `00 FF FF FF FF FF FF FF` (the current Linux app sends `FF` for the first byte)
  5. 0x30 magic keys `05 00`
- Control command: `09 00 [id] [v0 v1 v2 v3]`, always 4 value bytes, zero-padded. On receive, trailing zeros are trimmed. Booleans are 0x01 = on, 0x02 = off.

### Opcodes (R = host receives, S = host sends)

| Opcode | Name | Payload (fields from `payload[2]`) |
|---|---|---|
| 0x02 R | capabilities | `[count]{[capId][value]}`. The value is 4 bytes for capIds 03, 04, 06, 07, 30, else 1 byte. An unknown capId must not abort the parse. |
| 0x04 R | battery | `[n]` then n × `[component][01][level][status][01]`. Component: 01 headset, 02 right, 04 left, 08 case. Status: 00 unknown, 01 charging, 02 not charging, 04 disconnected, 05 optimized; 03 is seen on A2084, meaning unknown. |
| 0x06 R | ear detection | `[primary][secondary]`. 00 in ear, 01 out, 02 in case, 03 disconnected. Map to left/right using bud role (0x08). |
| 0x08 R | bud role | `[0]`: 01 = left primary, 02 = right primary, else headset. |
| 0x09 R/S | control command | see the control IDs below |
| 0x0C R | MAC address | `[MAC×6 reversed][extra1][extra2]` |
| 0x0E R | audio source | `[MAC×6][type]`. Main reverses the MAC, the rewrite doesn't. **Verify on hardware**: the A2084 sends the local adapter address `B0:60:88:AF:DD:DA`, and Rust's reversed decoding produces exactly that, so reversed is correct. Type: 00 none, 01 call, 02 media, 04 and 06 unknown. |
| 0x0F S | request notifications | `FF FF FF FF` |
| 0x10 S | smart routing | `[target MAC reversed×6][len u16 LE][OPACK payload, prefixed 01]` |
| 0x11 R | smart routing response | `[sender MAC reversed×6]` + strings: SetOwnershipToFalse, ReverseBannerTapped, ShowNearbyUI |
| 0x17 R/S | RTBuddy (head tracking / heart rate) | `[descriptor u32 LE][len u16 LE][data]`. Descriptor 0x00100000 = protobuf SensorDataWX. |
| 0x19 R | stem press | `[type][bud]`. Type: 05 single, 06 double, 07 triple, 08 long. Bud: 01 L, 02 R. |
| 0x1A S | rename | `01 [len] 00 [UTF-8 name]` (main's layout; the rewrite's layout is wrong) |
| 0x1D R | information | NUL-separated strings from `payload[4]`; drop the first token. Then: name, model, manufacturer, serial, version1, version2, hw revision, updater id, left serial, right serial, version3. |
| 0x2E R | connected devices | count at `payload[4]` (Rust currently reads `payload[2]`, which is wrong); entries at `payload[5 + i*8]`: `[MAC×6 not reversed][info1][info2]` |
| 0x30 S / 0x31 R | magic keys | request `05 00` (IRK 01 \| ENC 04). Response: `[count]{[type][?][len][?][key]}` |
| 0x4B R | conversation awareness | packet is 10 bytes; level at `packet[9]`. 1 = start, 2 = reduce, 3 = partial, 4/6/7/8/9 = stop. |
| 0x4D S | source feature capabilities | `D7 00 00 00 00 00 00 00` |
| 0x53 R/S | headphone accommodation | 140-byte packet: `84 00 02 02 [phone 01/02] [media 01/02]` + 4×8 float32 LE. Receive reads media at `[10]` and phone at `[11]`, the reverse of send. |
| 0x58 R/S | mic stream | start `00 00 09 00 00 01 82 00 00 00 04 96 00`; stop `00 00 02 00 03 01`. Incoming audio has payload `01 00`; frames from packet offset 22 are `[u32 LE ts][u8 len][AAC-ELD]`. ASC `F8 E6 30 00`. |
| 0x63 R/S | custom EQ | send `05 00 01 [state 1 off / 2 on] [low] [mid] [high]` (0–100, 50 = flat) |

Other names, for logging only: 01, 03, 05, 07, 0B, 0D, 12–16, 1B, 1E–28, 2B–2D, 32, 40, 44–4A, 4C, 4E–57, 59–64. See rewrite `bluetooth/aacp/types/Opcodes.kt`.

### Control IDs (0x09)

Values are listed with the identifier.

**Audio and listening modes**
- 01 MIC_MODE: 0 auto, 1 always right, 2 always left
- 0A EAR_DETECTION_CONFIG: bool
- 0D LISTENING_MODE: 1 off, 2 ANC, 3 transparency, 4 adaptive
- 1A LISTENING_MODE_CONFIGS: bitmask, off 01, ANC 02, transparency 04, adaptive 08; at least 2 bits set
- 1B ONE_BUD_ANC_MODE: bool
- 26 ADAPTIVE_VOLUME_CONFIG (personalized volume): bool
- 28 CONVERSATION_DETECT_CONFIG: bool
- 2E AUTO_ANC_STRENGTH: 0–100 (Android sends 100 − slider)
- 34 ALLOW_OFF_OPTION: bool

**Presses and gestures**
- 16 CLICK_HOLD_MODE: `[right][left]`, 01 noise control, 05 Siri
- 17 DOUBLE_CLICK_INTERVAL: 0 default, 1 slower, 2 slowest
- 18 CLICK_HOLD_INTERVAL: same values as 17
- 1F CHIME_VOLUME: `[0..100][0x50]`
- 23 VOLUME_SWIPE_INTERVAL: Android uses 1/2/3; the docs say 0/1/2. Verify.
- 24 CALL_MANAGEMENT_CONFIG: `00 03` = press once to mute, twice to hang up; `00 02` = flipped
- 25 VOLUME_SWIPE_MODE: bool
- 39 RAW_GESTURES_CONFIG / STEM_CONFIG: forwards presses to the host; bitmask single 01, double 02, triple 04, long 08

**Connection and ownership**
- 06 OWNS_CONNECTION: 01 own, 00 don't
- 20 AUTOMATIC_CONNECTION / SMART_ROUTING_MODE: bool
- 36 ALLOW_AUTO_CONNECT: observed only

**Hearing**
- 2C HEARING_AID: `[enrolled][enabled]`; on = `01 01`, off = `01 02`
- 2F HPS_GAIN_SWIPE: bool
- 33 HEARING_ASSIST_CONFIG: bool
- 37 PPE_TOGGLE: bool
- 38 PPE_CAP_LEVEL: unknown encoding

**Device behaviour**
- 35 SLEEP_DETECTION: bool
- 3B DYNAMIC_END_OF_CHARGE (optimized charging): bool

**Extras, from the docs:**
- 12 VOICE_TRIGGER
- 1E AUTO_ANSWER
- 27 SOFTWARE_MUTE
- 29 SSL
- 30 HRM_STATE
- 31 IN_CASE_TONE
- 32 SIRI_MULTITONE
- 3A TEMP_MANAGED_PAIRING
- 3C SYSTEM_SIRI_MODE
- 3D HEARING_AID_GENERIC
- 3E/3F UPLINK_EQ
- 40 IN_CASE_TONE_VOLUME
- 41 DISABLE_BUTTON_INPUT
- 42 EXTENDED_HOLD_AND_RELEASE

**Name-only IDs (no known encoding):** 02 SCAN, 03 RESET, 04, 05 BUTTON_SEND_MODE, 07, 08 BUD_ROLE, 09, 0B, 0C, 0E/0F HRM, 11 SWITCH_CONTROL, 13, 14 SINGLE_CLICK_MODE, 15 DOUBLE_CLICK_MODE, 1C CROWN_ROTATION, 22 HFP_UPLINK.

### Models (rewrite `devices/AirPods.kt`)

| Model | Model numbers | Capabilities |
|---|---|---|
| AirPods 1 | A1523, A1722 | none |
| AirPods 2 | A2032, A2031 | none |
| AirPods 3 | A2565, A2564 | HEAD_GESTURES |
| AirPods 4 | A3053, A3050, A3054 | HEAD_GESTURES, SLEEP_DETECTION, ADAPTIVE_VOLUME |
| AirPods 4 ANC | A3056, A3055, A3057 | LISTENING_MODE, CONVERSATION_AWARENESS, HEAD_GESTURES, ADAPTIVE_AUDIO, SLEEP_DETECTION, ADAPTIVE_VOLUME, STEM_CONFIG |
| AirPods 5 | A3531, A3532, A3533 | same as AirPods 4 ANC |
| AirPods 5 WCC | A3439, A3440, A3441 | AirPods 5 + SWIPE_FOR_VOLUME |
| **AirPods Pro 1** | **A2084, A2083** | **LISTENING_MODE** |
| Pro 2 Lightning | A2931, A2699, A2698 | LISTENING_MODE, CONVERSATION_AWARENESS, STEM_CONFIG, LOUD_SOUND_REDUCTION, SLEEP_DETECTION, HEARING_AID, ADAPTIVE_AUDIO, ADAPTIVE_VOLUME, SWIPE_FOR_VOLUME, HEAD_GESTURES |
| Pro 2 USB-C | A3047, A3048, A3049 | same as Pro 2 Lightning |
| Pro 3 | A3063, A3064, A3065 | Pro 2 + PPE + HRM |
| Max Lightning | A2096 | LISTENING_MODE (headset battery component) |
| Max USB-C | A3184 | LISTENING_MODE (headset battery component) |
| Max 2 | A3454 | LISTENING_MODE, CONVERSATION_AWARENESS, LOUD_SOUND_REDUCTION, ADAPTIVE_AUDIO, ADAPTIVE_VOLUME |
| UNKNOWN | — | all capabilities |

BLE model IDs (big-endian):

| ID | Model |
|---|---|
| 0E20 | Pro 1 |
| 1420 | Pro 2 |
| 2420 | Pro 2 USB-C |
| 0220 | AirPods 1 |
| 0F20 | AirPods 2 |
| 1320 | AirPods 3 |
| 1920 | AirPods 4 |
| 1B20 | AirPods 4 ANC |
| 0A20, 1F20 | Max |

### BLE (rewrite `bluetooth/ble/BLEManager.kt`)

Manufacturer data for company 0x004C. Offsets below are into the data after the company ID.

**Buds advert:** `07 19 01 ...`
- Accepted only if the address resolves against an IRK. Resolution uses `ah()` = AES-128-ECB with the key and data byte-reversed; the hash is the low 3 bytes of the address and prand the high 3.
- Plain bytes:
  - `[3..4]` model ID
  - `[5]` status:
    - bit 5 = primary is left;
    - bit 6 = this bud is in the case;
    - in-ear flags are 0x02 and 0x08; swap them when `primaryLeft XOR inCase`.
  - `[8]` lid: bit 3 = 0 means open. Count it as open only if a bud is charging and not both are in ear.
  - `[9]` colour
  - `[10]` state: 00 disconnected, 04 idle, 05 music, 06 call, 07 ringing, 09 hanging up
- Encrypted part: the last 16 bytes, AES-128-ECB with ENC_KEY.
  - `[1]` = primary bud, `[2]` = secondary bud, `[3]` = case.
  - Each is bit 7 = charging, low 7 bits = percent; 0xFF = absent.

**Case advert:**
- A TLV record of type 0x07 whose payload starts with `06`, followed by 16 encrypted bytes. It is not RPA-resolved, so try every ENC_KEY.
- A plaintext is valid when `pt[1]==0x20`, `pt[15]==0`, and each of `pt[3..5]` is 0xFF or has `(b & 0x7F) <= 100`.
- `[3]` = case, `[4]` = left, `[5]` = right. Fixed order, no primary flip.

### ATT (main `bluetooth/ATTManager.kt`; the rewrite stubs ATT out)

Transport: L2CAP BR/EDR PSM 0x1F. Requires `DeviceID = bluetooth:004C:0000:0000`.

| Handle | Purpose | Layout |
|---|---|---|
| 0x18 (CCCD 0x19) | Transparency | 100 or 104 bytes of float32 LE: `enabled`; then left `EQ[8]`, amplification, tone, conversation boost, ambient noise reduction; then right, the same 5 fields; then optional own-voice |
| 0x1B | Loud Sound Reduction | 1 byte; its CCCD does not work |
| 0x2A (CCCD 0x2B) | Hearing Aid | ≥ 104 bytes: header `02 02 60 00` (on write, byte[2] = 0x64); left EQ @4; left amplification @36, tone @40, conversation boost @44, ambient noise reduction @48; right EQ @52; right amplification @84, tone @88, conversation boost @92, ambient noise reduction @96; own-voice @100 |

- Writes are read-modify-write of the last-read blob, debounced 100 ms.
- Enabling the hearing aid also needs control commands 0x2C = `01 01` and 0x33 = 01.

### RTBuddy (rewrite `core/src/main/proto/rtbuddy.proto`)

**SensorDataWX**

| Field | Name |
|---|---|
| 1 | seq |
| 7 | command {1 service, 3 payload} |
| 8 | service_settings {1 service, 2 setting, 3 configuration} |
| 9 | start_ack |
| 12 | command_ack |

**Starting and stopping head tracking**
- Start: service_settings with setting 2 and configuration `01` + u32 LE interval in µs. Stop: interval 0.
- Service by firmware major version and model:

  | Condition | Service |
  |---|---|
  | firmware major < 8 | ACTIVITY (14) |
  | firmware major ≥ 8 | DEVMOTION6 (16) |
  | AirPods 5 | CMA_COMMAND (79) |

- Example start packet (ACTIVITY, 40 ms): `04 00 04 00 17 00 | 00 00 10 00 | 0D 00 | 42 0B 08 0E 10 02 1A 05 01 40 9C 00 00`.

**Head-tracking data**
- Arrives as a 58-byte command payload.
- i16 LE values: horizontal at [28], vertical at [30].
- Main's legacy format: orientation at 43/45/47 and acceleration at 51/53 of the full packet.

**Gesture detector** (main `utils/GestureDetector.kt`)
- Ignore readings with |v| > 6000.
- Count 3–4 extremes, with confidence ≥ 0.7.
- Nod = yes, shake = no.

**Heart rate**

| Item | Value |
|---|---|
| Service (firmware ≥ 9) | HEARTRATE_COMMAND (84) |
| Service (otherwise) | HEARTRATE (19) |
| Sample interval | 1 s |
| Data packet | 18-byte payload |
| bpm | `payload[1]` |
| Validity | ignore the sample if `payload[17]` ≠ 0 |

### Smart routing / ownership (main `services/AirPodsService.kt` §takeOver)

**OPACK encoding** (implement a small encoder; don't copy fixed buffers)

| Byte | Meaning |
|---|---|
| 0x40+n | string of length n |
| 0xE0+n | dict with n entries |
| `30 xx` | int8 |
| `31 xx xx` | int16 |
| 0xA0+n | back-reference |

**Messages:**
- MediaInformation: PlayingApp, HostStreamingState, btAddress, btName, otherDeviceAudioCategory
- MediaInformationNewDevice
- HijackRequest ("Hijackv2"): localscore, remotescore, audioRoutingScore, audioRoutingSetOwnershipToFalse
- SmartRoutingShowUI
- HijackReversed ("ReverseBannerTapped")
- AddTiPiDevice: newTipi, idleTime, nearbyAudioScore

**Ownership handling:**
- On receiving 06=00, or a 0x11 SetOwnershipToFalse: pause, release audio, and suppress takeover for 3 s.
- If the audio source is another MAC with type ≠ none: yield (send 06=00).

### Android bugs to not copy

1. Adaptive Audio is gated on ADAPTIVE_VOLUME.
2. A forwarded PLAY_PAUSE press does nothing.
3. Hearing-aid data gets written to the transparency handle.
4. Opening the Accessibility screen sends a 0x53 packet.
5. 0x53 swaps phone and media between send and receive.
6. Rename is sent on every keystroke.
7. `connectAudio` runs on every battery packet.
8. Takeover stays blocked after the app sends 06=00 itself.
9. Head-tracking start/stop on every connect.
10. ProxConnect compares timestamps from mismatched clocks.
11. The rewrite's rename layout.
12. The rewrite stubs out ATT.
