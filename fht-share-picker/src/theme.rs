// TODO: Make theme a separate library for iced
// I am currently working on one separately but its still not ready to be used.

pub mod container {
    use iced::widget::container::Style;
    use iced::{color, Background};

    pub fn surface() -> Style {
        Style {
            background: Some(Background::Color(color!(0x101115))),
            ..Default::default()
        }
    }

    pub fn default() -> Style {
        Style {
            background: Some(Background::Color(color!(0x1c1d22))),
            ..Default::default()
        }
    }
}

pub mod button {
    use iced::widget::button::Style;
    use iced::{color, Background};

    pub fn primary() -> Style {
        Style {
            background: Some(Background::Color(color!(0xabc4fd))),
            text_color: color!(0x002e69),
            ..Default::default()
        }
    }

    pub fn elevated() -> Style {
        Style {
            background: Some(Background::Color(color!(0x18191e))),
            text_color: color!(0xabc4fd),
            ..Default::default()
        }
    }
}
