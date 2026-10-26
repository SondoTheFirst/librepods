use crate::bluetooth::aacp::AACPManager;
use crate::bluetooth::aacp::{ControlCommandIdentifiers, EarDetectionStatus};
use crate::audio::{
    A2dpAction, AudioBackend, audio_lock_for, find_bluez_card, get_sink_name_by_mac,
    get_sink_volume_percent_by_name_sync, plan_a2dp_activation, transition_sink_volume,
};
use dbus::blocking::Connection;
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use log::{debug, error, info, warn};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

struct MediaControllerState {
    connected_device_mac: String,
    local_mac: String,
    is_playing: bool,
    paused_by_app_services: Vec<String>,
    old_in_ear_data: Vec<bool>,
    user_played_the_media: bool,
    i_paused_the_media: bool,
    ear_detection_enabled: bool,
    disconnect_when_not_wearing: bool,
    conv_original_volume: Option<u32>,
    conv_conversation_started: bool,
}

impl MediaControllerState {
    fn new() -> Self {
        MediaControllerState {
            connected_device_mac: String::new(),
            local_mac: String::new(),
            is_playing: false,
            paused_by_app_services: Vec::new(),
            old_in_ear_data: vec![false, false],
            user_played_the_media: false,
            i_paused_the_media: false,
            ear_detection_enabled: true,
            disconnect_when_not_wearing: true,
            conv_original_volume: None,
            conv_conversation_started: false,
        }
    }
}

#[derive(Clone)]
pub struct MediaController {
    state: Arc<Mutex<MediaControllerState>>,
    backend: Arc<dyn AudioBackend>,
    /// Serializes every card-profile change for this MAC, across all instances, so concurrent
    /// triggers can't race each other. Shared via `audio_lock_for`.
    audio_op: Arc<Mutex<()>>,
    a2dp_wait_attempts: u32,
    a2dp_wait_interval: Duration,
}

impl MediaController {
    pub fn new(connected_mac: String, local_mac: String, backend: Arc<dyn AudioBackend>) -> Self {
        let audio_op = audio_lock_for(&connected_mac);
        let mut state = MediaControllerState::new();
        state.connected_device_mac = connected_mac;
        state.local_mac = local_mac;
        MediaController {
            state: Arc::new(Mutex::new(state)),
            backend,
            audio_op,
            a2dp_wait_attempts: 10,
            a2dp_wait_interval: Duration::from_millis(500),
        }
    }

    #[cfg(test)]
    pub fn with_a2dp_wait(mut self, attempts: u32, interval: Duration) -> Self {
        self.a2dp_wait_attempts = attempts;
        self.a2dp_wait_interval = interval;
        self
    }

