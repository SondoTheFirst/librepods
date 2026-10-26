//! The AirPods page: every control this model supports, drawn from the catalog in
//! `devices::settings`, plus the name and the device information.

use crate::bluetooth::aacp::ControlCommandIdentifiers;
use crate::devices::enums::{AirPodsState, DeviceData, DeviceInformation};
use crate::devices::settings::{
    ChoiceOption, ControlKind, ControlSpec, Section, decode_toggle, encode_per_bud,
    encode_slider, encode_toggle, per_bud_values, rename_request, selected_choice, slider_value,
    visible_controls,
};
use crate::ui::window::Message;
use iced::Alignment::End;
use iced::border::Radius;
use iced::widget::button::Style;
use iced::widget::rule::FillMode;
use iced::widget::{
    Space, button, column, container, pick_list, row, rule, scrollable, slider, text, text_input,
    toggler,
};
use iced::{Background, Border, Center, Color, Element, Length, Padding, Theme};
use log::error;
use std::collections::HashMap;

/// Catalog sections in page order.
const SECTIONS: [Section; 8] = [
    Section::Audio,
    Section::PressAndHold,
    Section::Calls,
    Section::Microphone,
    Section::Connection,
    Section::Battery,
    Section::Accessibility,
    Section::Advanced,
];

/// Press-and-hold value for a bud the AirPods haven't reported: Apple's default, Noise Control.
const DEFAULT_PRESS_AND_HOLD: u8 = 0x01;

fn section_title(section: Section) -> &'static str {
    match section {
        Section::Audio => "Audio",
        Section::PressAndHold => "Press and Hold",
        Section::Calls => "Calls",
        Section::Microphone => "Microphone",
        Section::Connection => "Connection",
        Section::Battery => "Battery",
        Section::Accessibility => "Accessibility",
        Section::Advanced => "Advanced",
    }
}

/// One pick-list entry: the label shown and the bytes it sends.
#[derive(Debug, Clone, PartialEq)]
struct ChoiceItem {
    label: &'static str,
    value: &'static [u8],
}

impl std::fmt::Display for ChoiceItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label)
    }
}

impl From<&ChoiceOption> for ChoiceItem {
    fn from(option: &ChoiceOption) -> Self {
        ChoiceItem {
            label: option.label,
            value: option.value,
        }
    }
}

/// What a picker shows when the AirPods' value matches no option.
fn placeholder_for(value: Option<&[u8]>) -> String {
    match value {
        Some(value) => format!("Unknown ({})", hex::encode(value)),
        None => "Not reported".to_string(),
    }
}

pub fn airpods_view<'a>(
    mac: &'a str,
    devices_list: &HashMap<String, DeviceData>,
    state: &'a AirPodsState,
) -> iced::widget::Container<'a, Message> {
    let controls = visible_controls(state.model_or_unknown(), state.firmware_major);
    let mut page = column![name_row(mac, state)].spacing(20);

    for section in SECTIONS {
        let rows: Vec<Element<'a, Message>> = controls
            .iter()
            .filter(|spec| spec.section == section)
            .map(|spec| control_row(mac, spec, state))
            .collect();
        if rows.is_empty() {
            continue;
        }
        page = page.push(if section == Section::Advanced {
            advanced_section(mac, state, rows)
        } else {
            section_card(section_title(section), rows)
        });
    }

    if let Some(information) = information_section(mac, devices_list) {
        page = page.push(information);
    }

    container(scrollable(container(page).padding(20).width(Length::Fill)))
        .width(Length::Fill)
        .height(Length::Fill)
}

