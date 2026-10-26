# Stage 1: Stability Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the LibrePods Linux app reliable:
- one live device instance per AirPods;
- no WirePlumber restarts;
- no deadlocks;
- no panics on malformed packets;
- device data saved on the first connection.

**Architecture:**
- **Audio:** PulseAudio/BlueZ access moves behind a small `AudioBackend` trait with pure decision functions (`find_bluez_card`, `plan_a2dp_activation`). `MediaController` drives it, serialized by one lock.
- **Devices:** they are tracked by a `DeviceRegistry`. The registry owns each device's tasks and shuts them down on disconnect.
- **Parsing:** the AACP packet parsers that can panic move into pure functions in `aacp_parse.rs`.

**Tech Stack:** Rust 2024 (rustc 1.99), tokio, bluer 0.17, libpulse-binding 2.30, dbus 0.9, iced 0.14, ksni; tempfile (dev only).

**Spec:** `docs/superpowers/specs/2026-10-26-linux-port-design.md` (Roadmap item 1 and the Protocol reference)

## Global Constraints

**Build and test**
- Everything builds and tests inside distrobox `librepods-dev`. In this plan, `ct <args>` means:
  ```
  distrobox enter librepods-dev -- bash -c 'cd /var/home/Cold/librepods-linux-rust/linux-rust && cargo <args>'
  ```
- All paths below are relative to `/var/home/Cold/librepods-linux-rust/linux-rust/`.

**Audio safety**
- Never restart, stop or kill `wireplumber`, `pipewire` or `pipewire-pulse`. When this plan is done, the string `wireplumber` must not appear in `src/` (case-insensitive).
- Never change a PipeWire/Pulse card that isn't the AirPods card for the device's MAC.
- If the AirPods card already has an active profile whose name starts with `a2dp-sink`, leave it alone. That is the user's choice.
- A2DP recovery uses BlueZ `org.bluez.Device1.ConnectProfile("0000110b-0000-1000-8000-00805f9b34fb")`, attempted at most once per activation.

**Robustness**
- The release profile has `panic = "abort"`. New code must not `unwrap`/`expect`/index on data from the AirPods or the user's files.

**Style**
- Match the surrounding code: `log` macros (`info!`, `debug!`, `warn!`, `error!`), tokio primitives, and `#[cfg(test)] mod tests` inside the module under test, using `#[tokio::test]` for async.

**Version control**
- Git happens only in this worktree on branch `linux-port`. Never push.
- The commit steps run only if the user has approved committing. If they haven't, skip the commit steps and leave the changes staged.

**Hands off**
- Don't touch the user's running LibrePods service (`app-librepods@autostart.service`), `~/Applications/librepods-x86_64.AppImage`, or `~/.config/autostart/librepods.desktop` without asking the user in that moment.

## Review Focus

1. **Reconnect while the old instance is mid-operation.** The old instance must be fully shut down before the new one is created, so it can't set the card to "off" after the new one turns it on. Test: Task 6 `replacement_waits_for_old_shutdown`.
2. **pipewire-pulse unreachable or restarting** (empty card list). Activation tries BlueZ ConnectProfile once, polls, then gives up without setting anything or restarting anything. Test: Task 2 `activate_gives_up_without_restarting_anything`.
3. **Other cards present** (HDMI, built-in, a second pair of AirPods). Only the matching card is read or changed. Tests: Task 1 `ignores_other_cards_and_returns_none_when_absent`, `does_not_match_other_airpods`; Task 2 `deactivate_only_touches_airpods_card`.
4. **Truncated or malformed packets.** Parsers return `None` instead of panicking. Test: Task 4 `no_parser_panics_on_truncation`.
5. **The user already picked a different A2DP profile (e.g. AAC).** Activation leaves it as is. Tests: Task 1 `active_a2dp_profile_is_left_alone`; Task 2 `activate_does_nothing_when_a2dp_already_active`.

---

### Task 1: `audio.rs`: card snapshots, card matching, activation planning, real Pulse results

**Files:**
- Create: `src/audio.rs`
- Modify: `src/main.rs:1-5` (add `mod audio;`)
- Modify: `src/media_controller.rs` (move the Pulse helpers out)
- Modify: `Cargo.toml` (add `[dev-dependencies] tempfile = "3"`, used in Task 5)

