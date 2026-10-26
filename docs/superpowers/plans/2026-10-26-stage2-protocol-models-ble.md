# Stage 2: Protocol, AirPods Models and BLE: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stage 2 does four things:
- the app knows which AirPods model it is talking to and what that model supports;
- it understands the rest of the AACP control vocabulary;
- it reads the AirPods' Bluetooth LE broadcasts on this machine, even though BlueZ lacks the experimental advertisement-monitor API, to get battery, lid and in-ear status;
- it publishes the AirPods battery to the desktop through BlueZ.

**Architecture:**
- **New pure modules:** `devices/models.rs` (model and capability table), `bluetooth/proximity.rs` (BLE advert decoding, RPA resolution, auto-connect gate) and `bluetooth/battery_provider.rs` (battery summary plus a zbus-served `org.bluez.BatteryProvider1`).
- **BLE scanner:** `le.rs` keeps the AdvertisementMonitor path. When that's unavailable it falls back to plain LE discovery, but only while no AirPods hold an AACP connection.
- **AACP:** gains the bud-role (0x08) and capabilities (0x02) parsers and the full control-ID catalog.

**Tech Stack:** Rust 2024, tokio, bluer 0.17, aes 0.9, zbus 5 (already in the tree via ksni, now a direct dependency with `default-features = false, features = ["tokio"]`).

**Spec:** `docs/superpowers/specs/2026-10-26-linux-port-design.md` (Roadmap item 2, Protocol reference → Models, BLE, Control IDs)

## Global Constraints

Everything in Stage 1's Global Constraints still applies:
- builds go in distrobox `librepods-dev` (the `ct` shorthand);
- never restart audio services and never touch non-AirPods cards;
- no `unwrap`/`expect`/indexing on AirPods data, user files or BLE data;
- `log` macros, with `#[cfg(test)] mod tests` in each module;
- local commits on `linux-port`, then push to `sondo`;
- never touch the user's running app without asking.

Stage 2 adds:
- **Adapter path.** The adapter is `hci1` on this machine. Never hard-code `hci0`; get the adapter from bluer `Session::default_adapter()` or from `GetManagedObjects`.
- **No BLE scanning during AACP.** Never run BLE discovery while any AirPods have a live AACP connection, so scanning can't disturb A2DP audio. Stop discovery within 2 s of an AACP connect.
- **Auto-connect off by default.** The default for BLE auto-connect is **off** when `~/.config/librepods/preferences.json` has no `autoConnect` for the MAC. The user has never had BLE auto-connect, so it must not start stealing the AirPods by surprise. Stage 3 adds the toggle.
- **Never change system config.** Don't edit `/etc/bluetooth/main.conf` or enable BlueZ experimental mode. If AdvertisementMonitor is missing, log it once at `info` and fall back to discovery.
- **Keep keys out of logs.** The LE keys (IRK/ENC) are secrets. Don't log them in full at `info` or `debug`. Log at most the first 4 hex chars followed by `…`.

## Review Focus