    pub async fn run_playback_listener(
        self,
        aacp_manager: AACPManager,
        control_tx: tokio::sync::mpsc::UnboundedSender<(ControlCommandIdentifiers, Vec<u8>)>,
    ) {
        info!("Starting playback listener loop");
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if !aacp_manager.is_connected() {
                info!("AACP connection closed, stopping playback listener");
                return;
            }

            let is_playing = tokio::task::spawn_blocking(Self::check_if_playing)
                .await
                .unwrap_or(false);

            let mut state = self.state.lock().await;
            let was_playing = state.is_playing;
            state.is_playing = is_playing;
            let local_mac = state.local_mac.clone();
            drop(state);

            if !was_playing && is_playing {
                // Copy what we need and release the AACP lock before sending anything:
                // sending packets locks the same state.
                let (ear_status, connected_devices) = {
                    let aacp_state = aacp_manager.state.lock().await;
                    (
                        aacp_state.ear_detection_status.clone(),
                        aacp_state.connected_devices.clone(),
                    )
                };
                if !should_take_over(was_playing, is_playing, &ear_status) {
                    info!("Media playback started but buds not in ear, skipping takeover");
                    continue;
                }
                info!("Media playback started, taking ownership and activating a2dp");
                let _ = control_tx.send((ControlCommandIdentifiers::OwnsConnection, vec![0x01]));
                self.activate_a2dp_profile().await;

                for device in connected_devices {
                    if device.mac != local_mac {
                        info!("Asking {} to hand over the AirPods", device.mac);
                        if let Err(e) = aacp_manager
                            .send_media_information(&local_mac, &device.mac, true)
                            .await
                        {
                            error!("Failed to send media information to {}: {}", device.mac, e);
                        }
                        if let Err(e) = aacp_manager.send_smart_routing_show_ui(&device.mac).await {
                            error!(
                                "Failed to send smart routing show ui to {}: {}",
                                device.mac, e
                            );
                        }
                        if let Err(e) = aacp_manager.send_hijack_request(&device.mac).await {
                            error!("Failed to send hijack request to {}: {}", device.mac, e);
                        }
                    }
                }

                debug!("completed playback takeover process");
            }
        }
    }

    fn check_if_playing() -> bool {
        let conn = match Connection::new_session() {
            Ok(c) => c,
            Err(_) => return false,
        };
        Self::list_mpris_services(&conn).iter().any(|service| {
            let proxy = conn.with_proxy(service, "/org/mpris/MediaPlayer2", Duration::from_secs(5));
            proxy
                .get::<String>("org.mpris.MediaPlayer2.Player", "PlaybackStatus")
                .map(|s| s == "Playing")
                .unwrap_or(false)
        })
    }

    fn is_kdeconnect_service(service: &str) -> bool {
        service.starts_with("org.mpris.MediaPlayer2.kdeconnect.mpris_")
    }

    pub async fn handle_ear_detection(
        &self,
        old_statuses: Vec<EarDetectionStatus>,
        new_statuses: Vec<EarDetectionStatus>,
    ) {
        debug!(
            "Entering handle_ear_detection with old_statuses: {:?}, new_statuses: {:?}",
            old_statuses, new_statuses
        );

        let old_in_ear_data: Vec<bool> = old_statuses
            .iter()
            .map(|s| *s == EarDetectionStatus::InEar)
            .collect();
        let new_in_ear_data: Vec<bool> = new_statuses
            .iter()
            .map(|s| *s == EarDetectionStatus::InEar)
            .collect();

        let in_ear = new_in_ear_data.iter().all(|&b| b);
        let old_all_out = old_in_ear_data.iter().all(|&b| !b);
        let new_has_at_least_one_in = new_in_ear_data.iter().any(|&b| b);
        let new_all_out = new_in_ear_data.iter().all(|&b| !b);

        debug!(
            "Computed states: in_ear={}, old_all_out={}, new_has_at_least_one_in={}, new_all_out={}",
            in_ear, old_all_out, new_has_at_least_one_in, new_all_out
        );

        {
            let state = self.state.lock().await;
            if !state.ear_detection_enabled {
                debug!("Ear detection disabled, skipping");
                return;
            }
        }

        if new_has_at_least_one_in && old_all_out {
            debug!("Condition met: buds inserted, activating A2DP and checking play state");
            self.activate_a2dp_profile().await;
            {
                let mut state = self.state.lock().await;
                if state.is_playing {
                    state.user_played_the_media = true;
                    debug!("Set user_played_the_media to true as media was playing");
                }
            }
        } else if new_all_out {
            debug!("Condition met: buds removed, pausing media");
            self.pause().await;
            {
                let state = self.state.lock().await;
                if state.disconnect_when_not_wearing {
                    debug!("Disconnect when not wearing enabled, deactivating A2DP");
                    drop(state);
                    self.deactivate_a2dp_profile().await;
                }
            }
        }

        let reset_user_played = (old_in_ear_data.iter().any(|&b| !b)
            && new_in_ear_data.iter().all(|&b| b))
            || (new_in_ear_data.iter().any(|&b| !b) && old_in_ear_data.iter().all(|&b| b));
        if reset_user_played {
            debug!("Transition detected, resetting user_played_the_media");
            let mut state = self.state.lock().await;
            state.user_played_the_media = false;
        }

        info!(
            "Ear Detection - old_in_ear_data: {:?}, new_in_ear_data: {:?}",
            old_in_ear_data, new_in_ear_data
        );

        let mut old_sorted = old_in_ear_data.clone();
        old_sorted.sort();
        let mut new_sorted = new_in_ear_data.clone();
        new_sorted.sort();
        if new_sorted != old_sorted {
            debug!("Ear data changed, checking resume/pause logic");
            if in_ear {
                debug!("Resuming media as buds are in ear");
                self.resume().await;
                self.state.lock().await.i_paused_the_media = false;
            } else if !old_all_out {
                debug!("Pausing media as buds are not fully in ear");
                self.pause().await;
                self.state.lock().await.i_paused_the_media = true;
            } else {
                debug!("Playing media");
                self.resume().await;
                self.state.lock().await.i_paused_the_media = false;
            }
        }

        {
            let mut state = self.state.lock().await;
            state.old_in_ear_data = new_in_ear_data;
            debug!("Updated old_in_ear_data to {:?}", state.old_in_ear_data);
        }
    }

    pub async fn activate_a2dp_profile(&self) {
        debug!("Entering activate_a2dp_profile");
        // Every blocking closure keeps a reference to the guard, so the lock stays held until the
        // blocking work really ends, even if this task is aborted mid-await.
        let audio_op = Arc::new(self.audio_op.clone().lock_owned().await);
        let mac = self.state.lock().await.connected_device_mac.clone();
        if mac.is_empty() {
            warn!("Connected device MAC is empty, cannot activate A2DP profile");
            return;
        }

        let mut connect_attempted = false;
        let mut polls = 0;
        loop {
            let backend = self.backend.clone();
            let mac_clone = mac.clone();
            let held = audio_op.clone();
            let (action, card_index) = tokio::task::spawn_blocking(move || {
                let _held = held;
                let cards = backend.cards();
                let card = find_bluez_card(&cards, &mac_clone);
                (plan_a2dp_activation(card), card.map(|c| c.index))
            })
            .await
            .unwrap_or((A2dpAction::NoCard, None));

            match action {
                A2dpAction::AlreadyActive => {
                    debug!("A2DP profile already active for {}", mac);
                    return;
                }
                A2dpAction::SetProfile(profile) => {
                    let Some(index) = card_index else { return };
                    info!("Activating A2DP profile for AirPods: {}", profile);
                    let backend = self.backend.clone();
                    let profile_clone = profile.clone();
                    let held = audio_op.clone();
                    let success = tokio::task::spawn_blocking(move || {
                        let _held = held;
                        backend.set_card_profile(index, &profile_clone)
                    })
                    .await
                    .unwrap_or(false);
                    if success {
                        info!("Successfully activated A2DP profile: {}", profile);
                    } else {
                        warn!("Failed to activate A2DP profile: {}", profile);
                    }
                    return;
                }
                A2dpAction::ConnectProfile | A2dpAction::NoCard => {
                    if !connect_attempted {
                        connect_attempted = true;
                        info!("A2DP not available for {}, asking BlueZ to connect it", mac);
                        let backend = self.backend.clone();
                        let mac_clone = mac.clone();
                        let held = audio_op.clone();
                        tokio::task::spawn_blocking(move || {
                            let _held = held;
                            backend.connect_a2dp(&mac_clone)
                        })
                            .await
                            .unwrap_or(false);
                        continue;
                    }
                    if polls >= self.a2dp_wait_attempts {
                        warn!("A2DP still not available for {}, giving up", mac);
                        return;
                    }
                    polls += 1;
                    tokio::time::sleep(self.a2dp_wait_interval).await;
                }
            }
        }
    }

    async fn pause(&self) {
        debug!("Pausing playback");

        let paused_services = tokio::task::spawn_blocking(|| {
            let conn = match Connection::new_session() {
                Ok(c) => c,
                Err(_) => return vec![],
            };
            let mut paused_services = Vec::new();

            for service in Self::list_mpris_services(&conn) {
                debug!("Checking playback status for service: {}", service);
                let proxy = conn.with_proxy(
                    &service,
                    "/org/mpris/MediaPlayer2",
                    Duration::from_secs(5),
                );

                if let Ok(playback_status) =
                    proxy.get::<String>("org.mpris.MediaPlayer2.Player", "PlaybackStatus")
                    && playback_status == "Playing"
                {
                    if proxy
                        .method_call::<(), _, &str, &str>(
                            "org.mpris.MediaPlayer2.Player",
                            "Pause",
                            (),
                        )
                        .is_ok()
                    {
                        info!("Paused playback for: {}", service);
                        paused_services.push(service);
                    } else {
                        error!("Failed to pause {}", service);
                    }
                }
            }

            paused_services
        })
            .await
            .unwrap();

        if !paused_services.is_empty() {
            info!("Paused {} media player(s) via DBus", paused_services.len());
            let mut state = self.state.lock().await;
            state.paused_by_app_services = paused_services;
            state.is_playing = false;
        } else {
            info!("No playing media players found to pause");
        }
    }

    pub async fn pause_all_media(&self) {
        debug!("Pausing all media (without tracking for resume)");

        let paused_count = tokio::task::spawn_blocking(|| {
            let conn = match Connection::new_session() {
                Ok(c) => c,
                Err(_) => return 0,
            };
            let mut paused_count = 0;

            for service in Self::list_mpris_services(&conn) {
                let proxy =
                    conn.with_proxy(&service, "/org/mpris/MediaPlayer2", Duration::from_secs(5));
                if let Ok(playback_status) =
                    proxy.get::<String>("org.mpris.MediaPlayer2.Player", "PlaybackStatus")
                    && playback_status == "Playing"
                {
                    if proxy
                        .method_call::<(), _, &str, &str>(
                            "org.mpris.MediaPlayer2.Player",
                            "Pause",
                            (),
                        )
                        .is_ok()
                    {
                        info!("Paused playback for: {}", service);
                        paused_count += 1;
                    } else {
                        error!("Failed to pause {}", service);
                    }
                }
            }
            paused_count
        })
            .await
            .unwrap();

        if paused_count > 0 {
            info!("Paused {} media player(s) due to ownership loss", paused_count);
            self.state.lock().await.is_playing = false;
        } else {
            debug!("No playing media players found to pause");
        }
    }

    async fn resume(&self) {
        debug!("Resuming playback");
        let services = self.state.lock().await.paused_by_app_services.clone();

        if services.is_empty() {
            debug!("No services to resume");
            return;
        }

        let resumed_count = tokio::task::spawn_blocking(move || {
            let conn = match Connection::new_session() {
                Ok(c) => c,
                Err(_) => return 0,
            };
            let mut resumed_count = 0;
            for service in services {
                let proxy =
                    conn.with_proxy(&service, "/org/mpris/MediaPlayer2", Duration::from_secs(5));
                if proxy
                    .method_call::<(), _, &str, &str>("org.mpris.MediaPlayer2.Player", "Play", ())
                    .is_ok()
                {
                    info!("Resumed playback for: {}", service);
                    resumed_count += 1;
                } else {
                    warn!("Failed to resume {}", service);
                }
            }
            resumed_count
        })
            .await
            .unwrap();

        if resumed_count > 0 {
            info!("Resumed {} media player(s) via DBus", resumed_count);
            self.state.lock().await.paused_by_app_services.clear();
        } else {
            error!("Failed to resume any media players via DBus");
        }
    }

    pub async fn next_track(&self) {
        info!("Skipping to next track");
        self.mpris_command("Next").await;
    }

    pub async fn previous_track(&self) {
        info!("Going to previous track");
        self.mpris_command("Previous").await;
    }

    pub async fn deactivate_a2dp_profile(&self) {
        debug!("Entering deactivate_a2dp_profile");
        let audio_op = self.audio_op.clone().lock_owned().await;
        let mac = self.state.lock().await.connected_device_mac.clone();
        if mac.is_empty() {
            warn!("Connected device MAC is empty, cannot deactivate A2DP profile");
            return;
        }

        let backend = self.backend.clone();
        let success = tokio::task::spawn_blocking(move || {
            let _audio_op = audio_op;
            let cards = backend.cards();
            match find_bluez_card(&cards, &mac) {
                Some(card) if card.active_profile.as_deref() != Some("off") => {
                    info!("Deactivating A2DP profile for AirPods by setting to off");
                    Some(backend.set_card_profile(card.index, "off"))
                }
                Some(_) => None,
                None => {
                    debug!("No audio card for {}, nothing to deactivate", mac);
                    None
                }
            }
        })
        .await
        .unwrap_or(Some(false));

        match success {
            Some(true) => info!("Successfully deactivated A2DP profile"),
            Some(false) => warn!("Failed to deactivate A2DP profile"),
            None => {}
        }
    }

    pub async fn handle_conversational_awareness(&self, status: u8) {
        debug!("Entering handle_conversational_awareness with status: {}", status);

        let mac = self.state.lock().await.connected_device_mac.clone();
        if mac.is_empty() {
            debug!("No connected device MAC, skipping conversational awareness");
            return;
        }

        let sink = match get_sink_name_by_mac(&mac).await {
            Some(s) => s,
            None => {
                warn!(
                    "Could not find sink for MAC {}, skipping conversational awareness",
                    mac
                );
                return;
            }
        };

        let current_volume_opt = tokio::task::spawn_blocking({
            let sink = sink.clone();
            move || get_sink_volume_percent_by_name_sync(&sink)
        })
            .await
            .unwrap_or(None);

        match status {
            1 => {
                let original = current_volume_opt.unwrap_or(0);
                debug!("Conversation start (1). Current volume: {}", original);
                {
                    let mut state = self.state.lock().await;
                    if !state.conv_conversation_started {
                        state.conv_original_volume = Some(original);
                        state.conv_conversation_started = true;
                    } else {
                        debug!(
                            "Conversation already started; not overwriting conv_original_volume"
                        );
                    }
                }
                if original > 25 {
                    let sink_clone = sink.clone();
                    tokio::task::spawn_blocking(move || transition_sink_volume(&sink_clone, 25))
                        .await
                        .unwrap_or(false);
                    info!(
                        "Conversation start: lowered volume to 25% (original {})",
                        original
                    );
                } else {
                    debug!("Original volume {} <= 25, not reducing to 25", original);
                }
            }
            2 => {
                let original = self.state.lock().await.conv_original_volume;
                if let Some(orig) = original {
                    debug!("Conversation reduce (2). Original: {}", orig);
                    if orig > 15 {
                        let sink_clone = sink.clone();
                        tokio::task::spawn_blocking(move || {
                            transition_sink_volume(&sink_clone, 15)
                        })
                            .await
                            .unwrap_or(false);
                        info!(
                            "Conversation reduce: lowered volume to 15% (original {})",
                            orig
                        );
                    } else {
                        debug!("Original {} <= 15, not reducing to 15", orig);
                    }
                } else {
                    debug!("No original volume known for status 2, skipping");
                }
            }
            3 => {
                let (conv_started, conv_original) = {
                    let state = self.state.lock().await;
                    (state.conv_conversation_started, state.conv_original_volume)
                };
                if !conv_started {
                    debug!("Received status 3 but conversation was not started; ignoring increase");
                    return;
                }
                if let Some(orig) = conv_original {
                    let target = orig.min(25);
                    let sink_clone = sink.clone();
                    tokio::task::spawn_blocking(move || transition_sink_volume(&sink_clone, target))
                        .await
                        .unwrap_or(false);
                    info!(
                        "Conversation partial increase (3): set volume to {} (original {})",
                        target, orig
                    );
                } else if let Some(orig_from_current) = current_volume_opt {
                    let target = orig_from_current.min(25);
                    let sink_clone = sink.clone();
                    tokio::task::spawn_blocking(move || transition_sink_volume(&sink_clone, target))
                        .await
                        .unwrap_or(false);
                    info!(
                        "Conversation partial increase (3) with fallback current: set volume to {} (measured {})",
                        target, orig_from_current
                    );
                } else {
                    debug!("No original volume known for status 3, skipping");
                }
            }
            4 | 6 | 7 => {
                debug!("Conversation end ({}), restoring volume if needed", status);
                self.restore_volume_if_needed(&sink).await;
            }
            _ => {
                debug!("Conversation status ({}), ignoring", status);
            }
        }
    }

    async fn mpris_command(&self, command: &'static str) {
        tokio::task::spawn_blocking(move || {
            let conn = match Connection::new_session() {
                Ok(c) => c,
                Err(_) => return,
            };

            let services = Self::list_mpris_services(&conn);
            let mut playing = None;
            let mut fallback = None;

            for service in &services {
                let proxy =
                    conn.with_proxy(service, "/org/mpris/MediaPlayer2", Duration::from_secs(5));
                if let Ok(status) =
                    proxy.get::<String>("org.mpris.MediaPlayer2.Player", "PlaybackStatus")
                {
                    if status == "Playing" && playing.is_none() {
                        playing = Some(service);
                    }
                }
                if fallback.is_none() {
                    fallback = Some(service);
                }
            }

            if let Some(service) = playing.or(fallback) {
                let proxy =
                    conn.with_proxy(service, "/org/mpris/MediaPlayer2", Duration::from_secs(5));
                if proxy
                    .method_call::<(), _, &str, &str>(
                        "org.mpris.MediaPlayer2.Player",
                        command,
                        (),
                    )
                    .is_ok()
                {
                    info!("Sent {} to: {}", command, service);
                } else {
                    debug!("Failed to send {} to: {}", command, service);
                }
            }
        })
            .await
            .unwrap();
    }

    async fn restore_volume_if_needed(&self, sink: &str) {
        let maybe_original = {
            let mut state = self.state.lock().await;
            if state.conv_conversation_started {
                let orig = state.conv_original_volume;
                state.conv_original_volume = None;
                state.conv_conversation_started = false;
                orig
            } else {
                return;
            }
        };

        if let Some(orig) = maybe_original {
            let sink = sink.to_string();
            tokio::task::spawn_blocking(move || transition_sink_volume(&sink, orig))
                .await
                .unwrap_or(false);
        }
    }

    fn list_mpris_services(conn: &Connection) -> Vec<String> {
        let proxy = conn.with_proxy(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            Duration::from_secs(5),
        );

        let (names,): (Vec<String>,) =
            match proxy.method_call("org.freedesktop.DBus", "ListNames", ()) {
                Ok(n) => n,
                Err(_) => return vec![],
            };

        names
            .into_iter()
            .filter(|s| {
                s.starts_with("org.mpris.MediaPlayer2.") && !Self::is_kdeconnect_service(s)
            })
            .collect()
    }
}

