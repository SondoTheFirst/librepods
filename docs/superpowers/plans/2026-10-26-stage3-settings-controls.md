# Stage 3: Settings and Controls Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stage 3 delivers four things:
- **AirPods page.** The page is rebuilt from a per-model control catalog, so each pair of AirPods shows exactly the controls it supports. That covers every control command with a known encoding, plus custom EQ, headphone accommodation and rename.
- **Behaviour preferences.** Ear detection, conversation-awareness ducking, takeover and stem-press actions become per-device preferences instead of hard-coded behaviour.
- **Quick controls.** The tray gains quick controls that follow the model, and the app can post desktop notifications.
- **CLI.** A running LibrePods can be controlled from a terminal with `librepods ctl …`, through a session D-Bus interface.

**Architecture:**
- **New pure modules:**
  - `prefs.rs`: per-device preferences and app settings, with atomic JSON files.
  - `devices/settings.rs`: the control catalog, listening-mode rules and value codecs.
  - `devices/commands.rs`: stem actions, listening-mode cycling, and the stored configuration re-applied on connect.
- **The UI keeps a generic map of control values.** It mirrors the AACP control-command list and renders rows from the catalog. Commands go out through `Task::perform` on the device's `AACPManager`; the per-click thread and runtime (`run_async_in_thread`) is removed.
- **New D-Bus pieces:**
  - `ui/notifications.rs`: a zbus client for `org.freedesktop.Notifications`.
  - `control.rs`: the zbus service `me.kavishdevar.librepods.Control1`, plus the `ctl` client subcommand.

**Tech Stack:** Rust 2024, tokio, iced 0.14 (`pick_list`, `slider` with `on_release`, `checkbox`, `text_input` with `on_submit`), ksni 0.3, zbus 5 (already a direct dependency), serde_json, clap 4 (subcommands).

**Spec:** `docs/superpowers/specs/2026-10-26-linux-port-design.md`. Relevant parts:
- Roadmap item 3;
- Decisions 3 (capability gating) and 5 (Android UI → Linux);
- Protocol reference: Control IDs, 0x1A rename, 0x53, 0x63, Smart routing / ownership, and "Android bugs to not copy".

## Decisions made in this plan (Claude's calls; the user can overrule)

1. **Off listening mode.**
   - Models without Adaptive Audio (Pro 1, AirPods Max) always offer **Off**; it is a built-in mode on those models.
   - Models with Adaptive Audio offer Off only while control 0x34 reports on.
   - Both Android versions hide Off on Pro 1. Task 10 verifies on the user's A2084 that the buds echo `0D 01`.
2. **Behaviour defaults equal today's behaviour.** Defaults are:
   - ducking to 25 % / 15 %, as an absolute cap;
   - takeover on media start;
   - taking over from another device's music.

   The one change: taking over **during another device's call** is off by default. Today the app hijacks even then.
3. **Stem actions.**
   - The stem-action section is shown when the model has `StemConfig`, or when actions are already configured.
   - The global "Stem press track control" toggle becomes per-device actions: double = next, triple = previous, on both buds. That is the same 0x39 mask (0x06) the old toggle sent.
4. **Headphone accommodation (0x53)** is marked experimental. It is sent only when the user changes it, never when the page opens (Android bug 4). Send and receive both use `[phone][media]` order (Android bug 5).
5. **CLI.** The CLI is a `ctl` subcommand of the main binary, so it works straight from the AppImage: `librepods-x86_64.AppImage ctl status`. There is no separate `librepods-ctl` binary.
6. **Not changed.**
   - The init sequence stays as it is (0x4D `FF…`, 0x29 `FF…`); it works on the user's hardware, and reliability comes first.
   - Hearing, PPE and HRM controls (0x2C, 0x2F, 0x33, 0x37, 0x38, 0x30) wait for Stages 4–5.
   - Name-only IDs with no known encoding stay log-only, but are visible in `ctl status`.
   - The spec's ownership note includes "if the audio source becomes another device, yield (send 06 = 00)". That is left out: the AirPods already send 06 = 00 when another device takes over, which Stage 1 handles, and an extra yield risks exactly the dropouts this port exists to fix.
7. **App settings file.** An old LibrePods (0.1.0) kept app settings in `~/.local/share/librepods/app_settings.json`, and the current migration misses that path; this is why the user's TokyoNightStorm theme is lost. Task 1 copies the file to the current path and leaves the original in place.

## Global Constraints

Everything in Stages 1 and 2 still applies:
- `ct <args>` means `distrobox enter librepods-dev -- bash -c 'cd /var/home/Cold/librepods-linux-rust/linux-rust && cargo <args>'`, and all paths are relative to `linux-rust/`.
- Never restart audio services or touch non-AirPods cards.
- No `unwrap`/`expect`/indexing on data from the AirPods, user files, D-Bus callers or BLE.
- Use `log` macros and `#[cfg(test)] mod tests` per module.
- Make local commits on `linux-port`, then push to `sondo`.
- Never touch the user's running LibrePods without asking.
- Keep LE keys out of logs.

Stage 3 adds:
- **Tests never act on the real desktop.** No test may call any of these:
  - `MediaController::pause`, `resume`, `pause_all_media`, `mpris_command`, `next_track`, `previous_track` or `play_pause`: they pause or skip the user's real players;
  - `handle_conversational_awareness` or `transition_sink_volume`: they change real volume;
  - `Notifier`, `control::serve` or `run_ctl`: they use the real session bus.

  Test the pure decision functions instead.
- **No unrequested commands.** The app never sends a control command the user didn't ask for. The one exception is re-applying their own stored choices on connect: the listening-mode cycle and stem forwarding. Defaults are never pushed to the AirPods.
- **Send only on a finished action.**
  - Sliders send on release (`on_release`), never on every move.
  - Text sends on submit.
  - Toggles and pickers send once per change.
- **File compatibility.**
  - `app_settings.json` keeps the keys `theme`, `tray_text_mode` and `stem_control`.
  - `preferences.json` keeps Stage 2's `autoConnect` key and any unknown keys.
  - `devices.json` changes only its `name` field, on rename.
  - All writes go through a temp file followed by a rename.
- **User commands never block.** Stem-action commands run as `sh -c <command>`, detached, with stdin/stdout/stderr null. Nothing awaits them inside an AACP or UI event loop.
- **`ctl` never starts the app.** When the subcommand is present, `main` runs the client and exits before any tray, BlueZ, UI or logging-to-journal setup.

## Review Focus

1. **Hand-edited, Stage-2-era or corrupt `preferences.json`.** Settings load with defaults, and other devices' entries and unknown keys survive a write. Tests: Task 1 `stage2_auto_connect_entry_still_reads`, `bad_entry_does_not_lose_others`, `unknown_keys_survive_update`.
2. **A control the model lacks is never shown or sent.** For example, Adaptive on Pro 1, or Off on Pro 2 without 0x34. Neither the page, the tray nor `ctl` offers it. Tests: Task 2 `pro1_listening_modes_offer_off_without_adaptive`, `pro2_off_needs_allow_off`, `catalog_gates_pro1`; Task 9 `ctl_rejects_mode_the_model_lacks`.
3. **A stem-action command that hangs, fails or doesn't exist.** The AACP event loop keeps running. Test: Task 5 `run_command_does_not_wait`.
4. **The AirPods report a value the catalog doesn't know.** For example, call config `00 01` or mic mode `07`. The row shows "Unknown (…)", and nothing is sent until the user picks a value. Tests: Task 2 `choice_matches_trimmed_value`; Task 6 `view_builds_for_every_model`, which includes unknown values.
5. **`ctl` with no AirPods, or with two pairs connected.** It prints a clear error and never picks a device at random. Test: Task 9 `resolve_target_rules`.

---

### Task 1: Per-device preferences and app settings

**Files:**
- Create: `src/prefs.rs`
- Modify:
  - `src/main.rs`: add `mod prefs;`; at startup, run `migrate_legacy_stem_control` over the AirPods MACs in devices.json.
  - `src/utils.rs`: `app_settings_path_in` and the legacy-path copy.
  - `src/bluetooth/le.rs`: `auto_connect_enabled` now uses `parse_prefs`.
  - `src/ui/tray.rs`, `src/ui/window.rs`, `src/devices/airpods.rs`: read and write app settings only through `load_app_settings` / `save_app_settings`.

