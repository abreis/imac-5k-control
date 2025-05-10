#[derive(Copy, Clone)]
pub enum OnOff {
    On,
    Off,
}

#[derive(Copy, Clone)]
pub enum ControlMessage {
    ButtonPower,
    ButtonMenu,
    ButtonEnter,
    ButtonDown,
    ButtonUp,
    DisplayPower(OnOff),
    FanPower(OnOff),
}