**Interfaces:**
- Produces, in `crate::audio`:
  ```rust
  #[derive(Debug, Clone, PartialEq)] pub struct ProfileSnapshot { pub name: String, pub available: bool }
  #[derive(Debug, Clone, PartialEq)] pub struct CardSnapshot { pub index: u32, pub name: String, pub device_string: Option<String>, pub active_profile: Option<String>, pub profiles: Vec<ProfileSnapshot> }
  pub const PREFERRED_A2DP_PROFILES: [&str; 3] = ["a2dp-sink-sbc_xq", "a2dp-sink-sbc", "a2dp-sink"];
  pub const A2DP_SINK_UUID: &str = "0000110b-0000-1000-8000-00805f9b34fb";
  #[derive(Debug, Clone, PartialEq, Eq)] pub enum A2dpAction { AlreadyActive, SetProfile(String), ConnectProfile, NoCard }
  pub fn find_bluez_card<'a>(cards: &'a [CardSnapshot], mac: &str) -> Option<&'a CardSnapshot>;
  pub fn plan_a2dp_activation(card: Option<&CardSnapshot>) -> A2dpAction;
  pub trait AudioBackend: Send + Sync + 'static {
      fn cards(&self) -> Vec<CardSnapshot>;
      fn set_card_profile(&self, card_index: u32, profile: &str) -> bool;
      fn connect_a2dp(&self, mac: &str) -> bool;
  }
  pub struct SystemAudioBackend;   // impl AudioBackend via libpulse + system D-Bus
  // moved from media_controller.rs, signatures unchanged:
  pub fn get_sink_volume_percent_by_name_sync(sink_name: &str) -> Option<u32>;
  pub fn transition_sink_volume(sink_name: &str, target_volume: u32) -> bool;
  pub async fn get_sink_name_by_mac(mac: &str) -> Option<String>;
  ```

- [ ] **Step 1: Write the failing tests** in `src/audio.rs` `mod tests`. Use a helper `card(index, name, device_string, active, &[(profile, available)])`.
  ```rust
  const MAC: &str = "8C:7A:AA:D0:3B:50";
  #[test] fn finds_card_by_name() // cards: alsa_card.pci-0000_01_00.1 (925), alsa_card.pci-0000_00_1f.3 (926), bluez_card.8C_7A_AA_D0_3B_50 (1043) → Some(index 1043)
  #[test] fn finds_card_by_device_string_lowercase() // name "bluez_card.renamed", device_string Some("8c:7a:aa:d0:3b:50") → Some
  #[test] fn ignores_other_cards_and_returns_none_when_absent() // only the two alsa cards → None
  #[test] fn does_not_match_other_airpods() // bluez_card.11_22_33_44_55_66 with device_string "11:22:33:44:55:66" → None
  #[test] fn no_card_means_no_card() // plan_a2dp_activation(None) == A2dpAction::NoCard
  #[test] fn active_a2dp_profile_is_left_alone() // active Some("a2dp-sink") (AAC), sbc_xq available → AlreadyActive
  #[test] fn picks_first_available_preferred_profile() // active "off"; a2dp-sink, a2dp-sink-sbc, a2dp-sink-sbc_xq all available → SetProfile("a2dp-sink-sbc_xq")
  #[test] fn skips_unavailable_profiles() // sbc_xq available=false, sbc available=true → SetProfile("a2dp-sink-sbc")
  #[test] fn connect_profile_when_no_a2dp_available() // active "off", profiles [("off", true), ("a2dp-sink", false)] → ConnectProfile
  ```

- [ ] **Step 2: Run the tests and confirm they fail.** Run `ct test audio::`. Expected: compile errors (module/functions missing).