/// Take the AirPods over only when playback just started and at least one bud is in an ear.
pub fn should_take_over(was_playing: bool, is_playing: bool, ear: &[EarDetectionStatus]) -> bool {
    !was_playing && is_playing && ear.contains(&EarDetectionStatus::InEar)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{CardSnapshot, ProfileSnapshot};
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const MAC: &str = "8C:7A:AA:D0:3B:50";

    #[derive(Default)]
    struct FakeBackend {
        cards: StdMutex<Vec<CardSnapshot>>,
        cards_after_connect: Option<Vec<CardSnapshot>>,
        set_calls: StdMutex<Vec<(u32, String)>>,
        connect_calls: AtomicUsize,
        set_delay: Duration,
    }

    impl AudioBackend for FakeBackend {
        fn cards(&self) -> Vec<CardSnapshot> {
            self.cards.lock().unwrap().clone()
        }

        fn set_card_profile(&self, card_index: u32, profile: &str) -> bool {
            std::thread::sleep(self.set_delay);
            self.set_calls
                .lock()
                .unwrap()
                .push((card_index, profile.to_string()));
            for card in self.cards.lock().unwrap().iter_mut() {
                if card.index == card_index {
                    card.active_profile = Some(profile.to_string());
                }
            }
            true
        }

        fn connect_a2dp(&self, _mac: &str) -> bool {
            self.connect_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(cards) = &self.cards_after_connect {
                *self.cards.lock().unwrap() = cards.clone();
            }
            true
        }
    }

    fn airpods_card(active: &str, sbc_xq_available: bool) -> CardSnapshot {
        CardSnapshot {
            index: 1043,
            name: "bluez_card.8C_7A_AA_D0_3B_50".to_string(),
            device_string: Some(MAC.to_string()),
            active_profile: Some(active.to_string()),
            profiles: vec![
                ProfileSnapshot { name: "off".to_string(), available: true },
                ProfileSnapshot {
                    name: "a2dp-sink-sbc_xq".to_string(),
                    available: sbc_xq_available,
                },
                ProfileSnapshot { name: "a2dp-sink".to_string(), available: true },
            ],
        }
    }

    fn hdmi_card() -> CardSnapshot {
        CardSnapshot {
            index: 940,
            name: "alsa_card.pci-0000_01_00.1".to_string(),
            device_string: None,
            active_profile: Some("output:hdmi-stereo".to_string()),
            profiles: vec![],
        }
    }

    fn controller(backend: Arc<FakeBackend>) -> MediaController {
        MediaController::new(MAC.to_string(), "B0:60:88:AF:DD:DA".to_string(), backend)
            .with_a2dp_wait(3, Duration::from_millis(1))
    }

    fn backend_with(cards: Vec<CardSnapshot>) -> Arc<FakeBackend> {
        Arc::new(FakeBackend {
            cards: StdMutex::new(cards),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn activate_sets_preferred_profile_once() {
        let backend = backend_with(vec![airpods_card("off", true)]);
        controller(backend.clone()).activate_a2dp_profile().await;
        assert_eq!(
            *backend.set_calls.lock().unwrap(),
            vec![(1043, "a2dp-sink-sbc_xq".to_string())]
        );
        assert_eq!(backend.connect_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn activate_does_nothing_when_a2dp_already_active() {
        let backend = backend_with(vec![airpods_card("a2dp-sink", true)]);
        controller(backend.clone()).activate_a2dp_profile().await;
        assert!(backend.set_calls.lock().unwrap().is_empty());
        assert_eq!(backend.connect_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn activate_connects_profile_via_bluez_when_missing_then_sets_profile() {
        let backend = Arc::new(FakeBackend {
            cards_after_connect: Some(vec![airpods_card("off", true)]),
            ..Default::default()
        });
        controller(backend.clone()).activate_a2dp_profile().await;
        assert_eq!(backend.connect_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            *backend.set_calls.lock().unwrap(),
            vec![(1043, "a2dp-sink-sbc_xq".to_string())]
        );
    }

    #[tokio::test]
    async fn activate_gives_up_without_restarting_anything() {
        let backend = backend_with(vec![]);
        controller(backend.clone()).activate_a2dp_profile().await;
        assert_eq!(backend.connect_calls.load(Ordering::SeqCst), 1);
        assert!(backend.set_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_activations_set_profile_once() {
        let backend = backend_with(vec![airpods_card("off", true)]);
        let mc = controller(backend.clone());
        let (a, b, c) = (mc.clone(), mc.clone(), mc.clone());
        tokio::join!(
            a.activate_a2dp_profile(),
            b.activate_a2dp_profile(),
            c.activate_a2dp_profile()
        );
        assert_eq!(backend.set_calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn deactivate_only_touches_airpods_card() {
        let backend = backend_with(vec![hdmi_card(), airpods_card("a2dp-sink-sbc_xq", true)]);
        controller(backend.clone()).deactivate_a2dp_profile().await;
        assert_eq!(
            *backend.set_calls.lock().unwrap(),
            vec![(1043, "off".to_string())]
        );
    }

    #[test]
    fn takes_over_on_rising_edge_with_a_bud_in_ear() {
        use EarDetectionStatus::*;
        assert!(should_take_over(false, true, &[InEar, OutOfEar]));
    }

    #[test]
    fn no_takeover_when_buds_out() {
        use EarDetectionStatus::*;
        assert!(!should_take_over(false, true, &[OutOfEar, InCase]));
    }

    #[test]
    fn no_takeover_when_already_playing() {
        use EarDetectionStatus::*;
        assert!(!should_take_over(true, true, &[InEar, InEar]));
    }

    #[test]
    fn no_takeover_without_ear_data() {
        assert!(!should_take_over(false, true, &[]));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_off_cannot_land_after_new_activation() {
        // A MAC no other test uses, so only these two controllers share its audio lock.
        const STALE_MAC: &str = "11:22:33:44:55:66";
        let mut card = airpods_card("a2dp-sink-sbc_xq", true);
        card.name = "bluez_card.11_22_33_44_55_66".to_string();
        card.device_string = Some(STALE_MAC.to_string());
        let backend = Arc::new(FakeBackend {
            cards: StdMutex::new(vec![card]),
            set_delay: Duration::from_millis(100),
            ..Default::default()
        });
        let make = || {
            MediaController::new(
                STALE_MAC.to_string(),
                "B0:60:88:AF:DD:DA".to_string(),
                backend.clone(),
            )
            .with_a2dp_wait(3, Duration::from_millis(1))
        };
        let (old, new) = (make(), make());

        let old_task = tokio::spawn(async move { old.deactivate_a2dp_profile().await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        old_task.abort(); // the old instance is torn down mid-"off"
        new.activate_a2dp_profile().await;
        // Let any blocking work the aborted instance left behind finish before looking.
        tokio::time::sleep(Duration::from_millis(250)).await;

        let active = backend.cards.lock().unwrap()[0].active_profile.clone();
        assert_eq!(active.as_deref(), Some("a2dp-sink-sbc_xq"));
    }

    #[tokio::test]
    async fn playback_listener_stops_when_aacp_disconnected() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let finished = tokio::time::timeout(
            Duration::from_secs(3),
            controller(backend_with(vec![])).run_playback_listener(AACPManager::new(), tx),
        )
        .await;
        assert!(finished.is_ok(), "listener kept running with no AACP connection");
    }

    #[tokio::test]
    async fn deactivate_without_card_does_nothing() {
        let backend = backend_with(vec![]);
        controller(backend.clone()).deactivate_a2dp_profile().await;
        assert!(backend.set_calls.lock().unwrap().is_empty());
        assert_eq!(backend.connect_calls.load(Ordering::SeqCst), 0);
    }
}
