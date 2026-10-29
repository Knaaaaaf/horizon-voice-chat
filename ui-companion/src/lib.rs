//! In-game UI companion for Horizon Voice Chat.
//!
//! This is deliberately a UI/settings component. The pinned Horizon 0.1 API
//! does not expose microphone capture or a client/server custom network
//! channel to sandboxed mods, so these controls do not yet start the native
//! cpal backend or transmit voice.

use horizon_mod_sdk::{
    Mod, Registration, input,
    input::Event,
    render::{self, Corner, FillShape, Point, Rect, Shape, TextShape},
    storage,
    ui::{self, ButtonWidget, Screen, TextWidget, Widget},
};

const OPEN_KEY: &str = "voice_menu";
const MIC_BUTTON: u32 = 1;
const MUTE_BUTTON: u32 = 2;
const MODE_BUTTON: u32 = 3;
const DISTANCE_BUTTON: u32 = 4;
const SAVE_BUTTON: u32 = 5;

const BG: u32 = 0xB0182028;
const GREEN: u32 = 0x55DD88;
const RED: u32 = 0xFF6666;
const WHITE: u32 = 0xFFFFFF;

struct VoiceUi {
    mic_enabled: bool,
    muted: bool,
    push_to_talk: bool,
    distance: usize,
}

const DISTANCES: [u32; 4] = [8, 16, 32, 48];

fn stored_bool(key: &str, default: bool) -> bool {
    storage::get(key)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|value| value.trim() == "true")
        .unwrap_or(default)
}

fn stored_distance() -> usize {
    storage::get("distance")
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value < DISTANCES.len())
        .unwrap_or(1)
}

impl VoiceUi {
    fn save(&self) {
        let _ = storage::set("mic_enabled", if self.mic_enabled { b"true" } else { b"false" });
        let _ = storage::set("muted", if self.muted { b"true" } else { b"false" });
        let _ = storage::set("ptt", if self.push_to_talk { b"true" } else { b"false" });
        let _ = storage::set("distance", self.distance.to_string().as_bytes());
    }

    fn widgets(&self) -> Vec<Widget> {
        let state = |value: bool| if value { "ON" } else { "OFF" };
        vec![
            Widget::Text(TextWidget {
                id: 10,
                text: "VOICE CHAT  •  CLIENT CONTROLS".into(),
                color: WHITE,
            }),
            Widget::Text(TextWidget {
                id: 11,
                text: "Audio transport is not connected in this build.".into(),
                color: 0xFFCC66,
            }),
            Widget::Button(ButtonWidget {
                id: MIC_BUTTON,
                label: format!("Microphone: {}", state(self.mic_enabled)),
            }),
            Widget::Button(ButtonWidget {
                id: MUTE_BUTTON,
                label: format!("Mute: {}", state(self.muted)),
            }),
            Widget::Button(ButtonWidget {
                id: MODE_BUTTON,
                label: format!("Mode: {}", if self.push_to_talk { "Push to talk" } else { "Voice activation (UI only)" }),
            }),
            Widget::Button(ButtonWidget {
                id: DISTANCE_BUTTON,
                label: format!("Proximity range: {} blocks", DISTANCES[self.distance]),
            }),
            Widget::Text(TextWidget {
                id: 12,
                text: "Open this menu with V. Rebind it in Options > Mods.".into(),
                color: 0xB8C4D0,
            }),
            Widget::Button(ButtonWidget {
                id: SAVE_BUTTON,
                label: "Save settings".into(),
            }),
        ]
    }

    fn page(&self) {
        ui::set_settings(&self.widgets());
    }

    fn open(&mut self) {
        let _ = ui::open_screen(&Screen {
            title: "Horizon Voice Chat".into(),
            widgets: self.widgets(),
        });
    }

    fn draw_status(&self) {
        let status = if self.muted || !self.mic_enabled { "MIC OFF" } else { "MIC READY" };
        let color = if self.muted || !self.mic_enabled { RED } else { GREEN };
        let width = render::text_width(status) + 28.0;
        render::set_corner(
            Corner::TopRight,
            width,
            18.0,
            &[
                Shape::Fill(FillShape {
                    rect: Rect { x: 0.0, y: 0.0, w: width, h: 18.0 },
                    color: BG,
                }),
                Shape::Fill(FillShape {
                    rect: Rect { x: 4.0, y: 6.0, w: 6.0, h: 6.0 },
                    color,
                }),
                Shape::Text(TextShape {
                    text: status.into(),
                    at: Point { x: 14.0, y: 4.0 },
                    color: WHITE,
                    centered: false,
                    shadow: true,
                }),
            ],
        );
    }

    fn changed(&mut self) {
        self.save();
        self.page();
        self.draw_status();
    }
}

impl Mod for VoiceUi {
    fn init() -> Result<(Self, Registration), String> {
        input::register_key(OPEN_KEY, "Open voice chat menu", "key.keyboard.v")?;
        let this = Self {
            mic_enabled: stored_bool("mic_enabled", false),
            muted: stored_bool("muted", false),
            push_to_talk: stored_bool("ptt", true),
            distance: stored_distance(),
        };
        this.page();
        this.draw_status();
        Ok((this, Registration::new().on_tick()))
    }

    fn tick(&mut self, _tick: u64) {
        for event in input::poll() {
            if let Event::KeyDown(key) = event {
                if key == OPEN_KEY {
                    self.open();
                }
            }
        }
        self.draw_status();
    }

    fn ui_select(&mut self, id: u32, _row: u32) {
        match id {
            MIC_BUTTON => self.mic_enabled = !self.mic_enabled,
            MUTE_BUTTON => self.muted = !self.muted,
            MODE_BUTTON => self.push_to_talk = !self.push_to_talk,
            DISTANCE_BUTTON => self.distance = (self.distance + 1) % DISTANCES.len(),
            SAVE_BUTTON => self.save(),
            _ => return,
        }
        self.changed();
    }

    fn ui_settings_opened(&mut self) {
        self.page();
    }
}

horizon_mod_sdk::export_mod!(VoiceUi);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proximity_choices_are_bounded_and_cycle() {
        assert_eq!(DISTANCES, [8, 16, 32, 48]);
        assert_eq!((3 + 1) % DISTANCES.len(), 0);
    }

    #[test]
    fn menu_widget_ids_are_stable() {
        assert_ne!(MIC_BUTTON, MUTE_BUTTON);
        assert_ne!(MODE_BUTTON, DISTANCE_BUTTON);
        assert_ne!(SAVE_BUTTON, DISTANCE_BUTTON);
    }
}