fn name_row<'a>(mac: &'a str, state: &'a AirPodsState) -> Element<'a, Message> {
    let on_input = {
        let mac = mac.to_string();
        move |draft| Message::NameDraft(mac.clone(), draft)
    };
    let rename = rename_request(&state.device_name, &state.name_draft)
        .map(|_| Message::SubmitName(mac.to_string()));
    let input = text_input("AirPods name", &state.name_draft)
        .on_input(on_input)
        .on_submit(Message::SubmitName(mac.to_string()))
        .padding(Padding {
            top: 5.0,
            bottom: 5.0,
            left: 10.0,
            right: 10.0,
        })
        .style(|theme: &Theme, _status| text_input::Style {
            background: Background::Color(Color::TRANSPARENT),
            border: Default::default(),
            icon: Default::default(),
            placeholder: theme.palette().text.scale_alpha(0.7),
            value: theme.palette().text,
            selection: theme.palette().primary.scale_alpha(0.5),
        })
        .align_x(End);
    let rename_button = button(text("Rename").size(14))
        .on_press_maybe(rename)
        .padding(Padding {
            top: 4.0,
            bottom: 4.0,
            left: 10.0,
            right: 10.0,
        })
        .style(|theme: &Theme, _status| {
            let mut style = Style::default();
            style.text_color = theme.palette().text;
            style.background = Some(Background::Color(theme.palette().primary.scale_alpha(0.3)));
            style.border = Border::default().rounded(8.0);
            style
        });
    card(
        row![
            text("Name").size(16),
            Space::new().width(Length::Fill),
            input,
            rename_button
        ]
        .align_y(Center)
        .spacing(8),
    )
}

fn control_row<'a>(
    mac: &'a str,
    spec: &'static ControlSpec,
    state: &'a AirPodsState,
) -> Element<'a, Message> {
    let id = spec.id;
    let reported = state.controls.get(&id).map(Vec::as_slice);
    match &spec.kind {
        ControlKind::Toggle => {
            let value = reported.and_then(decode_toggle);
            // The app's own ear-detection preference stands in until the AirPods report theirs.
            let value = if id == ControlCommandIdentifiers::EarDetectionConfig {
                value.unwrap_or(state.prefs.ear_detection)
            } else {
                value.unwrap_or(false)
            };
            let mac = mac.to_string();
            toggle_row(spec.label, spec.description, value, move |on| {
                Message::SetControl(mac.clone(), id, encode_toggle(on))
            })
        }
        ControlKind::Choice(options) => {
            let items: Vec<ChoiceItem> = options.iter().map(ChoiceItem::from).collect();
            let selected = reported
                .and_then(|value| selected_choice(options, value))
                .and_then(|index| items.get(index).cloned());
            let mac = mac.to_string();
            let picker = pick_list(items, selected, move |item: ChoiceItem| {
                Message::SetControl(mac.clone(), id, item.value.to_vec())
            })
            .placeholder(placeholder_for(reported))
            .width(Length::Fixed(280.0));
            labeled_row(spec.label, spec.description, picker)
        }
        ControlKind::Slider { max, suffix } => {
            let (max, suffix) = (*max, *suffix);
            let value = reported.and_then(slider_value).map(|v| v.min(max));
            let draft_mac = mac.to_string();
            let control = slider(0..=max, value.unwrap_or(max / 2), move |v| {
                Message::ControlDraft(draft_mac.clone(), id, encode_slider(max, suffix, v))
            })
            .on_release(Message::SendControl(mac.to_string(), id))
            .width(Length::Fixed(200.0));
            let shown = value.map_or_else(|| "—".to_string(), |v| v.to_string());
            labeled_row(
                spec.label,
                spec.description,
                row![control, text(shown).size(14).width(Length::Fixed(32.0))]
                    .spacing(8)
                    .align_y(Center),
            )
        }
        ControlKind::PerBud(options) => {
            let (left, right) = reported.map(per_bud_values).unwrap_or((None, None));
            let items: Vec<ChoiceItem> = options.iter().map(ChoiceItem::from).collect();
            let item_for = |byte: Option<u8>| {
                byte.and_then(|b| options.iter().find(|option| option.value == [b].as_slice()))
                    .map(ChoiceItem::from)
            };
            let first_byte = |item: &ChoiceItem| {
                item.value.first().copied().unwrap_or(DEFAULT_PRESS_AND_HOLD)
            };
            let other_right = right.unwrap_or(DEFAULT_PRESS_AND_HOLD);
            let other_left = left.unwrap_or(DEFAULT_PRESS_AND_HOLD);
            let left_mac = mac.to_string();
            let right_mac = mac.to_string();
            let left_picker = pick_list(items.clone(), item_for(left), move |item: ChoiceItem| {
                Message::SetControl(left_mac.clone(), id, encode_per_bud(first_byte(&item), other_right))
            })
            .placeholder(placeholder_for(left.map(|b| [b]).as_ref().map(|b| b.as_slice())))
            .width(Length::Fixed(160.0));
            let right_picker = pick_list(items, item_for(right), move |item: ChoiceItem| {
                Message::SetControl(right_mac.clone(), id, encode_per_bud(other_left, first_byte(&item)))
            })
            .placeholder(placeholder_for(right.map(|b| [b]).as_ref().map(|b| b.as_slice())))
            .width(Length::Fixed(160.0));
            labeled_row(
                spec.label,
                spec.description,
                column![
                    row![text("Left").size(14).width(Length::Fixed(44.0)), left_picker].align_y(Center),
                    row![text("Right").size(14).width(Length::Fixed(44.0)), right_picker].align_y(Center),
                ]
                .spacing(6),
            )
        }
    }
}