- [ ] **Step 3: Implement the pure functions and move the Pulse helpers.**
  - `find_bluez_card` matches a card when either is true, both case-insensitive:
    - `name == "bluez_card." + mac.replace(':', "_")`
    - `device_string == mac`
  - `plan_a2dp_activation` returns, in this order:
    1. `NoCard` if there is no card;
    2. `AlreadyActive` if the active profile starts with `a2dp-sink`;
    3. the first `PREFERRED_A2DP_PROFILES` entry that is present with `available == true`;
    4. otherwise `ConnectProfile`.
  - Move these from `media_controller.rs` into `audio.rs`: `pulse_connect`, the card-list query (now returning `Vec<CardSnapshot>`, filled from `CardInfo.name`, the proplist `device.string`, `active_profile.name`, and `profiles[].{name, available}`), and the three sink helpers.
  - In all moved Pulse loops, use `mainloop.iterate(true)` instead of the busy-spinning `iterate(false)`.
  - `SystemAudioBackend::set_card_profile` must return the real result from `set_card_profile_by_index`'s success callback, not a hard-coded `true`.
  - `SystemAudioBackend::connect_a2dp` calls `org.bluez.Device1.ConnectProfile(A2DP_SINK_UUID)` with a blocking `dbus::blocking::Connection::new_system()`:
    - object path `/org/bluez/hci0/dev_` + `mac.replace(':', "_")`;
    - the adapter name comes from the first `org.bluez.Adapter1` in `GetManagedObjects`, not a hard-coded `hci0`;
    - 10 s timeout.
  - Point `media_controller.rs`'s callers at the moved helpers (`crate::audio::...`).

- [ ] **Step 4: Run the tests and confirm they pass.** Run `ct test audio::`. Expected: 9 passed. Then run `ct build`; expected to succeed.

- [ ] **Step 5: Commit** (only if the user approved commits)
  ```bash
  git add linux-rust/src/audio.rs linux-rust/src/main.rs linux-rust/src/media_controller.rs linux-rust/Cargo.toml linux-rust/Cargo.lock
  git commit -m "linux-rust: add audio backend with MAC-matched card lookup and A2DP planning"
  ```

### Task 2: MediaController activates A2DP through the backend; no WirePlumber restarts; one operation at a time

**Files:**
- Modify: `src/media_controller.rs` (`MediaControllerState`, `MediaController::new`, `activate_a2dp_profile`, `deactivate_a2dp_profile`; delete `restart_wire_plumber`, `is_a2dp_profile_available`, `get_preferred_a2dp_profile`, `is_profile_available`, `get_audio_device_index`, and the `device_index`/`cached_a2dp_profile` fields)
- Modify: `src/devices/airpods.rs:121-124` (pass a backend)

**Interfaces:**
- Consumes: Task 1's `AudioBackend`, `find_bluez_card`, `plan_a2dp_activation`, `A2dpAction`.
- Produces:
  ```rust
  impl MediaController {
      pub fn new(connected_mac: String, local_mac: String, backend: Arc<dyn AudioBackend>) -> Self;
      pub fn with_a2dp_wait(self, attempts: u32, interval: Duration) -> Self; // defaults: 10 attempts, 500 ms
      pub async fn activate_a2dp_profile(&self);
      pub async fn deactivate_a2dp_profile(&self);
  }
  ```

- [ ] **Step 1: Write the failing tests.**
  - Add `FakeBackend` to the `media_controller.rs` tests module. It holds:
    - `cards: Mutex<Vec<CardSnapshot>>`
    - `cards_after_connect: Option<Vec<CardSnapshot>>`
    - `set_calls: Mutex<Vec<(u32, String)>>`
    - `connect_calls: AtomicUsize`
  - Its behaviour:
    - `set_card_profile` records the call and sets that card's `active_profile`.
    - `connect_a2dp` increments the counter and swaps in `cards_after_connect` if present.
  - Every test builds its controller with `.with_a2dp_wait(3, Duration::from_millis(1))`.
  ```rust
  #[tokio::test] async fn activate_sets_preferred_profile_once() // card 1043 active "off", sbc_xq available → set_calls == [(1043, "a2dp-sink-sbc_xq")], connect_calls == 0
  #[tokio::test] async fn activate_does_nothing_when_a2dp_already_active() // active "a2dp-sink" → set_calls empty, connect_calls == 0
  #[tokio::test] async fn activate_connects_profile_via_bluez_when_missing_then_sets_profile() // cards [] then after connect: card 1043 active "off" with sbc_xq available → connect_calls == 1, set_calls == [(1043, "a2dp-sink-sbc_xq")]
  #[tokio::test] async fn activate_gives_up_without_restarting_anything() // cards always [] → connect_calls == 1, set_calls empty, returns (does not hang)
  #[tokio::test] async fn concurrent_activations_set_profile_once() // tokio::join! three activate_a2dp_profile() calls on clones → set_calls.len() == 1
  #[tokio::test] async fn deactivate_only_touches_airpods_card() // cards: alsa_card.pci-0000_01_00.1 (940, active "output:hdmi-stereo") + bluez 1043 (active sbc_xq) → set_calls == [(1043, "off")]
  #[tokio::test] async fn deactivate_without_card_does_nothing() // cards [] → set_calls empty, connect_calls == 0
  ```