1. **BLE adverts from other Apple devices nearby** (phones, other people's AirPods). Only an advert whose address resolves against our IRK is treated as ours. Test: Task 4 `foreign_rpa_does_not_resolve`.
2. **Garbage or short manufacturer data.** The decoder returns `None` and doesn't panic. Test: Task 4 `no_decoder_panics_on_truncation`.
3. **Scanning while the AirPods are connected and playing audio.** The policy says don't scan. Test: Task 5 `scan_policy_never_scans_while_connected`.
4. **Auto-connect spam when the AirPods sit disconnected and advertising** (lid open on the desk). At most one attempt per MAC per cooldown, and nothing when the pref is off or missing. Tests: Task 5 `auto_connect_respects_cooldown`, `auto_connect_off_without_pref`.
5. **Unknown or future model numbers** (new AirPods, Beats). They fall back to `Unknown` with permissive capabilities, which never removes a control that works today. Test: Task 1 `unknown_model_falls_back_permissively`.

---

### Task 1: Model and capability table

**Files:**
- Create: `src/devices/models.rs`
- Modify: `src/devices/mod.rs` (add `pub mod models;`)

**Interfaces:**
- Produces, in `crate::devices::models`:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
  pub enum Capability { ListeningMode, ConversationAwareness, StemConfig, HeadGestures, LoudSoundReduction, Ppe, SleepDetection, HearingAid, AdaptiveAudio, AdaptiveVolume, SwipeForVolume, Hrm }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum AirPodsModel { AirPods1, AirPods2, AirPods3, AirPods4, AirPods4Anc, AirPods5, AirPods5Wcc, AirPodsPro1, AirPodsPro2Lightning, AirPodsPro2UsbC, AirPodsPro3, AirPodsMaxLightning, AirPodsMaxUsbC, AirPodsMax2, Unknown }
  impl AirPodsModel {
      pub fn from_model_number(model_number: &str) -> Self;   // trims, case-insensitive
      pub fn from_ble_model_id(id: u16) -> Self;
      pub fn display_name(&self) -> &'static str;
      pub fn capabilities(&self) -> &'static [Capability];
      pub fn has(&self, capability: Capability) -> bool;
      pub fn is_headset(&self) -> bool;                      // Max models: single "headphone" battery
  }
  ```
- Model numbers and capabilities: use exactly the spec's **Models** table, including Max and Unknown.
  - `Unknown` returns all 12 capabilities.
  - `display_name` examples: "AirPods Pro (1st generation)", "AirPods Pro 2 (USB-C)", "AirPods Max".
- BLE ids: 0x0E20 Pro1, 0x1420 Pro2 Lightning, 0x2420 Pro2 USB-C, 0x0220 AirPods1, 0x0F20 AirPods2, 0x1320 AirPods3, 0x1920 AirPods4, 0x1B20 AirPods4Anc, 0x0A20 and 0x1F20 Max Lightning. Anything else is Unknown.

- [ ] **Step 1: Write the failing tests:**
  ```rust
  #[test] fn pro1_from_a2084_supports_only_listening_mode() // from_model_number("A2084") == AirPodsPro1; capabilities() == [ListeningMode]; !has(AdaptiveAudio)
  #[test] fn pro2_usbc_has_hearing_aid_and_adaptive() // "A3048" → AirPodsPro2UsbC; has(HearingAid) && has(AdaptiveAudio)
  #[test] fn pro3_adds_ppe_and_hrm() // "A3064" → AirPodsPro3; has(Ppe) && has(Hrm)
  #[test] fn max_is_headset() // "A2096" → AirPodsMaxLightning; is_headset()
  #[test] fn model_number_is_trimmed_and_case_insensitive() // " a2084 " → AirPodsPro1
  #[test] fn unknown_model_falls_back_permissively() // "A9999" → Unknown; capabilities().len() == 12
  #[test] fn ble_model_ids_map() // 0x0E20 → AirPodsPro1, 0x2420 → AirPodsPro2UsbC, 0xBEEF → Unknown
  ```
- [ ] **Step 2: Run and confirm they fail.** Run `ct test models::`. Expected: compile errors.
- [ ] **Step 3: Implement the table.** Use one `match` per mapping; no runtime data files.
- [ ] **Step 4: Run and confirm they pass.** `ct test models::` → 7 passed.
- [ ] **Step 5: Commit** `linux-rust: AirPods model and capability table`

### Task 2: Complete control-command catalog

**Files:**
- Modify: `src/bluetooth/aacp.rs:51-175` (`ControlCommandIdentifiers`, `from_u8`, `Display`)

**Interfaces:**
- Produces: every identifier from the spec's Control IDs list as a variant of `ControlCommandIdentifiers` (`#[repr(u8)]`, existing names kept). That covers 0x01–0x42, except IDs that have no known name.
- New names, using the rewrite's naming in CamelCase:
  - 0x02 Scan, 0x03 Reset, 0x04 BasicDoubleTapMode, 0x07 TapInterval, 0x08 BudRole, 0x09 DebugGetData
  - 0x0B JitterBuffer, 0x0C DoubleTapMode, 0x0E HeartRateMonitor1, 0x0F HeartRateMonitor2, 0x11 SwitchControl, 0x13 DoapMode
  - 0x22 HfpUplinkMode, 0x2F HpsGainSwipe (exists), 0x37 PpeToggleConfig, 0x38 PpeCapLevelConfig, 0x3A AllowTemporaryManagedPairing, 0x3B DynamicEndOfCharge
  - 0x3C SystemSiriMode, 0x3D HearingAidGeneric, 0x3E UplinkEqBud, 0x3F UplinkEqSource, 0x40 InCaseToneVolume, 0x41 DisableButtonInput, 0x42 ExtendedHoldAndRelease
- Add `pub const ALL: &'static [ControlCommandIdentifiers]` listing every variant.
- An unknown ID logs at `debug`, not `error`, and is dropped as before.

- [ ] **Step 1: Write the failing tests** (in a new `#[cfg(test)] mod tests` at the end of `aacp.rs`):
  ```rust
  #[test] fn every_identifier_round_trips() // for id in ControlCommandIdentifiers::ALL: from_u8(*id as u8) == Some(*id)
  #[test] fn android_ids_are_known() // from_u8(0x37) == Some(PpeToggleConfig), from_u8(0x3B) == Some(DynamicEndOfCharge), from_u8(0x08) == Some(BudRole)
  #[test] fn catalog_has_no_duplicate_bytes() // ALL mapped to u8, collected into a HashSet, same length as ALL
  ```
- [ ] **Step 2: Run and confirm they fail.** Run `ct test aacp::tests`. Expected: compile errors (`ALL` and the new variants are missing).
- [ ] **Step 3: Implement.** Add the variants, `from_u8` arms, `Display` names and `ALL`.
- [ ] **Step 4: Run and confirm they pass.** `ct test aacp::tests` → all pass (including Stage 1's `clearing_connection_state_forgets_ear_and_ownership`).
- [ ] **Step 5: Commit** `linux-rust: complete AACP control-command catalog`

### Task 3: Bud role, left/right ear mapping, capabilities packet, model on connect

**Files:**
- Modify: `src/bluetooth/aacp_parse.rs` (new parsers)
- Modify: `src/bluetooth/aacp.rs` (new opcodes `BUD_ROLE = 0x08` and `CAPABILITIES = 0x02`; state fields; receive arms; set the model on INFORMATION)

**Interfaces:**
- Produces:
  ```rust
  // crate::bluetooth::aacp_parse
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum BudRole { LeftPrimary, RightPrimary, Headset }
  pub fn parse_bud_role(payload: &[u8]) -> Option<BudRole>;                       // payload[2]: 01 left, 02 right, else headset
  pub fn parse_capabilities(payload: &[u8]) -> Option<Vec<(u8, Vec<u8>)>>;        // payload[2] = count, then {id, value}; value is 4 bytes for ids 0x03,0x04,0x06,0x07,0x30, else 1
  pub fn ear_left_right(statuses: [EarDetectionStatus; 2], role: Option<BudRole>) -> (EarDetectionStatus, EarDetectionStatus);
  // crate::bluetooth::aacp::AACPManagerState — new pub fields
  pub bud_role: Option<BudRole>,
  pub capabilities: Vec<(u8, Vec<u8>)>,
  pub model: Option<crate::devices::models::AirPodsModel>,
  ```
- `ear_left_right` mapping:
  - LeftPrimary or None → (primary, secondary);
  - RightPrimary → (secondary, primary);
  - Headset → (primary, primary).
- The INFORMATION arm sets `state.model = Some(AirPodsModel::from_model_number(&info.model_number))` and logs `info!("Detected {} ({}), capabilities: {:?}", display_name, model_number, capabilities)`.
- On disconnect, `clear_connection_state` also resets `bud_role`, `capabilities` and `model`.

- [ ] **Step 1: Write the failing tests** (in `aacp_parse.rs`):
  ```rust
  #[test] fn bud_role_values() // 08 00 01 → LeftPrimary; 08 00 02 → RightPrimary; 08 00 00 → Headset; 08 00 → None
  #[test] fn capabilities_mixed_widths() // 02 00 03 | 01 05 | 03 AA BB CC DD | 21 01 → [(0x01,[05]), (0x03,[AA,BB,CC,DD]), (0x21,[01])]
  #[test] fn capabilities_truncated_is_none() // 02 00 02 01 05 03 AA → None
  #[test] fn ear_mapping_follows_bud_role() // ([InEar, OutOfEar], Some(RightPrimary)) → (OutOfEar, InEar); (…, None) → (InEar, OutOfEar)
  ```
  Also extend `no_parser_panics_on_truncation` with the bud-role and capabilities fixtures. Add to `aacp.rs` tests:
  ```rust
  #[test] fn clearing_connection_state_forgets_model_and_role() // model Some(AirPodsPro1), bud_role Some(..), capabilities non-empty → all cleared
  ```
- [ ] **Step 2: Run and confirm they fail.** Run `ct test aacp`. Expected: compile errors.
- [ ] **Step 3: Implement the parsers, state fields, receive arms and model detection.** Logging rules for the new arms:
  - bud role: `info!`
  - capabilities: `info!` with ids only
  - Information: once per connect.
- [ ] **Step 4: Run and confirm they pass.** `ct test` → all pass.
- [ ] **Step 5: Commit** `linux-rust: parse bud role and capabilities, detect AirPods model on connect`

### Task 4: BLE proximity decoding (pure)

**Files:**
- Create: `src/bluetooth/proximity.rs`
- Modify: `src/bluetooth/mod.rs` (add `pub mod proximity;`)
- Modify: `src/bluetooth/le.rs` (delete its private `decrypt`/`verify_rpa`; use the new module)

**Interfaces:**
- Produces, in `crate::bluetooth::proximity` (`apple_data` = manufacturer data for company 0x004C, i.e. after the company ID):
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct Level { pub percent: u8, pub charging: bool }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum AccessoryState { Disconnected, Idle, Music, Call, Ringing, HangingUp, Other(u8) }
  #[derive(Debug, Clone, PartialEq, Eq)] pub struct BudsAdvert { pub model_id: u16, pub primary_left: bool, pub left_in_ear: bool, pub right_in_ear: bool, pub lid_open: bool, pub color: u8, pub state: AccessoryState, pub left: Option<Level>, pub right: Option<Level>, pub case: Option<Level> }
  #[derive(Debug, Clone, PartialEq, Eq)] pub struct CaseAdvert { pub case: Option<Level>, pub left: Option<Level>, pub right: Option<Level> }
  pub fn resolve_rpa(address: &str, irk: &[u8; 16]) -> bool;                      // false on malformed address
  pub fn parse_buds_advert(apple_data: &[u8], enc_key: &[u8; 16]) -> Option<BudsAdvert>;
  pub fn parse_case_advert(apple_data: &[u8], enc_keys: &[[u8; 16]]) -> Option<CaseAdvert>;
  ```
- Layouts are in the spec's **BLE** section.
- **Buds advert:**
  - It is only accepted if `apple_data[0] == 0x07` and the data is at least 27 bytes long.
  - Use the rewrite's in-ear XOR swap.
  - Battery index swap: when `!primary_left`, left is `dec[2]` and right is `dec[1]`.
  - A byte of `0xFF` means `None`.
  - `lid_open = (apple_data[8] >> 3) & 1 == 0 && (!left_in_ear || !right_in_ear) && any level charging`.
- **Case advert:** scan the TLV records for kind `0x07`, length ≥ 17 and payload starting `0x06`. Decrypt the 16 bytes that follow with each key and accept the first valid plaintext (`pt[1]==0x20`, `pt[15]==0`, and each of `pt[3..=5]` is 0xFF or ≤100 after `& 0x7F`). In the result, `[3]` is the case, `[4]` is left and `[5]` is right.

- [ ] **Step 1: Write the failing tests.**
  - Add a test helper `encrypt(key, block)` using `aes::Aes128` `encrypt_block`.
  - Build each fixture from a chosen plaintext.
  - Build the RPA with `crate::utils::ah`: `prand = [0x12, 0x34, 0x56 | 0x40]` and `hash = ah(irk, &prand)`. The address string is the bytes `prand.rev ++ hash.rev`, formatted `XX:XX:XX:XX:XX:XX`. The most significant byte comes first, so the address reads `prand[2]:prand[1]:prand[0]:hash[2]:hash[1]:hash[0]`.
  ```rust
  #[test] fn own_rpa_resolves() // resolve_rpa(addr, &IRK) == true
  #[test] fn foreign_rpa_does_not_resolve() // resolve_rpa(addr, &OTHER_IRK) == false
  #[test] fn malformed_address_is_false() // "zz:00:00:00:00:00", "", "8C:7A" → false
  #[test] fn buds_advert_primary_left() // status bit5 set; dec[1]=0x80|55, dec[2]=60, dec[3]=0xFF → left 55 charging, right 60, case None; state from byte 10 = 0x00 → Disconnected
  #[test] fn buds_advert_primary_right_swaps_batteries() // status bit5 clear; dec[1]=40, dec[2]=70 → left 70, right 40
  #[test] fn buds_advert_lid_open_needs_charging_bud() // lid bit3 = 0, a bud charging, buds not both in ear → lid_open; no charging → !lid_open
  #[test] fn case_advert_tries_each_key() // keys [WRONG, RIGHT]; plaintext pt[1]=0x20, pt[3]=0x80|90, pt[4]=50, pt[5]=0xFF, pt[15]=0 → case 90 charging, left 50, right None
  #[test] fn case_advert_rejects_invalid_plaintext() // only WRONG key → None
  #[test] fn no_decoder_panics_on_truncation() // every prefix of both fixtures through both parsers
  ```
- [ ] **Step 2: Run and confirm they fail.** Run `ct test proximity::`. Expected: compile errors.
- [ ] **Step 3: Implement.** Switch `le.rs` to `resolve_rpa` and `parse_buds_advert`. Keep `le.rs`'s existing tray updates for now; Task 5 restructures it.
- [ ] **Step 4: Run and confirm they pass.** `ct test proximity::` → 9 passed; `ct build` succeeds.
- [ ] **Step 5: Commit** `linux-rust: pure BLE proximity decoding (buds and case adverts, RPA resolution)`

### Task 5: BLE scanner that works without AdvertisementMonitor; safe auto-connect

**Files:**
- Modify: `src/bluetooth/le.rs` (rewrite of `start_le_monitor`)
- Modify: `src/main.rs` (shared `watch` channel for "AirPods AACP connected"; pass it to the LE task and flip it in `connect_airpods`/`disconnect_airpods`)

**Interfaces:**
- Consumes: Task 4 decoders, and Stage 1's `connect_airpods`/`disconnect_airpods`.
- Produces:
  ```rust
  // crate::bluetooth::le
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum ScanMode { Monitor, Discovery }
  pub fn should_scan(mode: ScanMode, aacp_connected: bool) -> bool;   // Monitor: always (passive); Discovery: only when !aacp_connected
  pub struct AutoConnectGate { /* HashMap<String, Instant> */ }
  impl AutoConnectGate {
      pub fn new(cooldown: Duration) -> Self;
      pub fn should_attempt(&mut self, mac: &str, enabled: bool, now: Instant) -> bool;
  }
  pub fn auto_connect_enabled(prefs_json: &str, mac: &str) -> bool;   // missing/invalid → false
  pub async fn start_le_monitor(tray_handle: Option<ksni::Handle<MyTray>>, aacp_connected: tokio::sync::watch::Receiver<bool>) -> bluer::Result<()>;
  ```
- `start_le_monitor` steps:
  1. Load the IRK/ENC keys from devices.json each time scanning (re)starts, so keys learned on a connect are used without restarting the app.
  2. Try `adapter.monitor().register(..)`. If that fails, log once at `info` ("BlueZ advertisement monitor unavailable; using LE discovery while AirPods are disconnected") and use Discovery.
  3. Discovery mode: `set_discovery_filter(DiscoveryFilter { transport: DiscoveryTransport::Le, duplicate_data: true, ..Default::default() })` plus `discover_devices_with_changes()`. Keep the stream only while `should_scan` is true. Drop it, which stops discovery, as soon as `aacp_connected` flips to true, and restart it when it flips back.
- Per matched advert:
  - Update the tray batteries the way it does today. Only do this when `!aacp_connected`, so AACP battery wins while connected.
  - If `state == Disconnected` and `gate.should_attempt(mac, auto_connect_enabled(..), now)`, run `bluetoothctl connect <mac>` with a 15 s timeout, without holding any lock.
  - Cooldown is 60 s.
  - A failed attempt is not retried until the cooldown passes.
  - An unmatched RPA is remembered as foreign for 10 minutes, not forever, because RPAs rotate.

- [ ] **Step 1: Write the failing tests:**
  ```rust
  #[test] fn scan_policy_never_scans_while_connected() // should_scan(Discovery, true) == false; should_scan(Discovery, false) == true; should_scan(Monitor, true) == true
  #[test] fn auto_connect_respects_cooldown() // gate 60 s: attempt at t0 → true; t0+10s → false; t0+61s → true; other MAC at t0+10s → true
  #[test] fn auto_connect_disabled_never_attempts() // should_attempt(mac, false, t0) == false
  #[test] fn auto_connect_off_without_pref() // auto_connect_enabled("{}", MAC) == false; ("not json", MAC) == false; (r#"{"MAC":{"autoConnect":true}}"#, MAC) == true
  ```
- [ ] **Step 2: Run and confirm they fail.** Run `ct test le::`. Expected: compile errors.
- [ ] **Step 3: Implement the policy pieces, the scanner rewrite and the main.rs wiring.** In `main.rs`, `connect_airpods` sends `true` after the registry registers a device, and `disconnect_airpods` sends `false` after shutdown.
- [ ] **Step 4: Run and confirm they pass.**
  - `ct test` → all pass.
  - `ct build` succeeds.
  - `grep -n "unwrap()" src/bluetooth/le.rs` → no matches on advert or address data.
- [ ] **Step 5: Commit** `linux-rust: LE discovery fallback when AdvertisementMonitor is missing; rate-limited opt-in auto-connect`

### Task 6: Publish AirPods battery to the desktop (BlueZ BatteryProvider)

**Files:**
- Create: `src/bluetooth/battery_provider.rs`
- Modify: `src/bluetooth/mod.rs`
- Modify: `Cargo.toml` (`zbus = { version = "5", default-features = false, features = ["tokio"] }`)
- Modify: `src/devices/airpods.rs` (report on BatteryInfo; remove on shutdown)
- Modify: `src/main.rs` (create one reporter at startup and pass it to `AirPodsDevice::new`)

**Interfaces:**
- Produces:
  ```rust
  // crate::bluetooth::battery_provider
  pub fn system_battery_percent(batteries: &[BatteryInfo]) -> Option<u8>;   // min over Left/Right with level>0 and status Charging/NotCharging/OptimizedCharging/Unknown; else Headphone; Case ignored; None if nothing usable
  #[derive(Clone)] pub struct BatteryReporter { /* zbus::Connection + adapter path */ }
  impl BatteryReporter {
      pub async fn register() -> Option<Self>;                       // None (logged) if BlueZ lacks BatteryProviderManager1
      pub async fn update(&self, mac: &str, percent: u8);
      pub async fn remove(&self, mac: &str);
  }
  ```
- `register` does the following:
  - connects to the system bus with zbus;
  - serves `org.freedesktop.DBus.ObjectManager` at `/me/kavishdevar/librepods/battery`;
  - calls `org.bluez.BatteryProviderManager1.RegisterBatteryProvider(that path)` on the adapter path found via `GetManagedObjects`.
- `update` serves (or refreshes) `org.bluez.BatteryProvider1` at `/me/kavishdevar/librepods/battery/dev_XX_XX_…` with three properties:
  - `Percentage: u8`
  - `Source: "LibrePods"`
  - `Device: ObjectPath` (`<adapter>/dev_XX_…`)

  It emits `PropertiesChanged` when the value changes.
- `remove` takes the object off the server, which emits `InterfacesRemoved`.

- [ ] **Step 1: Write the failing tests:**
  ```rust
  #[test] fn battery_is_lowest_bud() // R 51 NotCharging, L 49 NotCharging, Case 0 Disconnected → Some(49)
  #[test] fn disconnected_bud_is_ignored() // L 0 Disconnected, R 70 NotCharging → Some(70)
  #[test] fn headset_battery_used_for_max() // Headphone 80 Charging → Some(80)
  #[test] fn case_only_is_none() // Case 90 Charging → None
  #[test] fn unknown_status_bud_still_counts() // L 96 Unknown, R 97 NotCharging → Some(96)
  ```
- [ ] **Step 2: Run and confirm they fail.** Run `ct test battery_provider::`. Expected: compile errors.
- [ ] **Step 3: Implement.**
  - `system_battery_percent` and the zbus reporter.
  - Wiring: the AirPods battery event calls `reporter.update(mac, p)` when `Some(p)`; `AirPodsDevice::shutdown` calls `reporter.remove(mac)`.
  - If `register` returns `None`, everything else keeps working.
- [ ] **Step 4: Run and confirm they pass.** `ct test` → all pass; `ct build --release` succeeds.
- [ ] **Step 5: Commit** `linux-rust: publish AirPods battery to BlueZ so the desktop shows it`

### Task 7: Build alpha.3, check on hardware, push

**Files:** `Cargo.toml` (`version = "0.2.0-alpha.3"`)

- [ ] **Step 1: Gate.** `ct test` passes, and `ct clippy` shows no warnings in `devices/models.rs`, `bluetooth/proximity.rs` or `bluetooth/battery_provider.rs`.
- [ ] **Step 2: Build.** Build the AppImage as in Stage 1. Install it as `~/Applications/LibrePods-port-0.2.0-alpha.3-x86_64.AppImage` and repoint the `librepods-x86_64.AppImage` symlink. The user approved this arrangement on 2026-10-26.
- [ ] **Step 3: Restart the user's LibrePods with the new build.** Ask first if a game is running, or if the user is in a call: check `flatpak ps` and see whether `pactl list sink-inputs` shows a voice app. Then `systemctl --user restart app-librepods@autostart.service`.
- [ ] **Step 4: Passive hardware checks.** No user action is needed for these.
  - The journal shows `Detected AirPods Pro (1st generation) (A2084)` and the capabilities line.
  - `busctl introspect org.bluez /org/bluez/hci1/dev_8C_7A_AA_D0_3B_50` now lists `org.bluez.Battery1`, and its `Percentage` equals the lower bud level from the latest `Received Battery Info`.
  - The journal shows the "monitor unavailable; using LE discovery" line once, and no discovery while connected: `busctl get-property org.bluez /org/bluez/hci1 org.bluez.Adapter1 Discovering` returns `false`.
- [ ] **Step 5: Hardware check that needs the user.** Ask them to put the AirPods in the case for 20 s with the lid open. Expected:
  - the AACP disconnect is logged;
  - `Discovering` turns `true`;
  - the tray battery keeps updating from BLE, and the log shows a BLE-sourced battery line;
  - when they take the buds out, the AirPods reconnect on their own, auto-connect makes no attempt (it's off), and `Discovering` returns to `false`.
- [ ] **Step 6: Commit and push.** Commit the version bump, then `git push sondo linux-port`.