fn label_column<'a>(label: &'a str, description: &'a str) -> iced::widget::Column<'a, Message> {
    let mut labels = column![text(label).size(16)];
    if !description.is_empty() {
        labels = labels.push(
            text(description)
                .size(12)
                .style(|theme: &Theme| {
                    let mut style = text::Style::default();
                    style.color = Some(theme.palette().text.scale_alpha(0.7));
                    style
                })
                .width(Length::Fill),
        );
    }
    labels.width(Length::Fill)
}

fn toggle_row<'a>(
    label: &'a str,
    description: &'a str,
    value: bool,
    on_toggle: impl Fn(bool) -> Message + 'a,
) -> Element<'a, Message> {
    row![
        label_column(label, description),
        toggler(value).on_toggle(on_toggle).spacing(0).size(20)
    ]
    .align_y(Center)
    .spacing(8)
    .into()
}

fn labeled_row<'a>(
    label: &'a str,
    description: &'a str,
    control: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    row![label_column(label, description), control.into()]
        .align_y(Center)
        .spacing(12)
        .into()
}

fn divider<'a>() -> Element<'a, Message> {
    rule::horizontal(1)
        .style(|theme: &Theme| rule::Style {
            color: theme.palette().text.scale_alpha(0.2),
            radius: Radius::from(12),
            fill_mode: FillMode::Full,
            snap: false,
        })
        .into()
}

fn heading<'a>(title: &'a str) -> Element<'a, Message> {
    container(text(title).size(18).style(|theme: &Theme| {
        let mut style = text::Style::default();
        style.color = Some(theme.palette().primary);
        style
    }))
    .padding(Padding {
        top: 5.0,
        bottom: 5.0,
        left: 18.0,
        right: 18.0,
    })
    .into()
}

fn card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .padding(Padding {
            top: 5.0,
            bottom: 5.0,
            left: 10.0,
            right: 10.0,
        })
        .width(Length::Fill)
        .style(|theme: &Theme| {
            let mut style = container::Style::default();
            style.background = Some(Background::Color(theme.palette().primary.scale_alpha(0.1)));
            let mut border = Border::default();
            border.color = theme.palette().primary.scale_alpha(0.5);
            style.border = border.rounded(16);
            style
        })
        .into()
}

fn rows_card<'a>(rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let mut body = column![].spacing(8).padding(8);
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            body = body.push(divider());
        }
        body = body.push(row);
    }
    card(body)
}

fn section_card<'a>(title: &'a str, rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    column![heading(title), rows_card(rows)].spacing(4).into()
}

fn advanced_section<'a>(
    mac: &'a str,
    state: &'a AirPodsState,
    rows: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    let toggle = button(text(if state.show_advanced { "Hide" } else { "Show" }).size(14))
        .on_press(Message::ToggleAdvanced(mac.to_string()))
        .padding(Padding {
            top: 4.0,
            bottom: 4.0,
            left: 10.0,
            right: 10.0,
        })
        .style(|theme: &Theme, _status| {
            let mut style = Style::default();
            style.text_color = theme.palette().text;
            style.background = Some(Background::Color(theme.palette().primary.scale_alpha(0.2)));
            style.border = Border::default().rounded(8.0);
            style
        });
    let mut section = column![
        row![
            heading(section_title(Section::Advanced)),
            Space::new().width(Length::Fill),
            toggle
        ]
        .align_y(Center)
    ]
    .spacing(4);
    if state.show_advanced {
        section = section
            .push(
                container(text("These may not be supported by your AirPods.").size(12)).padding(
                    Padding {
                        top: 0.0,
                        bottom: 0.0,
                        left: 18.0,
                        right: 18.0,
                    },
                ),
            )
            .push(rows_card(rows));
    }
    section.into()
}