- [ ] **Step 2: Run the tests and confirm they fail.** Run `ct test media_controller::`. Expected: compile errors (new constructor/backend missing).

- [ ] **Step 3: Implement.**
  - Add two fields to `MediaController`, both `Clone`: `backend: Arc<dyn AudioBackend>` and `audio_op: Arc<tokio::sync::Mutex<()>>`.
  - `activate_a2dp_profile`:
    1. Hold `audio_op` for the whole call.
    2. Each round: read the cards with `spawn_blocking`, then `plan_a2dp_activation(find_bluez_card(..))`.
    3. On `AlreadyActive`, return.
    4. On `SetProfile(p)`, call `set_card_profile` and log `info!` on success or `warn!` on failure. Return.
    5. On `ConnectProfile` or `NoCard`:
       - first time: call `connect_a2dp` once, then poll again straight away;
       - after that: sleep `interval` before each re-poll, up to `attempts` polls, then `warn!` and return.
  - `deactivate_a2dp_profile`: hold `audio_op`, find the card fresh, and call `set_card_profile(index, "off")` only if the card exists and its active profile isn't `"off"`.
  - Never hold `self.state` while awaiting anything that locks `self.state` again.
  - In `airpods.rs`, construct with `Arc::new(SystemAudioBackend)`.

- [ ] **Step 4: Run the tests and confirm they pass.**
  - `ct test media_controller::` → 7 passed.
  - `grep -rin wireplumber src/` → no output.
  - `ct build` succeeds.

- [ ] **Step 5: Commit** (only if the user approved commits)
  ```bash
  git add linux-rust/src/media_controller.rs linux-rust/src/devices/airpods.rs
  git commit -m "linux-rust: activate A2DP via backend and BlueZ ConnectProfile, never restart WirePlumber"
  ```

### Task 3: Playback listener without deadlocks; takeover decision as a pure function

**Files:**
- Modify: `src/media_controller.rs:94-182` (listener)
- Modify: `src/devices/airpods.rs` (MediaController no longer wrapped in `Arc<Mutex<…>>`)
- Modify: `src/bluetooth/aacp.rs:1205-1233` (`recv_thread` clears ear state on disconnect)

**Interfaces:**
- Consumes: Task 2's `MediaController` (it is `Clone`, and all its methods take `&self`).
- Produces:
  ```rust
  pub fn should_take_over(was_playing: bool, is_playing: bool, ear: &[EarDetectionStatus]) -> bool;
  impl MediaController { pub async fn run_playback_listener(self, aacp_manager: AACPManager, control_tx: UnboundedSender<(ControlCommandIdentifiers, Vec<u8>)>); }
  // AirPodsDevice.media_controller: MediaController   (was Arc<Mutex<MediaController>>)
  ```
  `start_playback_listener` and `playback_listener_running` are removed.

- [ ] **Step 1: Write the failing tests** in `media_controller.rs` tests:
  ```rust
  #[test] fn takes_over_on_rising_edge_with_a_bud_in_ear() // (false, true, [InEar, OutOfEar]) → true
  #[test] fn no_takeover_when_buds_out() // (false, true, [OutOfEar, InCase]) → false
  #[test] fn no_takeover_when_already_playing() // (true, true, [InEar, InEar]) → false
  #[test] fn no_takeover_without_ear_data() // (false, true, []) → false
  ```

- [ ] **Step 2: Run the tests and confirm they fail.** Run `ct test -- takes_over no_takeover`. Expected: compile error, `should_take_over` not found.