**Interfaces:**
- Produces, in `crate::prefs`:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)] pub enum Bud { Left, Right }
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)] pub enum PressType { Single, Double, Triple, Long }
  #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
  #[serde(tag = "action", content = "command", rename_all = "camelCase")]
  pub enum StemAction { #[default] Native, PlayPause, NextTrack, PreviousTrack, CycleListeningMode, Command(String) }
  #[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)] #[serde(default, rename_all = "camelCase")]
  pub struct StemActions { pub left_single: StemAction, pub right_single: StemAction, pub left_double: StemAction, pub right_double: StemAction,
                           pub left_triple: StemAction, pub right_triple: StemAction, pub left_long: StemAction, pub right_long: StemAction }
  impl StemActions { pub fn get(&self, bud: Bud, press: PressType) -> &StemAction; pub fn set(&mut self, bud: Bud, press: PressType, action: StemAction); pub fn is_all_native(&self) -> bool }
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] #[serde(default, rename_all = "camelCase")]
  pub struct DevicePrefs {
      pub auto_connect: bool, pub ear_detection: bool, pub disconnect_when_not_wearing: bool,
      pub conversation_volume: u8, pub conversation_reduced_volume: u8, pub conversation_relative: bool, pub conversation_pause_music: bool,
      pub takeover_on_media_start: bool, pub takeover_from_music: bool, pub takeover_from_call: bool,
      pub listening_mode_cycle: Option<u8>, pub stem_actions: StemActions,
      #[serde(flatten)] pub extra: serde_json::Map<String, serde_json::Value>,
  }
  #[derive(Clone)] pub struct PrefsStore { /* path: PathBuf */ }
  impl PrefsStore {
      pub fn new(path: PathBuf) -> Self;
      pub fn at_default_path() -> Self;                         // utils::get_preferences_path()
      pub fn device(&self, mac: &str) -> DevicePrefs;           // re-reads the file every call; defaults when absent
      pub fn update(&self, mac: &str, f: impl FnOnce(&mut DevicePrefs)) -> std::io::Result<()>;
  }
  pub fn parse_prefs(json: &str) -> std::collections::HashMap<String, DevicePrefs>;
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)] #[serde(default)]
  pub struct AppSettings { pub theme: MyTheme, pub tray_text_mode: bool, pub stem_control: bool, pub notify_on_connect: bool, pub notify_low_battery: bool }
  pub fn load_app_settings(path: &Path) -> AppSettings;          // missing or invalid → Default
  pub fn save_app_settings(path: &Path, settings: &AppSettings) -> std::io::Result<()>;
  pub fn migrate_legacy_stem_control(settings: &mut AppSettings, store: &PrefsStore, macs: &[String]) -> bool;
  ```
  And in `crate::utils`: `pub fn app_settings_path_in(config_dir: &Path, data_dir: &Path) -> PathBuf`. `get_app_settings_path()` calls it with the XDG directories.
- **Defaults:**
  - `DevicePrefs`:
    - `auto_connect` false
    - `ear_detection` true
    - `disconnect_when_not_wearing` true
    - `conversation_volume` 25
    - `conversation_reduced_volume` 15
    - `conversation_relative` false
    - `conversation_pause_music` false
    - `takeover_on_media_start` true
    - `takeover_from_music` true
    - `takeover_from_call` false
    - `listening_mode_cycle` None
    - every stem action `Native`
  - `AppSettings`:
    - `theme` Dark
    - `tray_text_mode` false
    - `stem_control` false
    - `notify_on_connect` true
    - `notify_low_battery` true
- **`parse_prefs`:**
  - Parse the file to a `serde_json::Value`, then convert each MAC entry on its own. A bad entry becomes the defaults (warned) and the other entries are kept.
  - A non-object file gives an empty map.
- **`update`:**
  - Hold one process-wide `std::sync::Mutex` for the whole read-modify-write.
  - Re-read the file, change one entry, then write `preferences.json.tmp` and rename it over the file.
  - A corrupt file is renamed to `preferences.json.bak` first.
- **`app_settings_path_in`:**
  - Returns `config_dir/librepods/app_settings.json`.
  - If that file is missing, it copies the first of these that exists, and keeps the original: `data_dir/librepods/app_settings.json` (0.1.0), then `data_dir/app_settings.json`. The old code removed `data_dir/app_settings.json` after copying; it now stays too.
- **`migrate_legacy_stem_control`:**
  - Runs only when `settings.stem_control` is true.
  - Each listed MAC whose stem actions are all `Native` gets `left_double`/`right_double = NextTrack` and `left_triple`/`right_triple = PreviousTrack`.
  - It then sets `stem_control` false and returns true. `main` saves the settings when it returns true.

- [ ] **Step 1: Write the failing tests** (in `prefs.rs` and `utils.rs`):
  ```rust
  #[test] fn missing_prefs_file_gives_defaults() // PrefsStore::new(tmp/"preferences.json").device(MAC) == DevicePrefs::default(); spot-check: ear_detection, takeover_from_music true; takeover_from_call false; conversation_volume 25; listening_mode_cycle None; stem_actions.is_all_native()
  #[test] fn stage2_auto_connect_entry_still_reads() // file {"MAC":{"autoConnect":true}} → device(MAC).auto_connect && device(MAC).ear_detection
  #[test] fn bad_entry_does_not_lose_others() // {"A":{"autoConnect":"yes"},"B":{"earDetection":false}} → parse_prefs: A == default, B.ear_detection == false
  #[test] fn unknown_keys_survive_update() // {"MAC":{"futureKey":7},"OTHER":{"autoConnect":true}} → update(MAC, |p| p.ear_detection = false) → file still has MAC.futureKey == 7, OTHER.autoConnect == true, MAC.earDetection == false; no preferences.json.tmp left
  #[test] fn corrupt_prefs_are_backed_up_on_write() // "{oops" → update(MAC, ..) Ok; preferences.json.bak == "{oops"
  #[test] fn stem_action_serde_shape() // StemAction::Command("notify-send hi") ↔ {"action":"command","command":"notify-send hi"}; Native ↔ {"action":"native"}
  #[test] fn app_settings_defaults_and_partial_file() // load missing → default; {"theme":"TokyoNightStorm"} → theme TokyoNightStorm, notify_on_connect true; save then load round-trips
  #[test] fn legacy_stem_control_becomes_next_previous() // stem_control true, store empty, macs [MAC] → returns true; device(MAC).stem_actions.left_double == NextTrack, right_triple == PreviousTrack, left_single == Native; settings.stem_control == false; second call returns false
  #[test] fn app_settings_from_0_1_0_are_copied() // utils: data/librepods/app_settings.json = {"theme":"TokyoNightStorm"} → app_settings_path_in(config, data) == config/librepods/app_settings.json with that content; the data file still exists
  ```
  Update `le.rs`'s `auto_connect_off_without_pref` to also assert `auto_connect_enabled(r#"{"MAC":{"autoConnect":true,"earDetection":false}}"#, MAC)`. Under Stage 2's strict schema, that mixed entry returned false.
- [ ] **Step 2: Run and confirm they fail.** Run `ct test prefs:: utils:: le::`. Expected: compile errors.
- [ ] **Step 3: Implement.** Swap the three hand-written `json!({...})` writers in `window.rs` and the field-by-field readers in `window.rs`, `tray.rs` and `airpods.rs` for `load_app_settings` / `save_app_settings`.
- [ ] **Step 4: Run and confirm they pass.** `ct test` → all pass.
- [ ] **Step 5: Commit** `linux-rust: per-device preferences and app settings with atomic writes; recover 0.1.0 app settings`

### Task 2: Control catalog and listening-mode rules (pure)

**Files:**
- Create: `src/devices/settings.rs`
- Modify:
  - `src/devices/mod.rs`
  - `src/devices/models.rs`: remove the two `#[allow(dead_code)]`.
  - `src/bluetooth/aacp.rs`: the receive path uses `settings::trim_value`; `ControlCommandIdentifiers::from_u8` becomes `pub`.

**Interfaces:**
- Consumes: `AirPodsModel`/`Capability` (Stage 2), `StemActions` (Task 1).
- Produces, in `crate::devices::settings`:
  ```rust
  pub fn firmware_major(version3: &str) -> Option<u32>;                 // first char as a digit
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)] pub enum ListeningMode { Off, NoiseCancellation, Transparency, Adaptive }
  impl ListeningMode {
      pub fn byte(self) -> u8;                      // 0x01, 0x02, 0x03, 0x04
      pub fn from_byte(b: u8) -> Option<Self>;
      pub fn cycle_bit(self) -> u8;                 // 0x01, 0x02, 0x04, 0x08
      pub fn label(self) -> &'static str;           // "Off", "Noise Cancellation", "Transparency", "Adaptive"
      pub fn cli_name(self) -> &'static str;        // "off", "anc", "transparency", "adaptive"
  }
  pub fn listening_modes(model: AirPodsModel, allow_off: Option<bool>) -> Vec<ListeningMode>;   // order Off, NC, Transparency, Adaptive
  pub fn default_cycle(model: AirPodsModel) -> u8;                      // NC|Transparency, plus Adaptive if AdaptiveAudio
  pub fn toggle_cycle_mode(current: u8, mode: ListeningMode) -> u8;      // setting always allowed; clearing only if ≥ 2 bits remain
  pub fn next_in_cycle(current: Option<ListeningMode>, cycle: u8, available: &[ListeningMode]) -> Option<ListeningMode>;
  pub fn trim_value(value: &[u8]) -> Vec<u8>;                           // drop trailing zeros; all zero → [0]
  pub fn encode_toggle(on: bool) -> Vec<u8>;                            // [0x01] / [0x02]
  pub fn decode_toggle(value: &[u8]) -> Option<bool>;                   // first byte 1 → true, 2 → false, else None
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Section { Audio, PressAndHold, Calls, Microphone, Connection, Battery, Accessibility, Advanced }
  pub struct ChoiceOption { pub label: &'static str, pub value: &'static [u8] }
  pub enum ControlKind { Toggle, Choice(&'static [ChoiceOption]), Slider { max: u8, suffix: &'static [u8] }, PerBud(&'static [ChoiceOption]) }
  pub struct ControlSpec { pub id: ControlCommandIdentifiers, pub section: Section, pub label: &'static str, pub description: &'static str, pub kind: ControlKind }
  pub fn visible_controls(model: AirPodsModel, firmware_major: Option<u32>) -> Vec<&'static ControlSpec>;   // catalog order
  pub fn selected_choice(options: &[ChoiceOption], value: &[u8]) -> Option<usize>;  // trim_value(option) == trim_value(value)
  pub fn slider_value(value: &[u8]) -> Option<u8>;                      // first byte
  pub fn encode_slider(max: u8, suffix: &[u8], v: u8) -> Vec<u8>;       // [v.min(max)] ++ suffix
  pub fn encode_per_bud(left: u8, right: u8) -> Vec<u8>;                // [right, left]
  pub fn per_bud_values(value: &[u8]) -> (Option<u8>, Option<u8>);       // (left = value[1], right = value[0])
  pub fn shows_custom_eq(firmware_major: Option<u32>) -> bool;           // ≥ 9
  pub fn shows_stem_actions(model: AirPodsModel, actions: &StemActions) -> bool;   // has(StemConfig) || !is_all_native()
  pub fn rename_request(current: &str, draft: &str) -> Option<String>;   // trimmed; None if empty or unchanged
  ```
- **`listening_modes` rules:**
  - No modes without `Capability::ListeningMode`.
  - Off when `allow_off == Some(true)` or the model lacks `AdaptiveAudio`.
  - NC and Transparency always.
  - Adaptive only with `AdaptiveAudio`.
- **`next_in_cycle`:** walks Off → NC → Transparency → Adaptive → wrap, starting after `current` (or from the start when `None`). It returns the first mode that is both in `cycle` and in `available`.
- **The catalog (exact).** Labels and descriptions come from the Android rewrite's strings. Gate notation:
  - "LM" = `has(ListeningMode)`;
  - "¬H" = `!is_headset()`;
  - "fw" = `firmware_major`.

  | ID | Section | Label | Kind and values | Shown when |
  |---|---|---|---|---|
  | 0x28 ConversationDetectConfig | Audio | Conversation Awareness — "Lowers media volume and reduces background noise when you start speaking to other people." | Toggle | ConversationAwareness |
  | 0x26 AdaptiveVolumeConfig | Audio | Personalized Volume — "Adjusts the volume of media in response to your environment." | Toggle | AdaptiveVolume |
  | 0x35 SleepDetectionConfig | Audio | Pause media when falling asleep | Toggle | SleepDetection |
  | 0x34 AllowOffOption | Audio | Off Listening Mode — "When this is on, AirPods listening modes will include an Off option. Loud sound levels are not reduced when the listening mode is set to Off." | Toggle | LoudSoundReduction and fw ≥ 8 |
  | 0x16 ClickHoldMode | PressAndHold | Press and Hold | PerBud: Noise Control `01`, Siri `05` | LM and ¬H |
  | 0x24 CallManagementConfig | Calls | Call Controls | Choice: "Press once to mute, twice to hang up" `00 03`; "Press once to hang up, twice to mute" `00 02` | always |
  | 0x01 MicMode | Microphone | Microphone | Choice: Automatic `00`, Always Right `01`, Always Left `02` | always |
  | 0x0A EarDetectionConfig | Connection | Automatic Ear Detection — "Pauses media when you take an AirPod out and resumes it when you put it back." | Toggle | always |
  | 0x20 AutomaticConnectionConfig | Connection | Connect to this device automatically — "When enabled, AirPods will try to connect to this device automatically. Else, they will try to autoconnect only when last connected." | Toggle | always |
  | 0x3B DynamicEndOfCharge | Battery | Optimized Charge Limit — "AirPods learn your daily usage and charge to an optimized limit to preserve battery lifespan. May not affect older AirPods or firmware." | Toggle | always |
  | 0x17 DoubleClickInterval | Accessibility | Press Speed — "Adjust the speed required to press two or three times on your AirPods." | Choice: Default `00`, Slower `01`, Slowest `02` | always |
  | 0x18 ClickHoldInterval | Accessibility | Press and Hold Duration — "Adjust the duration required to press and hold on your AirPods." | Choice: Default `00`, Slower `01`, Slowest `02` | always |
  | 0x1B OneBudAncMode | Accessibility | Noise Cancellation with Single AirPod — "Allow AirPods to be put in noise cancellation mode when only one AirPod is in your ear." | Toggle | LM and ¬H |
  | 0x1F ChimeVolume | Accessibility | Tone Volume — "Adjust the tone volume of sound effects played by AirPods." | Slider max 100, suffix `50` | always |
  | 0x25 VolumeSwipeMode | Accessibility | Volume Control — "Adjust the volume by swiping up or down on the stem." | Toggle | SwipeForVolume |
  | 0x23 VolumeSwipeInterval | Accessibility | Volume Swipe Speed — "To prevent unintended volume adjustments, select preferred wait time between swipes." | Choice: Default `01`, Longer `02`, Longest `03` | SwipeForVolume |
  | 0x1C CrownRotationDirection | Accessibility | Digital Crown Direction | Choice: Default `02`, Reversed `01` | is_headset |
  | 0x12 VoiceTrigger | Advanced | "Hey Siri" — "Lets the AirPods listen for “Hey Siri” (used by Apple devices)." | Toggle | always |
  | 0x31 InCaseToneConfig | Advanced | Case Tones — "Sounds from charging cases with a speaker." | Toggle | always |
  | 0x40 InCaseToneVolume | Advanced | Case Tone Volume | Slider max 100, no suffix | always |

  The page in Tasks 6–7 renders these 0x09 IDs specially, not from the catalog:
  - 0x0D listening mode;
  - 0x2E adaptive strength;
  - 0x1A cycle;
  - 0x39 stem forwarding.

- [ ] **Step 1: Write the failing tests** (in `settings.rs`):
  ```rust
  #[test] fn pro1_listening_modes_offer_off_without_adaptive() // listening_modes(AirPodsPro1, None) == [Off, NoiseCancellation, Transparency]
  #[test] fn pro2_off_needs_allow_off() // (AirPodsPro2UsbC, None) == [NC, Transparency, Adaptive]; (…, Some(true)) == [Off, NC, Transparency, Adaptive]
  #[test] fn models_without_listening_mode_have_none() // listening_modes(AirPods2, Some(true)).is_empty()
  #[test] fn cycle_toggle_keeps_two_modes() // toggle(0x06, Transparency) == 0x06; toggle(0x07, Off) == 0x06; toggle(0x06, Off) == 0x07; toggle(0x0E, Adaptive) == 0x06
  #[test] fn next_in_cycle_skips_unselected() // avail [Off,NC,T]: (Some(NC), 0x06) → T; (Some(T), 0x06) → NC; (Some(Off), 0x06) → NC; (None, 0x07) → Off; (Some(T), 0x08 adaptive-only, avail without Adaptive) → None
  #[test] fn default_cycle_per_model() // Pro1 → 0x06; Pro2UsbC → 0x0E
  #[test] fn catalog_gates_pro1() // visible ids for (AirPodsPro1, Some(6)) contain ClickHoldMode, CallManagementConfig, MicMode, EarDetectionConfig, AutomaticConnectionConfig, DynamicEndOfCharge, DoubleClickInterval, ClickHoldInterval, OneBudAncMode, ChimeVolume, VoiceTrigger, InCaseToneConfig, InCaseToneVolume; and none of ConversationDetectConfig, AdaptiveVolumeConfig, AllowOffOption, SleepDetectionConfig, VolumeSwipeMode, VolumeSwipeInterval, CrownRotationDirection
  #[test] fn catalog_gates_pro2_by_firmware() // (Pro2UsbC, Some(8)) contains AllowOffOption, ConversationDetectConfig, VolumeSwipeMode; (Pro2UsbC, Some(7)) has no AllowOffOption
  #[test] fn catalog_gates_headsets() // (AirPodsMaxLightning, None) contains CrownRotationDirection, no ClickHoldMode, no OneBudAncMode
  #[test] fn catalog_ids_are_unique()
  #[test] fn choice_matches_trimmed_value() // call config: [00,03] → Some(0), [00,02] → Some(1), [00,01] → None; mic: [00] → Some(0), [07] → None; press speed [00] → Some(0)
  #[test] fn chime_volume_encodes_with_suffix() // encode_slider(100, &[0x50], 120) == [100, 0x50]; slider_value(&[0x50, 0x50]) == Some(80)
  #[test] fn per_bud_layout_is_right_then_left() // encode_per_bud(0x05, 0x01) == [0x01, 0x05]; per_bud_values(&[0x01, 0x05]) == (Some(0x05), Some(0x01)); per_bud_values(&[0x01]) == (None, Some(0x01))
  #[test] fn toggle_codec() // encode_toggle(true) == [1]; encode_toggle(false) == [2]; decode_toggle(&[1]) == Some(true); (&[2]) == Some(false); (&[0]) == None; (&[]) == None
  #[test] fn trim_matches_receive_path() // trim_value(&[3,0,0,0]) == [3]; (&[0,3,0,0]) == [0,3]; (&[0,0,0,0]) == [0]
  #[test] fn firmware_major_parses_first_digit() // "6619472" → Some(6); "8A356" → Some(8); "" → None; "A1" → None
  #[test] fn custom_eq_needs_firmware_9() // Some(9) true, Some(8) false, None false
  #[test] fn stem_actions_shown_when_supported_or_configured() // (Pro1, default) false; (Pro2UsbC, default) true; (Pro1, left_double = NextTrack) true
  #[test] fn rename_request_rules() // ("A", "A") None; ("A", "  ") None; ("A", " B ") Some("B")
  ```
- [ ] **Step 2: Run and confirm they fail.** Run `ct test settings::`. Expected: compile errors.
- [ ] **Step 3: Implement** the catalog as one `static CATALOG: &[ControlSpec]`, in the table's order, plus one gate `match` on the ID inside `visible_controls`. No unwraps.
- [ ] **Step 4: Run and confirm they pass.** `ct test` → all pass, including Stage 2's aacp tests after the `trim_value` switch.
- [ ] **Step 5: Commit** `linux-rust: per-model control catalog and listening-mode rules`

### Task 3: Custom EQ, headphone accommodation, safe rename, device info event

**Files:**
- Modify `src/bluetooth/aacp_parse.rs`: pure encoders and parsers.
- Modify `src/bluetooth/aacp.rs`:
  - opcodes, state fields, events, receive arms and send methods;
  - `AACPManagerState::new` becomes `pub(crate)`.

**Interfaces:**
- Produces:
  ```rust
  // crate::bluetooth::aacp_parse
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct CustomEq { pub enabled: bool, pub low: u8, pub mid: u8, pub high: u8 }
  pub fn custom_eq_payload(eq: CustomEq) -> [u8; 7];          // 05 00 01 [02 on | 01 off] low mid high, each min(100)
  pub fn parse_custom_eq(payload: &[u8]) -> Option<CustomEq>; // payload[2] == 5, state payload[5] ∈ {1,2}, bands payload[6..9] ≤ 100
  #[derive(Debug, Clone, Copy, PartialEq)] pub struct Accommodation { pub phone: bool, pub media: bool, pub bands: [f32; 8] }
  pub fn accommodation_payload(a: &Accommodation) -> Vec<u8>; // 84 00 02 02 [phone 01/02] [media 01/02] + bands (clamped 0..=100) as f32 LE, repeated 4× → 134 bytes
  pub fn parse_accommodation(payload: &[u8]) -> Option<Accommodation>; // len == 136, payload[2] == 0x84; phone payload[6], media payload[7]; bands from payload[8..40]
  pub fn rename_payload(name: &str) -> Option<Vec<u8>>;       // trimmed; empty or > 255 bytes → None; [0x01, len, 0x00, utf8…]
  // crate::bluetooth::aacp
  pub mod opcodes { pub const CUSTOM_EQ: u8 = 0x63; pub const HEADPHONE_ACCOMMODATION: u8 = 0x53; /* replaces EQ_DATA */ }
  // AACPManagerState: pub custom_eq: Option<CustomEq>, pub accommodation: Option<Accommodation>, pub firmware_major: Option<u32>  (all cleared by clear_connection_state)
  // AACPEvent: CustomEq(CustomEq), HeadphoneAccommodation(Accommodation), DeviceInfo { model: AirPodsModel, firmware_major: Option<u32> }
  impl AACPManager {
      pub async fn send_custom_eq(&self, eq: CustomEq) -> Result<()>;                   // data packet [0x63, 0x00] ++ payload
      pub async fn send_headphone_accommodation(&self, a: &Accommodation) -> Result<()>; // [0x53, 0x00] ++ payload (140-byte packet)
      pub async fn send_rename_packet(&self, name: &str) -> Result<()>;                 // uses rename_payload; None → InvalidInput error, nothing sent
  }
  ```
- **Offsets.** In this codebase `payload = packet[4..]`. A 0x53 packet is 140 bytes on the wire, so its payload is 136 bytes.
- **The INFORMATION arm:**
  - also sets `state.firmware_major = settings::firmware_major(&info.version3)`;
  - sends `AACPEvent::DeviceInfo { model, firmware_major }` on every Information packet.
- **The 0x63 and 0x53 arms:** store the parsed value in state, send the event, and log at `info`. A malformed packet logs at `debug` and is dropped.

- [ ] **Step 1: Write the failing tests** (in `aacp_parse.rs`, plus one in `aacp.rs`):
  ```rust
  #[test] fn custom_eq_round_trip() // payload(enabled, 30, 50, 70) == [05,00,01,02,30,50,70]; parse([63,00] ++ that) == Some(same); disabled → state byte 01
  #[test] fn custom_eq_clamps_and_rejects() // payload low 150 → byte 100; parse with len byte 4 → None; parse with band 101 → None; parse with state 3 → None
  #[test] fn accommodation_layout() // phone on, media off, bands[0] = 25.0: payload.len() == 134; payload[..6] == [84,00,02,02,01,02]; payload[6..10] == 25f32.to_le_bytes(); payload[6..38] == payload[38..70] == payload[70..102] == payload[102..134]
  #[test] fn accommodation_keeps_phone_media_order() // parse([53,00] ++ payload(phone on, media off)) → phone true, media false, bands equal
  #[test] fn accommodation_rejects_wrong_size_or_tag() // 135-byte payload → None; payload[2] = 0x85 → None
  #[test] fn rename_payload_rules() // "Logan’s AirPods" → [01, 17, 00] ++ utf8 (17 bytes: ’ is 3); "  x  " → [01,01,00,'x']; "" and "   " → None; "a"×256 → None; "a"×255 → Some, byte[1] == 255
  #[test] fn clearing_connection_state_forgets_eq_and_accommodation() // aacp.rs: custom_eq, accommodation, firmware_major Some → None after clear_connection_state
  ```
  Extend `no_parser_panics_on_truncation` with the 0x63 and 0x53 fixtures.
- [ ] **Step 2: Run and confirm they fail.** Run `ct test aacp`. Expected: compile errors.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** `ct test` → all pass.
- [ ] **Step 5: Commit** `linux-rust: custom EQ and headphone accommodation packets, length-checked rename, device info event`

### Task 4: Behaviour preferences drive ear detection, ducking and takeover

**Files:**
- Modify:
  - `src/media_controller.rs`;
  - `src/devices/airpods.rs`: `DeviceContext`; ownership-loss handlers call `suppress_takeover`;
  - `src/main.rs`: create one `PrefsStore` and one `DeviceContext`; pass them to `connect_airpods`.

**Interfaces:**
- Consumes: `PrefsStore`/`DevicePrefs` (Task 1), `AudioSource` (aacp).
- Produces:
  ```rust
  // crate::devices::airpods
  #[derive(Clone)] pub struct DeviceContext { pub tray_handle: Option<ksni::Handle<MyTray>>, pub ui_tx: UnboundedSender<BluetoothUIMessage>, pub battery_reporter: Option<BatteryReporter>, pub prefs: PrefsStore }
  impl AirPodsDevice { pub async fn new(mac_address: Address, ctx: DeviceContext) -> Option<Self> }
  // crate::media_controller
  impl MediaController {
      pub fn new(connected_mac: String, local_mac: String, backend: Arc<dyn AudioBackend>, prefs: PrefsStore) -> Self;
      pub async fn suppress_takeover(&self, duration: Duration);
      pub async fn play_pause(&self);                       // MPRIS "PlayPause" (used by Task 5)
  }
  #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum ConversationAction { Duck(u32), Restore, Pause, Resume, Nothing }
  pub fn conversation_action(level: u8, original: u32, prefs: &DevicePrefs) -> ConversationAction;
  pub fn takeover_allowed(prefs: &DevicePrefs, source: Option<&AudioSource>, local_mac: &str, suppressed: bool) -> bool;
  ```
- **`conversation_action`:**
  - Ducking mode (`conversation_pause_music` false):
    - level 1 or 3 → `Duck(t(conversation_volume))`;
    - level 2 → `Duck(t(conversation_reduced_volume))`;
    - level 4, 6, 7, 8 or 9 → `Restore`. Today 8 and 9 are ignored, which leaves the volume ducked.
    - anything else → `Nothing`.
    - Here `t(p) = if relative { original * p / 100 } else { original.min(p) }`.
  - Pause mode:
    - level 1 → `Pause`;
    - level 4, 6, 7, 8 or 9 → `Resume`;
    - anything else → `Nothing`.
  - `handle_conversational_awareness` keeps its original-volume capture and smooth transition, and branches on this result.
  - `Resume` resumes only the players this controller paused.
- **`takeover_allowed`:** false when `suppressed` or `!takeover_on_media_start`. Otherwise:
  - a source with a MAC other than `local_mac`:
    - type `Media` → `takeover_from_music`;
    - type `Call` → `takeover_from_call`;
  - anything else → true.

  `run_playback_listener` copies `audio_source` along with the ear status and checks this before taking over. Suppression lasts until the `Instant` stored by `suppress_takeover`.
- **Ownership loss suppresses takeover for 3 s.** This covers both 06 = 00 and the 0x11 SetOwnershipToFalse handlers in `airpods.rs`, via `suppress_takeover(Duration::from_secs(3))`.
- **Ear detection reads prefs on every event.** `handle_ear_detection` reads `prefs.device(mac)` each time:
  - it returns early when `!ear_detection`;
  - it deactivates A2DP on all-out only when `disconnect_when_not_wearing`.

  The two hard-coded state fields go away.

- [ ] **Step 1: Write the failing tests** (in `media_controller.rs`; pure functions only, per Global Constraints):
  ```rust
  #[test] fn conversation_ducks_to_absolute_cap() // defaults, original 60: level 1 → Duck(25); 2 → Duck(15); 3 → Duck(25); original 10, level 1 → Duck(10); 8 → Restore; 9 → Restore; 5 → Nothing
  #[test] fn conversation_relative_scales_original() // relative, 50 / 20, original 80: level 1 → Duck(40); level 2 → Duck(16)
  #[test] fn conversation_pause_mode() // pause_music: 1 → Pause; 2 → Nothing; 6 → Resume
  #[test] fn takeover_respects_audio_source() // defaults: None → true; other MAC Media → true; other MAC Call → false; local MAC Call → true; suppressed → false; takeover_on_media_start false → false
  ```
  Update the existing test helpers (`controller`, `make`) to pass a `PrefsStore` on a tempdir path.
- [ ] **Step 2: Run and confirm they fail.** Run `ct test media_controller::`. Expected: compile errors.
- [ ] **Step 3: Implement**, including the `DeviceContext` plumbing. `connect_airpods(registry, addr, ctx, device_managers)` replaces its four separate arguments.
- [ ] **Step 4: Run and confirm they pass.**
  - `ct test` → all pass.
  - `ct build` succeeds.
- [ ] **Step 5: Commit** `linux-rust: per-device ear detection, conversation ducking and takeover preferences`

### Task 5: Stem-press actions, listening-mode cycling, stored config on connect

**Files:**
- Create: `src/devices/commands.rs`
- Modify:
  - `src/devices/mod.rs`
  - `src/devices/airpods.rs`:
    - re-apply after the init sequence;
    - the StemPress arm uses `effective_stem_action`;
    - delete the old `stem_control` block.

**Interfaces:**
- Consumes: Task 1 (`StemActions`, `DevicePrefs`), Task 2 (`listening_modes`, `next_in_cycle`, `default_cycle`, `decode_toggle`), Task 4 (`play_pause`, `DeviceContext`).
- Produces, in `crate::devices::commands`:
  ```rust
  pub fn stem_forwarding_mask(actions: &StemActions) -> u8;   // per press type with any non-Native action on either bud: single 01, double 02, triple 04, long 08
  pub fn reapply_on_connect(prefs: &DevicePrefs) -> Vec<(ControlCommandIdentifiers, Vec<u8>)>;  // [ListeningModeConfigs [cycle]] if stored, then [StemConfig [mask]] if mask != 0
  pub fn effective_stem_action(actions: &StemActions, bud: Bud, press: PressType) -> StemAction; // Native → Single PlayPause, Double NextTrack, Triple PreviousTrack, Long CycleListeningMode
  pub fn press_from_aacp(press: StemPressType) -> PressType;
  pub fn bud_from_aacp(bud: StemPressBudType) -> Bud;
  pub fn cycle_target(model: AirPodsModel, controls: &[ControlCommandStatus], stored_cycle: Option<u8>) -> Option<ListeningMode>;
  pub async fn cycle_listening_mode(aacp: &AACPManager, prefs: &DevicePrefs) -> Option<ListeningMode>;  // computes the target from a state snapshot (lock released), sends 0x0D
  pub fn run_command(command: &str);                         // empty → warn and return; else spawn `sh -c`, detached, stdio null; spawn errors logged
  ```
- **`run_command` details:**
  - It uses `std::process::Command::spawn`, so no runtime is needed.
  - It hands the `Child` to a detached `std::thread` that calls `wait()`. Finished commands are reaped, so a long-running app collects no zombies, and the caller never waits.
- **`cycle_target`:**
  - current mode from the 0x0D value;
  - allow-off from 0x34;
  - cycle is the 0x1A value if reported, else `stored_cycle`, else `default_cycle(model)`;
  - returns `next_in_cycle(current, cycle, &listening_modes(model, allow_off))`.

  `cycle_listening_mode` uses `Unknown` while the model is not yet known.
- **StemPress arm** (`airpods.rs`), matching on `effective_stem_action`:
  - `PlayPause`, `NextTrack`, `PreviousTrack` → the `MediaController` methods;
  - `CycleListeningMode` → `cycle_listening_mode`;
  - `Command(c)` → `run_command(&c)`.

  Presses only arrive for press types in the forwarding mask, so a forwarded press whose action is `PlayPause` really does play/pause. That avoids Android bug 2.

- [ ] **Step 1: Write the failing tests** (in `commands.rs`):
  ```rust
  #[test] fn stem_mask_per_press_type() // left_single PlayPause → 0x01; right_triple Command("x") → 0x04; left_double + right_long set → 0x0A; default → 0x00
  #[test] fn nothing_reapplied_by_default() // reapply_on_connect(&DevicePrefs::default()).is_empty()
  #[test] fn stored_cycle_and_stem_mask_are_reapplied() // cycle Some(0x07), left_double NextTrack → [(ListeningModeConfigs, [0x07]), (StemConfig, [0x02])]
  #[test] fn native_presses_map_to_native_equivalents() // default: (Left, Double) → NextTrack, (Right, Long) → CycleListeningMode; left_double = PlayPause → (Left, Double) → PlayPause
  #[test] fn cycle_target_uses_reported_stored_or_default() // Pro1, controls [0D=02]: stored None → Transparency; stored Some(0x07) and 0D=03 → Off; controls [0D=02, 1A=0x03] → Off (reported wins); AirPods2 → None
  #[test] fn run_command_does_not_wait() // Instant before run_command("sleep 5") → elapsed < 200 ms; run_command("") and run_command("/nonexistent/x") return without panic
  ```
- [ ] **Step 2: Run and confirm they fail.** Run `ct test commands::`. Expected: compile errors.
- [ ] **Step 3: Implement.** In `AirPodsDevice::new`, send `reapply_on_connect(&ctx.prefs.device(mac))` after the proximity-key request. It replaces the `stem_control` block.
- [ ] **Step 4: Run and confirm they pass.** `ct test` → all pass; `ct build` succeeds.
- [ ] **Step 5: Commit** `linux-rust: configurable stem-press actions, listening-mode cycling, re-apply stored AirPods config on connect`

### Task 6: AirPods page state and catalog-driven controls

**Files:**
- Modify:
  - `src/devices/enums.rs`: new `AirPodsState`; delete `AirPodsNoiseControlMode`.
  - `src/ui/window.rs`: building the state, new messages and update arms, and `start_ui(…, prefs: PrefsStore)`.
  - `src/ui/airpods.rs`: rewrite of the view; delete `run_async_in_thread`.
  - `src/main.rs`: pass the `PrefsStore` to `start_ui`.

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces:
  ```rust
  // crate::devices::enums
  #[derive(Clone, Debug)] pub struct AirPodsState {
      pub device_name: String, pub name_draft: String, pub battery: Vec<BatteryInfo>,
      pub controls: HashMap<ControlCommandIdentifiers, Vec<u8>>,
      pub model: Option<AirPodsModel>, pub firmware_major: Option<u32>,
      pub custom_eq: Option<CustomEq>, pub accommodation: Option<Accommodation>,
      pub prefs: DevicePrefs, pub show_advanced: bool,
  }
  impl AirPodsState {
      pub fn from_aacp(state: &AACPManagerState, name: String, prefs: DevicePrefs) -> Self;
      pub fn apply_event(&mut self, event: &AACPEvent);   // ControlCommand → controls; BatteryInfo; DeviceInfo; CustomEq; HeadphoneAccommodation
      pub fn model_or_unknown(&self) -> AirPodsModel;
  }
  // crate::ui::window::Message (new; StateChanged stays for Nothing devices only)
  SetControl(String, ControlCommandIdentifiers, Vec<u8>),   // optimistic insert + send
  ControlDraft(String, ControlCommandIdentifiers, Vec<u8>), // slider moving: local only
  SendControl(String, ControlCommandIdentifiers),           // slider released: send the local value
  NameDraft(String, String), SubmitName(String),
  ToggleAdvanced(String), CommandSent(String, bool),
  // crate::ui::airpods
  pub fn airpods_view<'a>(mac: &'a str, devices_list: &HashMap<String, DeviceData>, state: &'a AirPodsState) -> iced::widget::Container<'a, Message>;
  ```
- **Update arms:**
  - Sends go through `Task::perform` with the `Arc<AACPManager>` from `device_managers` (`blocking_read`, as today). There is no unwrap: a missing manager logs and does nothing.
  - `SetControl(…, EarDetectionConfig, v)` also stores `prefs.ear_detection = decode_toggle(&v)`.
  - `SetControl(…, ListeningModeConfigs, v)` also stores `prefs.listening_mode_cycle = v.first()`.
  - **`SubmitName`:**
    - it runs `rename_request`;
    - then `send_rename_packet`;
    - then, inside the same task and via `spawn_blocking`, `update_devices_file(…, |d| d.name = new)`.
  - The `DeviceConnected` arm drops its two `unwrap`s; when there's no AACP manager it logs and skips.
- **View, generic part.**
  - **Layout:**
    - a name row (`text_input` with `on_input` → `NameDraft`, `on_submit` → `SubmitName`, and a "Rename" button);
    - then one titled card per `Section`, holding that section's `visible_controls(model_or_unknown, firmware_major)` rows;
    - then Device Information (unchanged);
    - all inside a `scrollable`.
  - **Rows by kind:**
    - Toggle → `toggler`. Its value is `decode_toggle`; for 0x0A an unreported value falls back to `prefs.ear_detection`.
    - Choice → `pick_list` of labels. Selected is `selected_choice`; an unknown value shows the placeholder `Unknown (hex)`.
    - Slider → `slider(0..=max)` with `ControlDraft` on change and `SendControl` on release.
    - PerBud → two `pick_list`s, Left and Right, that send `encode_per_bud`.
  - **Advanced** is collapsed behind a button (`ToggleAdvanced`), with the note "These may not be supported by your AirPods."
  - A section with no visible rows is not drawn.

- [ ] **Step 1: Write the failing tests:**
  ```rust
  // devices/enums.rs
  #[test] fn state_from_aacp_and_events() // AACPManagerState with 0D=02 and 1F=[50,50] → controls[ListeningMode] == [2]; apply ControlCommand(0D=03) → [3]; apply DeviceInfo{AirPodsPro1, Some(6)} → model Some(Pro1), firmware_major Some(6); apply CustomEq(..) → Some; apply BatteryInfo(v) → battery == v
  // ui/airpods.rs
  #[test] fn view_builds_for_every_model() // for every AirPodsModel variant, plus model None: state with controls {24=[00,01], 01=[07], 1F=[50,50], 16=[01,05]} (unknown values included), show_advanced both ways → airpods_view(MAC, &HashMap::new(), &state) builds without panic
  ```
- [ ] **Step 2: Run and confirm they fail.** Run `ct test enums:: ui::airpods`. Expected: compile errors.
- [ ] **Step 3: Implement.** Extract small private row builders (`toggle_row`, `choice_row`, `slider_row`, `per_bud_row`, `section_card`) that keep today's card style. That style is the `primary.scale_alpha(0.1)` background with radius 16.
- [ ] **Step 4: Run and confirm they pass.**
  - `ct test` → all pass.
  - `ct build` succeeds.
  - `grep -n "run_async_in_thread\|Runtime::new" src/ui/` → no matches.
- [ ] **Step 5: Commit** `linux-rust: AirPods page driven by the per-model control catalog`

### Task 7: Page sections for listening modes, press-and-hold, stem actions, EQ, accommodation and behaviour

**Files:**
- Modify:
  - `src/ui/airpods.rs`: new sections;
  - `src/ui/window.rs`: new messages and arms; the Settings tab gains notification toggles and loses the old stem toggle.

**Interfaces:**
- Consumes: Task 2 (modes, cycle), Task 3 (`send_custom_eq`, `send_headphone_accommodation`), Task 5 (`stem_forwarding_mask`), Task 6 (state, `SetControl`).
- Produces, as new `Message` variants:
  ```rust
  CustomEqDraft(String, CustomEq), SendCustomEq(String),
  AccommodationDraft(String, Accommodation), SendAccommodation(String),
  SetPrefs(String, DevicePrefs),        // store + PrefsStore::update; if stem_forwarding_mask changed → send StemConfig [new mask]
  PrefsDraft(String, DevicePrefs),      // sliders / command text: local only
  SavePrefs(String),                    // release / submit
  NotifyOnConnectChanged(bool), NotifyLowBatteryChanged(bool),
  // private, in ui/window.rs
  fn stem_config_update(old: &DevicePrefs, new: &DevicePrefs) -> Option<Vec<u8>>;  // Some([new mask]) when stem_forwarding_mask differs, else None
  ```
- **Sections, in page order** (all gates use `model_or_unknown()`):
  1. **Listening Mode.**
     - Shown when `listening_modes(model, allow_off)` is not empty.
     - A row of buttons, one per mode, with the current mode highlighted; a press sends `SetControl(ListeningMode, [mode.byte()])`.
     - With `AdaptiveAudio` and the current mode Adaptive, add an "Adaptive noise filtering" slider for 0x2E. It runs 0..=100, with the ends labelled "Let in more sound" / "Reduce more noise", and sends the raw value on release.
  2. **Press and Hold** (catalog section `PressAndHold`, extended).
     - One checkbox per available listening mode, labelled with the Android descriptions: Off "Turns off noise management", Noise Cancellation "Blocks out external sounds", Transparency "Lets in external sounds", Adaptive "Dynamically adjust external noise".
     - The mask is the reported 0x1A, else `prefs.listening_mode_cycle`, else `default_cycle`. A change sends `SetControl(ListeningModeConfigs, [toggle_cycle_mode(mask, mode)])`.
  3. **Stem Actions** (when `shows_stem_actions`).
     - For each press type, two `pick_list`s (Left, Right) with the options "AirPods default", "Play/Pause", "Next track", "Previous track", "Cycle listening mode", "Run command…".
     - "Run command…" shows a `text_input` (`PrefsDraft` on input, `SavePrefs` on submit).
  4. **Audio**, extended:
     - **Equalizer** when `shows_custom_eq`: an enable toggle (sends at once), plus Low, Mid and High sliders 0..=100 that send on release. The default before any 0x63 report is (off, 50, 50, 50).
     - **"When you speak"** when `has(ConversationAwareness)`. These are prefs only:
       - "Lower media to" slider (`conversation_volume`);
       - "Lower further to" slider (`conversation_reduced_volume`);
       - "Relative to current volume" toggle;
       - "Pause media instead" toggle.
  5. **Connection**, extended with prefs:
     - "Disconnect audio when not wearing";
     - "Connect when nearby (BLE)": `auto_connect`, described as "Connect these AirPods to this computer when they are out of the case and not connected to anything.";
     - Takeover:
       - "Take over when media starts playing here" (`takeover_on_media_start`);
       - "…even if they are playing music for another device" (`takeover_from_music`);
       - "…even during a call on another device" (`takeover_from_call`).
  6. **Accessibility**, extended with **Headphone Accommodation (experimental)**:
     - Phone and Media toggles;
     - when either is on, 8 band sliders 0..=100;
     - it sends `send_headphone_accommodation` on a toggle or a slider release, using the last reported bands or 50.0 each;
     - nothing is sent when the page opens.
- **The Settings tab** replaces "Stem press track control" with "Notify when AirPods connect" and "Low battery notifications" (20 %). Both are saved via `save_app_settings`.

- [ ] **Step 1: Write the failing tests:**
  ```rust
  // ui/airpods.rs — extend view_builds_for_every_model with: stem actions configured (Command("x") on left_long), custom_eq Some, accommodation Some(phone on), firmware Some(9); and a Pro2UsbC state with 0D=04 (Adaptive slider visible)
  // ui/window.rs
  #[test] fn changing_stem_actions_recomputes_forwarding() // stem_config_update: default → left_double NextTrack == Some([0x02]); unchanged == None; back to default == Some([0x00])
  ```
- [ ] **Step 2: Run and confirm they fail.** Run `ct test ui::`. Expected: compile errors.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** `ct test` → all pass; `ct build` succeeds.
- [ ] **Step 5: Commit** `linux-rust: listening-mode, press-and-hold, stem action, EQ, accommodation and behaviour sections`

### Task 8: Tray quick controls and desktop notifications

**Files:**
- Create: `src/ui/notifications.rs`
- Modify:
  - `src/ui/mod.rs`
  - `src/ui/tray.rs`: model-aware listening modes, conversation-awareness gating, a battery line.
  - `src/devices/airpods.rs`:
    - the DeviceInfo arm sets `tray.model`;
    - the BatteryInfo arm notifies;
    - `DeviceContext` gains `notifier`.
  - `src/main.rs`:
    - create the `Notifier`;
    - `disconnect_airpods` also clears `command_tx`, `conversation_detect_enabled`, `allow_off_option` and `model`. This was a Stage 1 deferred minor.
  - `src/bluetooth/aacp.rs`: `BatteryComponent` derives `Hash`.

**Interfaces:**
- Consumes: Task 2 (`listening_modes`), Task 3 (`DeviceInfo`), Task 1 (`AppSettings`).
- Produces:
  ```rust
  // crate::ui::notifications
  #[derive(Clone)] pub struct Notifier { /* zbus::Connection + Arc<Mutex<HashMap<String, u32>>> */ }
  impl Notifier {
      pub async fn connect() -> Option<Self>;                            // session bus; None (logged) if unavailable
      pub async fn show(&self, key: &str, summary: &str, body: &str);    // org.freedesktop.Notifications.Notify("LibrePods", replaces_id for key, "audio-headphones", summary, body, [], {}, -1)
  }
  pub fn battery_summary(batteries: &[BatteryInfo]) -> String;          // "L 94% · R 95%⚡ · Case 80%"; skips Disconnected; Headphone alone → "80%"
  #[derive(Default)] pub struct LowBatteryTracker { /* HashSet<BatteryComponent> */ }
  impl LowBatteryTracker { pub fn check(&mut self, batteries: &[BatteryInfo]) -> Vec<BatteryComponent>; }   // newly ≤ 20 % (level > 0, not charging, not disconnected, not Case); re-arms at ≥ 25 % or charging
  // crate::ui::tray
  pub model: Option<AirPodsModel>,                                       // new MyTray field
  pub fn tray_modes(model: Option<AirPodsModel>, allow_off: Option<u8>) -> Vec<ListeningMode>;  // listening_modes(model.unwrap_or(Unknown), allow_off.map(|b| b == 0x01))
  ```
- **Notifications:**
  - The first BatteryInfo of a connection, when `notify_on_connect`, gives summary "{name} connected" with body `battery_summary`, under key `connect:{mac}`.
  - Each component returned by `check`, when `notify_low_battery`, gives "{name}: left AirPod at 18 %", "right AirPod", or "AirPods" for a headset, under key `battery:{mac}`.
  - Both settings are read with `load_app_settings` when the event happens.
  - `{name}` is the device's name in devices.json, or "AirPods" if it has none.
- **Tray menu:**
  - a disabled battery line (the tooltip text) while connected;
  - Open Window;
  - the listening-mode radio from `tray_modes`;
  - Conversation Detection, shown only when the model has ConversationAwareness, or while the model is unknown and the AirPods have reported 0x28;
  - Exit.

- [ ] **Step 1: Write the failing tests:**
  ```rust
  #[test] fn battery_summary_formats() // L 94 NotCharging, R 95 Charging, Case 0 Disconnected → "L 94% · R 95%⚡"; Headphone 80 NotCharging → "80%"; empty → ""
  #[test] fn low_battery_alerts_once_and_rearms() // L 19 NC → [Left]; L 18 → []; L 30 → []; L 19 → [Left]; R 15 Charging → []; Case 10 → []; L 0 Disconnected → []
  #[test] fn tray_modes_follow_model() // (None, None) → [NC, T, Adaptive]; (Some(Pro1), None) → [Off, NC, T]; (Some(Pro2UsbC), Some(0x01)) → [Off, NC, T, Adaptive]
  ```
- [ ] **Step 2: Run and confirm they fail.** Run `ct test notifications:: tray::`. Expected: compile errors.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.** `ct test` → all pass; `ct build` succeeds.
- [ ] **Step 5: Commit** `linux-rust: model-aware tray quick controls and desktop notifications`

### Task 9: Session D-Bus control interface and `librepods ctl`

**Files:**
- Create: `src/control.rs`
- Modify `src/main.rs`:
  - `Args` gains `#[command(subcommand)] command: Option<Command>`;
  - `Command::Ctl` runs `control::run_ctl` and exits before any other setup;
  - `async_main` keeps the `serve` connection alive.

**Interfaces:**
- Consumes: Task 2 (`ListeningMode`, `listening_modes`), Task 5 (`cycle_listening_mode`), Task 1 (`PrefsStore`), `DeviceManagers`.
- Produces, in `crate::control`:
  ```rust
  pub const BUS_NAME: &str = "me.kavishdevar.librepods";
  pub const OBJECT_PATH: &str = "/me/kavishdevar/librepods";
  // interface "me.kavishdevar.librepods.Control1":
  //   Devices() -> a(ssb)                          (mac, name from devices.json, connected = aacp.is_connected())
  //   Status(mac s) -> s                           status_json
  //   SetListeningMode(mac s, mode s)              mode: off | anc | transparency | adaptive | next
  //   SetConversationAwareness(mac s, on b)
  //   SetControl(mac s, id y, value ay)            raw, for testing; ≤ 4 value bytes; unknown id → error
  //   Errors: zbus::fdo::Error::Failed(message)
  pub async fn serve(device_managers: Arc<RwLock<HashMap<String, DeviceManagers>>>, prefs: PrefsStore) -> Option<zbus::Connection>;  // name taken → warn, None
  #[derive(clap::Subcommand)] pub enum CtlCommand { Devices, Status, Mode { mode: String }, Ca { state: String }, Control { id: String, value: String } }
  #[derive(clap::Args)] pub struct CtlArgs { #[command(subcommand)] pub command: CtlCommand, #[arg(long, global = true)] pub mac: Option<String> }
  pub fn run_ctl(args: CtlArgs) -> i32;        // own tokio runtime; "LibrePods is not running" (exit 1) when BUS_NAME has no owner; Status/Devices print JSON; errors to stderr, exit 1
  pub fn resolve_target(connected: &[String], requested: Option<&str>) -> Result<String, String>;
  #[derive(Debug, PartialEq, Eq)] pub enum ModeArg { Mode(ListeningMode), Next }
  pub fn parse_mode_arg(s: &str) -> Option<ModeArg>;   // off; anc|nc|noise-cancellation; transparency|t; adaptive; next
  pub fn mode_for_request(model: AirPodsModel, allow_off: Option<bool>, mode: ListeningMode) -> Result<ListeningMode, String>;  // must be in listening_modes
  pub fn parse_control_args(id: &str, value: &str) -> Result<(u8, Vec<u8>), String>;   // hex; value 1–4 bytes
  pub fn status_json(mac: &str, name: &str, state: &AACPManagerState) -> String;
  ```
- **`resolve_target`:**
  - `requested` is matched case-insensitively against `connected` and must be in it;
  - with no MAC requested:
    - exactly one connected → that one;
    - none → `"no AirPods connected"`;
    - several → `"several AirPods connected; pass --mac"`.
- **`status_json` keys:**
  - `mac`, `name`;
  - `model` (display name or null), `firmwareMajor`;
  - `battery` (`[{component, level, status}]`);
  - `listeningMode` (`cli_name` or null);
  - `controls`: an object mapping lowercase hex ID (`"0x0d"`) to the hex value, for **every** reported control, including name-only IDs.

- [ ] **Step 1: Write the failing tests:**
  ```rust
  #[test] fn resolve_target_rules() // ([A], None) → Ok(A); ([], None) → Err("no AirPods connected"); ([A, B], None) → Err containing "--mac"; ([A], Some(a lowercase)) → Ok(A); ([A], Some(B)) → Err
  #[test] fn mode_arg_parsing() // "anc", "nc", "noise-cancellation" → Mode(NC); "t" → Mode(Transparency); "off"; "adaptive"; "next" → Next; "loud" → None
  #[test] fn ctl_rejects_mode_the_model_lacks() // (Pro1, None, Adaptive) → Err; (Pro1, None, Off) → Ok; (Pro2UsbC, None, Off) → Err; (Pro2UsbC, Some(true), Off) → Ok
  #[test] fn parse_control_args_hex() // ("0d","02") → (0x0D, [2]); ("1f","5050") → (0x1F, [0x50, 0x50]); ("zz","00") → Err; ("0d","0102030405") → Err; ("0d","") → Err
  #[test] fn status_json_shape() // state with controls 0D=[03] and 1F=[50,50], model Pro1 → JSON parses; listeningMode == "transparency"; controls["0x1f"] == "5050"; model == "AirPods Pro (1st generation)"
  #[test] fn ctl_subcommand_parses() // Args::try_parse_from(["librepods", "ctl", "mode", "anc", "--mac", MAC]) → Command::Ctl with Mode{"anc"} and mac Some(MAC)
  ```
- [ ] **Step 2: Run and confirm they fail.** Run `ct test control::`. Expected: compile errors.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run and confirm they pass.**
  - `ct test` → all pass.
  - `ct build` succeeds.
  - `ct run -- ctl status` with no LibrePods on the container's session bus prints "LibrePods is not running" and exits 1.
- [ ] **Step 5: Commit** `linux-rust: session D-Bus control interface and ctl subcommand`

### Task 10: Build alpha.5, check on hardware, push

**Files:** `Cargo.toml` (`version = "0.2.0-alpha.5"`)

- [ ] **Step 1: Gate.**
  - `ct test` passes.
  - `ct clippy` shows no warnings in `prefs.rs`, `devices/settings.rs`, `devices/commands.rs`, `ui/notifications.rs` or `control.rs`.
- [ ] **Step 2: Build.** Run:
  ```
  distrobox enter librepods-dev -- bash -c 'cd /var/home/Cold/librepods-linux-rust/linux-rust && PATH=$HOME/.local/opt/appimage-tools/bin:$PATH APPIMAGE_EXTRACT_AND_RUN=1 just build-appimage'
  ```
  Then copy the result to `~/Applications/LibrePods-port-0.2.0-alpha.5-x86_64.AppImage`, `chmod +x` it, and repoint the `librepods-x86_64.AppImage` symlink. This is the arrangement the user approved on 2026-10-26.
- [ ] **Step 3: Ask the user, then restart their LibrePods.** Check first that no game is running (`flatpak ps`, Steam games) and no call is active (`pactl list sink-inputs`). Then run `systemctl --user restart app-librepods@autostart.service`.
- [ ] **Step 4: Passive hardware checks.** No user action is needed:
  - The journal shows `Detected AirPods Pro (1st generation)`, the line for the re-applied stored config (none expected yet), and no errors.
  - `~/Applications/librepods-x86_64.AppImage ctl status` prints `"model": "AirPods Pro (1st generation)"`, the battery, and the current listening mode.
  - `~/.config/librepods/app_settings.json` exists, its theme is `TokyoNightStorm`, and the window uses that theme.
  - The tray menu offers Off, Noise Cancellation and Transparency, with no Adaptive and no Conversation Detection. Read it via `busctl --user call <tray name> /MenuBar com.canonical.dbusmenu GetLayout iias 0 -1 0`.
- [ ] **Step 5: Hardware checks that need the user.** Tell them what each check does first, because they will hear the listening mode change.
  1. `ctl mode off`, then `ctl mode <the previous mode>`. Expected: the journal shows `Received Control Command: ListeningMode, value: 01`, then the previous value. This confirms Decision 1, Off on Pro 1. If the buds answer with another mode instead, change `listening_modes` so models without Adaptive need 0x34 like the others, and note it in the ledger.
  2. In the window, tick Off + Noise Cancellation + Transparency under Press and Hold, then long-press a stem three times. Expected: the journal shows 0x0D cycling through 01, 02 and 03.
  3. In the window, change Press Speed to Slower and back. Expected: no errors in the journal; log the AirPods' echo either way.
- [ ] **Step 6: Commit and push.** Commit the version bump, then `git push sondo linux-port`.