fn information_section<'a>(
    mac: &str,
    devices_list: &HashMap<String, DeviceData>,
) -> Option<Element<'a, Message>> {
    let device = devices_list.get(mac)?;
    let Some(DeviceInformation::AirPods(info)) = &device.information else {
        if device.information.is_some() {
            error!("Expected AirPodsInformation for device {}, got something else", mac);
        }
        return None;
    };
    let plain = |label: &'static str, value: String| -> Element<'a, Message> {
        row![text(label).size(16), Space::new().width(Length::Fill), text(value).size(16)].into()
    };
    let copyable = |label: &'static str, value: String| -> Element<'a, Message> {
        row![
            text(label).size(16),
            Space::new().width(Length::Fill),
            button(text(value.clone()).size(16))
                .style(|theme: &Theme, _status| {
                    let mut style = Style::default();
                    style.text_color = theme.palette().text;
                    style.background = Some(Background::Color(Color::TRANSPARENT));
                    style
                })
                .padding(0)
                .on_press(Message::CopyToClipboard(value))
        ]
        .into()
    };
    let rows = column![
        plain("Model Number", info.model_number.clone()),
        plain("Manufacturer", info.manufacturer.clone()),
        copyable("Serial Number", info.serial_number.clone()),
        copyable("Left Serial Number", info.left_serial_number.clone()),
        copyable("Right Serial Number", info.right_serial_number.clone()),
        plain("Version 1", info.version1.clone()),
        plain("Version 2", info.version2.clone()),
        plain("Version 3", info.version3.clone()),
    ]
    .spacing(4)
    .padding(8);
    Some(column![heading("Device Information"), card(rows)].spacing(4).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bluetooth::aacp::ControlCommandIdentifiers as Id;
    use crate::devices::models::AirPodsModel;
    use crate::prefs::DevicePrefs;

    const MAC: &str = "AA:BB:CC:DD:EE:01";

    fn sample_state() -> AirPodsState {
        AirPodsState {
            device_name: "Test AirPods Pro".to_string(),
            name_draft: "Test AirPods Pro".to_string(),
            battery: vec![],
            // Includes values the catalog doesn't know: call config 00 01, mic mode 07.
            controls: HashMap::from([
                (Id::CallManagementConfig, vec![0x00, 0x01]),
                (Id::MicMode, vec![0x07]),
                (Id::ChimeVolume, vec![0x50, 0x50]),
                (Id::ClickHoldMode, vec![0x01, 0x05]),
                (Id::ListeningMode, vec![0x02]),
            ]),
            model: None,
            firmware_major: None,
            custom_eq: None,
            accommodation: None,
            prefs: DevicePrefs::default(),
            show_advanced: false,
        }
    }

    #[test]
    fn view_builds_for_every_model() {
        use AirPodsModel::*;
        let models = [
            None,
            Some(AirPods1),
            Some(AirPods2),
            Some(AirPods3),
            Some(AirPods4),
            Some(AirPods4Anc),
            Some(AirPods5),
            Some(AirPods5Wcc),
            Some(AirPodsPro1),
            Some(AirPodsPro2Lightning),
            Some(AirPodsPro2UsbC),
            Some(AirPodsPro3),
            Some(AirPodsMaxLightning),
            Some(AirPodsMaxUsbC),
            Some(AirPodsMax2),
            Some(Unknown),
        ];
        for model in models {
            for show_advanced in [false, true] {
                let mut state = sample_state();
                state.model = model;
                state.firmware_major = Some(9);
                state.show_advanced = show_advanced;
                let _ = airpods_view(MAC, &HashMap::new(), &state);
            }
        }
    }
}