- [ ] **Step 3: Implement.**
  - In `run_playback_listener`, take the AACP state lock only long enough to clone `ear_detection_status` and `connected_devices`, then drop the lock. Only after that call `should_take_over`, send `OwnsConnection=01`, activate A2DP, and send the smart-routing packets.
  - In `airpods.rs`:
    - hold `MediaController` directly;
    - start the listener with `tokio::spawn(media_controller.clone().run_playback_listener(aacp.clone(), command_tx.clone()))` (Task 6 moves it into the device's JoinSet);
    - every `mc_clone.lock().await` becomes a plain clone.
  - In `recv_thread`'s disconnect branch, also clear `ear_detection_status` and `old_ear_detection_status`, and set `audio_source = None`.

- [ ] **Step 4: Run the tests and confirm they pass.** `ct test` → all tests pass (Tasks 1–3); `ct build` succeeds.

- [ ] **Step 5: Commit** (only if the user approved commits)
  ```bash
  git add linux-rust/src/media_controller.rs linux-rust/src/devices/airpods.rs linux-rust/src/bluetooth/aacp.rs
  git commit -m "linux-rust: fix playback-listener deadlocks and stale ear state after disconnect"
  ```

### Task 4: Panic-safe AACP parsers and UI sends

**Files:**
- Create: `src/bluetooth/aacp_parse.rs`
- Modify: `src/bluetooth/mod.rs` (add `pub mod aacp_parse;`)
- Modify: `src/bluetooth/aacp.rs:242-255` (`BatteryStatus` gets `Unknown = 0x00` and `OptimizedCharging = 0x05`), plus the receive arms at `:514-559`, `:612-654`, `:671-734`, `:859-916`, which call the new parsers
- Modify: `src/ui/airpods.rs:69,123,231,277,334`

**Interfaces:**
- Produces, in `crate::bluetooth::aacp_parse` (all take `payload = packet[4..]`):
  ```rust
  pub fn parse_battery(payload: &[u8]) -> Option<Vec<BatteryInfo>>;
  pub fn parse_ear_detection(payload: &[u8]) -> Option<[EarDetectionStatus; 2]>;
  pub fn parse_information(payload: &[u8]) -> Option<AirPodsInformation>;
  pub fn parse_connected_devices(payload: &[u8]) -> Option<Vec<ConnectedDevice>>;
  pub fn parse_smart_routing_response(payload: &[u8]) -> Option<String>;
  ```
- Parser rules:
  - **Battery:** skip entries with an unknown component. Map an unknown status byte (including 0x03) to `BatteryStatus::Unknown` and keep the level.
  - **Connected devices:** the count is at `payload[4]` and entries start at `payload[5 + i*8]`, with the MAC not reversed.
  - **Information:** keep the current token rule: skip the run before the first NUL, collect the NUL-separated strings, drop the first one collected. Return `None` if nothing is left.

- [ ] **Step 1: Write the failing tests** in `aacp_parse.rs` tests (fixtures are hex payloads):
  ```rust
  #[test] fn battery_keeps_unknown_status_entries() // 04 00 03 02 01 61 02 01 04 01 60 03 01 08 01 00 04 01 → [Right 97 NotCharging, Left 96 Unknown, Case 0 Disconnected]
  #[test] fn battery_headset_only() // 04 00 01 01 01 50 01 01 → [Headphone 80 Charging]
  #[test] fn ear_detection_two_statuses() // 06 00 00 01 → [InEar, OutOfEar]
  #[test] fn ear_detection_too_short() // 06 00 02 → None
  #[test] fn information_fields() // 1D 00 AA BB + "X\0Y\0Logan's AirPods Pro\0A2084\0Apple Inc.\0SER\0v1\0v2\01.0.0\0upd\0LS\0RS\0v3\0" → name "Logan's AirPods Pro", model_number "A2084", version3 "v3"
  #[test] fn information_without_strings_is_none() // 1D 00 00 00 00 00 → None
  #[test] fn connected_devices_count_at_payload_4() // 2E 00 00 00 01 B0 60 88 AF DD DA 02 03 → [mac "B0:60:88:AF:DD:DA", info1 2, info2 3]
  #[test] fn connected_devices_short_entry_is_none() // 2E 00 00 00 02 B0 60 88 AF DD DA 02 03 → None
  #[test] fn smart_routing_response_text() // 11 00 + b"..SetOwnershipToFalse.." → Some(s) where s.contains("SetOwnershipToFalse")
  #[test] fn no_parser_panics_on_truncation() // for each fixture above, for n in 0..=len: call its parser on &fixture[..n]
  ```

- [ ] **Step 2: Run the tests and confirm they fail.** Run `ct test aacp_parse::`. Expected: compile errors.

- [ ] **Step 3: Implement the parsers and switch the `receive_packet` arms to them.** On `None`, a parser arm logs `error!` with the hex and returns. In `ui/airpods.rs`, replace each `.expect("Failed to send …")` with `if let Err(e) = … { log::error!("Failed to send …: {}", e); }`.

- [ ] **Step 4: Run the tests and confirm they pass.**
  - `ct test aacp_parse::` → 10 passed.
  - `grep -n 'expect("Failed to send' src/ui/airpods.rs` → no output.
  - `ct build` succeeds.

- [ ] **Step 5: Commit** (only if the user approved commits)
  ```bash
  git add linux-rust/src/bluetooth/aacp_parse.rs linux-rust/src/bluetooth/mod.rs linux-rust/src/bluetooth/aacp.rs linux-rust/src/ui/airpods.rs
  git commit -m "linux-rust: panic-safe AACP parsers, keep unknown battery status, fix connected-devices count"
  ```

### Task 5: Safe devices.json updates that work on the first connection

**Files:**
- Modify: `src/utils.rs` (new functions)
- Modify: `src/devices/airpods.rs:400-414` (derive `Default` on `AirPodsInformation`)
- Modify: `src/bluetooth/aacp.rs` (derive `Default` on `AirPodsLEKeys`; the INFORMATION and PROXIMITY_KEYS_RSP arms persist through `update_devices_file`; remove the `devices` snapshot field from `AACPManagerState`)

**Interfaces:**
- Produces, in `crate::utils`:
  ```rust
  pub fn update_devices_file(path: &Path, mac: &str, update: impl FnOnce(&mut DeviceData)) -> std::io::Result<()>;
  pub fn apply_information(data: &mut DeviceData, info: AirPodsInformation);   // keeps existing le_keys
  pub fn apply_le_key(data: &mut DeviceData, key_type: ProximityKeyType, key_hex: &str); // creates AirPods info if absent
  ```
- `update_devices_file` behaviour:
  - It reads the current file each call; a missing file means `{}`.
  - If the file isn't valid JSON, rename it to `devices.json.bak` and start from `{}`.
  - A missing entry gets inserted as `DeviceData { name: mac, type_: DeviceType::AirPods, information: None }`.
  - Then `update` is applied, the result is written to `<path>.tmp`, and that file is renamed over `path`.

- [ ] **Step 1: Write the failing tests** in `utils.rs` tests (use `tempfile::tempdir()`):
  ```rust
  #[test] fn creates_entry_when_file_missing() // update "8C:7A:AA:D0:3B:50" → file has that key with type_ AirPods
  #[test] fn preserves_other_devices() // file has a Nothing device "AA:BB:CC:DD:EE:FF"; update AirPods entry → both present, Nothing entry unchanged
  #[test] fn corrupt_file_is_backed_up() // file "{not json" → Ok(()), devices.json.bak exists with the old bytes, new file parses with 1 entry
  #[test] fn apply_le_key_creates_information_when_missing() // DeviceData{information: None} + Irk "00ff" → AirPods info with le_keys.irk == "00ff"
  #[test] fn apply_information_keeps_existing_keys() // existing irk "aa" enc "bb"; apply info with empty keys → irk "aa", enc_key "bb", model_number updated
  ```

- [ ] **Step 2: Run the tests and confirm they fail.** Run `ct test utils::`. Expected: compile errors.

- [ ] **Step 3: Implement the three functions and switch both AACP arms to them.** The AACP arms call `update_devices_file` inside `tokio::task::spawn_blocking`, and log `error!` on `Err`.

- [ ] **Step 4: Run the tests and confirm they pass.** `ct test utils::` → 5 passed; `ct build` succeeds.

- [ ] **Step 5: Commit** (only if the user approved commits)
  ```bash
  git add linux-rust/src/utils.rs linux-rust/src/devices/airpods.rs linux-rust/src/bluetooth/aacp.rs
  git commit -m "linux-rust: persist device info and LE keys on first connect without clobbering devices.json"
  ```

### Task 6: Device lifecycle: one live instance per MAC, torn down on disconnect

**Files:**
- Create: `src/devices/registry.rs`
- Modify: `src/devices/mod.rs` (add `pub mod registry;`)
- Modify: `src/bluetooth/aacp.rs:360-439` (alive flag, `connect` returns `bool`, add `disconnect`), plus `recv_thread` (clears the alive flag on exit)
- Modify: `src/devices/airpods.rs` (`new` returns `Option<Self>`; every spawned loop goes into a `JoinSet` the device owns; `impl ManagedDevice`)
- Modify: `src/main.rs:164-330` (initial scan and the D-Bus handler go through the registry; on `Connected=0`, disconnect and reset the tray)

**Interfaces:**
- Consumes: Task 3's `MediaController::run_playback_listener`.
- Produces:
  ```rust
  // crate::devices::registry
  pub trait ManagedDevice: Send + 'static {
      fn is_alive(&self) -> bool;
      fn shutdown(self) -> impl std::future::Future<Output = ()> + Send;
  }
  #[derive(Clone)] pub struct DeviceRegistry<D: ManagedDevice> { /* Arc<tokio::sync::Mutex<HashMap<String, Slot<D>>>> where enum Slot<D> { Connecting, Live(D) } */ }
  impl<D: ManagedDevice> DeviceRegistry<D> {
      pub fn new() -> Self;
      /// false if a live or connecting device already exists for `mac`
      pub async fn connect_with<F, Fut>(&self, mac: &str, factory: F) -> bool
          where F: FnOnce() -> Fut + Send, Fut: std::future::Future<Output = Option<D>> + Send;
      pub async fn disconnect(&self, mac: &str) -> bool;
      pub async fn is_live(&self, mac: &str) -> bool;
  }
  // crate::bluetooth::aacp
  impl AACPManager {
      pub async fn connect(&mut self, addr: Address) -> bool;
      pub fn is_connected(&self) -> bool;
      pub async fn disconnect(&self);
  }
  // crate::devices::airpods
  impl AirPodsDevice { pub async fn new(mac_address: Address, tray_handle: Option<Handle<MyTray>>, ui_tx: UnboundedSender<BluetoothUIMessage>) -> Option<Self>; }
  impl ManagedDevice for AirPodsDevice { … }
  ```
- `connect_with` semantics:
  - If a dead `Live` slot exists, remove it and **await its `shutdown()` before** calling `factory`.
  - Insert `Connecting` while `factory` runs, then:
    - `None` → remove the slot;
    - `Some(d)` while the slot is still `Connecting` → store `Live(d)`;
    - `Some(d)` after the slot was removed by a disconnect → `d.shutdown().await`.
- `AACPManager` internals:
  - `is_connected` reads an `Arc<AtomicBool>`, set after the L2CAP connection is established and cleared when `recv_thread` exits.
  - `disconnect` aborts the manager's JoinSet, sets `sender = None` and clears the flag.
- `AirPodsDevice::shutdown`:
  1. logs `info!("Shutting down AirPodsDevice for {}", mac)`;
  2. aborts and drains its JoinSet;
  3. awaits `aacp_manager.disconnect()`.

- [ ] **Step 1: Write the failing tests** in `registry.rs` tests. `FakeDevice` holds `alive: Arc<AtomicBool>`, `shutdowns: Arc<AtomicUsize>` and `shutdown_delay: Duration`. Its `shutdown` sleeps `shutdown_delay`, increments `shutdowns`, and sets a shared `old_shutdown_done` flag.
  ```rust
  #[tokio::test] async fn second_connect_for_live_device_is_ignored() // factory calls == 1, second connect_with returns false
  #[tokio::test] async fn concurrent_connects_create_one_device() // factory sleeps 50 ms; join two connect_with → factory calls == 1
  #[tokio::test] async fn dead_device_is_shut_down_and_replaced() // alive=false → second connect_with returns true, shutdowns == 1, factory calls == 2
  #[tokio::test] async fn replacement_waits_for_old_shutdown() // old dead device with shutdown_delay 50 ms; second factory asserts old_shutdown_done is true when called
  #[tokio::test] async fn disconnect_shuts_down_live_device() // disconnect → true, shutdowns == 1, is_live false
  #[tokio::test] async fn disconnect_while_connecting_shuts_down_the_new_device() // factory sleeps 100 ms; disconnect at 20 ms; after connect_with returns: is_live false, shutdowns == 1
  #[tokio::test] async fn failed_factory_leaves_no_slot() // factory returns None → is_live false; next connect_with calls factory again
  ```

- [ ] **Step 2: Run the tests and confirm they fail.** Run `ct test registry::`. Expected: compile errors.

- [ ] **Step 3: Implement the registry, then wire it into AACPManager, AirPodsDevice and main.**
  - In `main.rs`, create `let airpods: DeviceRegistry<AirPodsDevice> = DeviceRegistry::new();`.
  - **Startup scan and `Connected=1` signals** (with the AirPods UUID): spawn `airpods.connect_with(&addr_str, || async move { … })`. The factory awaits `AirPodsDevice::new`. On `Some`, before returning the device, it:
    - registers `device_managers` with `set_aacp`;
    - sends `BluetoothUIMessage::DeviceConnected`.
  - **`Connected=0` signals:** spawn `airpods.disconnect(&addr_str)`, then update the tray: `connected = false`, every `battery_*` and `battery_*_status` field `None`, `listening_mode = None`. Keep the existing `DeviceDisconnected` UI message.
  - Replace `devices_list.get(&addr_str).unwrap()` at `main.rs:204` and `:284` with a `let Some(..) else` that logs and skips.

- [ ] **Step 4: Run the tests and confirm they pass.** `ct test` → all tests from Tasks 1–6 pass; `ct build --release` succeeds.

- [ ] **Step 5: Commit** (only if the user approved commits)
  ```bash
  git add linux-rust/src/devices/registry.rs linux-rust/src/devices/mod.rs linux-rust/src/bluetooth/aacp.rs linux-rust/src/devices/airpods.rs linux-rust/src/main.rs
  git commit -m "linux-rust: one AirPods instance per MAC with teardown on disconnect"
  ```

### Task 7: Build the AppImage and verify on the user's AirPods

**Files:**
- Modify: `Cargo.toml` (`version = "0.2.0-alpha.1"`)
- Output: `dist/LibrePods-x86_64.AppImage`, copied to `~/Applications/LibrePods-port-0.2.0-alpha.1-x86_64.AppImage` (a new file; the old AppImage stays untouched)

**Interfaces:** consumes everything from Tasks 1–6.

- [ ] **Step 1: Gate on tests and lint.**
  - `ct test` → all pass.
  - `ct clippy` → no warnings in `src/audio.rs`, `src/bluetooth/aacp_parse.rs` or `src/devices/registry.rs`.

- [ ] **Step 2: Get the packaging tools.**
  - Download `linuxdeploy-x86_64.AppImage` from `https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/` and `appimagetool-x86_64.AppImage` from `https://github.com/AppImage/appimagetool/releases/download/continuous/` into `~/.local/opt/appimage-tools/`, then `chmod +x` both.
  - Symlink them as `linuxdeploy` and `appimagetool` in `~/.local/opt/appimage-tools/bin/`.

- [ ] **Step 3: Build.** Run:
  ```
  distrobox enter librepods-dev -- bash -c 'cd /var/home/Cold/librepods-linux-rust/linux-rust && PATH=$HOME/.local/opt/appimage-tools/bin:$PATH APPIMAGE_EXTRACT_AND_RUN=1 just build-appimage'
  ```
  Expected: `Done!` and `dist/LibrePods-x86_64.AppImage` exists. Then copy it to `~/Applications/LibrePods-port-0.2.0-alpha.1-x86_64.AppImage` and `chmod +x` it.

- [ ] **Step 4: Hardware check. Ask the user first.**
  1. Ask permission to stop `app-librepods@autostart.service` for the test, and to have the user move their AirPods in and out of the case. Only one LibrePods may hold the AACP channel at a time.
  2. Run:
     ```
     systemd-run --user --unit=librepods-port-test ~/Applications/LibrePods-port-0.2.0-alpha.1-x86_64.AppImage --debug --start-minimized
     ```
  3. Have the user do the following:
     - case → out, three times;
     - start playback in a browser;
     - take one bud out, then put it back.
  4. Check the logs with `journalctl --user -u librepods-port-test`. Expected:
     - the number of `Creating new AirPodsDevice` lines equals the number of `Shutting down AirPodsDevice` lines + 1;
     - `wireplumber` appears 0 times (case-insensitive);
     - after playback starts, there is an `Activating A2DP`/`set` line or an already-active line;
     - no panic and no abort.
  5. `wpctl status` shows the AirPods sink, and `pactl list cards short` still lists the HDMI and built-in cards with the same profiles as before the test.

- [ ] **Step 5: Hand back to the user.**
  - Report what you saw.
  - Ask whether to keep the test build running, or to stop `librepods-port-test` and restart `app-librepods@autostart.service`.
  - Don't change autostart in this plan.

- [ ] **Step 6: Commit** (only if the user approved commits)
  ```bash
  git add linux-rust/Cargo.toml linux-rust/Cargo.lock
  git commit -m "linux-rust: 0.2.0-alpha.1 stability build"
  ```
