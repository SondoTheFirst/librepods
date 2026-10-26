//! Desktop notifications (org.freedesktop.Notifications on the session bus): AirPods
//! connected, and low battery, each replacing its own previous notification.

use crate::bluetooth::aacp::{BatteryComponent, BatteryInfo, BatteryStatus};
use log::warn;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use zbus::zvariant::Value;

/// A bud at or below this level gets one warning.
const LOW_BATTERY: u8 = 20;
/// Back at or above this level (or charging), it may warn again.
const REARM_BATTERY: u8 = 25;

#[derive(Clone)]
pub struct Notifier {
    connection: zbus::Connection,
    /// The notification id last shown under each key, so a new one replaces it.
    ids: Arc<Mutex<HashMap<String, u32>>>,
}

impl Notifier {
    pub async fn connect() -> Option<Self> {
        match zbus::Connection::session().await {
            Ok(connection) => Some(Notifier {
                connection,
                ids: Arc::new(Mutex::new(HashMap::new())),
            }),
            Err(e) => {
                warn!("Desktop notifications are unavailable: {}", e);
                None
            }
        }
    }

    pub async fn show(&self, key: &str, summary: &str, body: &str) {
        let replaces_id = self
            .ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
            .copied()
            .unwrap_or(0);
        let hints: HashMap<&str, Value> = HashMap::new();
        let reply = self
            .connection
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "Notify",
                &(
                    "LibrePods",
                    replaces_id,
                    "audio-headphones",
                    summary,
                    body,
                    Vec::<&str>::new(),
                    hints,
                    -1i32,
                ),
            )
            .await;
        match reply.and_then(|message| message.body().deserialize::<u32>()) {
            Ok(id) => {
                self.ids
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(key.to_string(), id);
            }
            Err(e) => warn!("Could not show a desktop notification: {}", e),
        }
    }
}

fn level_text(battery: &BatteryInfo) -> String {
    let charging = if battery.status == BatteryStatus::Charging { "⚡" } else { "" };
    format!("{}%{}", battery.level, charging)
}

/// "L 94% · R 95%⚡ · Case 80%" (parts that are disconnected are left out); a headset's
/// single battery is just "80%".
pub fn battery_summary(batteries: &[BatteryInfo]) -> String {
    let present = |component: BatteryComponent| {
        batteries
            .iter()
            .find(|b| b.component == component && b.status != BatteryStatus::Disconnected)
    };
    if let Some(headset) = present(BatteryComponent::Headphone) {
        return level_text(headset);
    }
    [
        (BatteryComponent::Left, "L"),
        (BatteryComponent::Right, "R"),
        (BatteryComponent::Case, "Case"),
    ]
    .iter()
    .filter_map(|(component, label)| present(*component).map(|b| format!("{} {}", label, level_text(b))))
    .collect::<Vec<_>>()
    .join(" · ")
}

/// "Logan’s AirPods Pro: left AirPod at 18%".
pub fn low_battery_text(name: &str, battery: &BatteryInfo) -> String {
    let part = match battery.component {
        BatteryComponent::Left => "left AirPod",
        BatteryComponent::Right => "right AirPod",
        BatteryComponent::Headphone => "AirPods",
        BatteryComponent::Case => "case",
    };
    format!("{}: {} at {}%", name, part, battery.level)
}

/// Warns once per bud when it runs low, and again only after it has charged back up.
#[derive(Default)]
pub struct LowBatteryTracker {
    alerted: HashSet<BatteryComponent>,
}

impl LowBatteryTracker {
    /// The buds (or headset) that just became low.
    pub fn check(&mut self, batteries: &[BatteryInfo]) -> Vec<BatteryComponent> {
        let mut newly_low = Vec::new();
        for battery in batteries {
            if battery.component == BatteryComponent::Case
                || battery.status == BatteryStatus::Disconnected
                || battery.level == 0
            {
                continue;
            }
            if battery.status == BatteryStatus::Charging || battery.level >= REARM_BATTERY {
                self.alerted.remove(&battery.component);
            } else if battery.level <= LOW_BATTERY && self.alerted.insert(battery.component) {
                newly_low.push(battery.component);
            }
        }
        newly_low
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bluetooth::aacp::{BatteryComponent, BatteryInfo, BatteryStatus};

    fn battery(component: BatteryComponent, level: u8, status: BatteryStatus) -> BatteryInfo {
        BatteryInfo {
            component,
            level,
            status,
        }
    }

    #[test]
    fn battery_summary_formats() {
        use BatteryComponent::*;
        use BatteryStatus::*;
        assert_eq!(
            battery_summary(&[
                battery(Left, 94, NotCharging),
                battery(Right, 95, Charging),
                battery(Case, 0, Disconnected),
            ]),
            "L 94% · R 95%⚡"
        );
        assert_eq!(
            battery_summary(&[
                battery(Left, 50, Unknown),
                battery(Right, 51, NotCharging),
                battery(Case, 80, NotCharging),
            ]),
            "L 50% · R 51% · Case 80%"
        );
        assert_eq!(battery_summary(&[battery(Headphone, 80, NotCharging)]), "80%");
        assert_eq!(battery_summary(&[]), "");
    }

    #[test]
    fn low_battery_alerts_once_and_rearms() {
        use BatteryComponent::*;
        use BatteryStatus::*;
        let mut tracker = LowBatteryTracker::default();
        assert_eq!(tracker.check(&[battery(Left, 19, NotCharging)]), vec![Left]);
        assert_eq!(tracker.check(&[battery(Left, 18, NotCharging)]), vec![]);
        assert_eq!(tracker.check(&[battery(Left, 30, NotCharging)]), vec![]);
        assert_eq!(tracker.check(&[battery(Left, 19, NotCharging)]), vec![Left]);
        // Between 20 % and 25 % nothing re-arms, so a level that wobbles doesn't nag.
        assert_eq!(tracker.check(&[battery(Left, 22, NotCharging)]), vec![]);
        assert_eq!(tracker.check(&[battery(Left, 19, NotCharging)]), vec![]);
        assert_eq!(tracker.check(&[battery(Right, 15, Charging)]), vec![]);
        assert_eq!(tracker.check(&[battery(Case, 10, NotCharging)]), vec![]);
        assert_eq!(tracker.check(&[battery(Right, 0, Disconnected)]), vec![]);
        assert_eq!(tracker.check(&[battery(Headphone, 12, Unknown)]), vec![Headphone]);
    }

    #[test]
    fn low_battery_text_names_the_bud() {
        use BatteryComponent::*;
        use BatteryStatus::*;
        assert_eq!(
            low_battery_text("Logan’s AirPods Pro", &battery(Left, 18, NotCharging)),
            "Logan’s AirPods Pro: left AirPod at 18%"
        );
        assert_eq!(
            low_battery_text("AirPods", &battery(Right, 9, NotCharging)),
            "AirPods: right AirPod at 9%"
        );
        assert_eq!(
            low_battery_text("Max", &battery(Headphone, 15, NotCharging)),
            "Max: AirPods at 15%"
        );
    }
}
