//! k9s stock skin colours as plain `u32` RGB values.

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub background: u32,
    pub foreground: u32,
    pub header: u32,
    pub accent: u32,
    pub selection_bg: u32,
    pub selection_fg: u32,
    pub border: u32,
    pub pending: u32,
    pub completed: u32,
    pub error: u32,
    pub killing: u32,
    pub dim: u32,
    pub key: u32,
    pub string: u32,
    pub number: u32,
    pub boolean: u32,
    pub info: u32,
    pub warn: u32,
}

impl Theme {
    pub fn tone(&self, tone: k9s_core::Tone) -> u32 {
        use k9s_core::Tone;
        match tone {
            Tone::Plain => self.foreground,
            Tone::Dim | Tone::Null | Tone::Debug => self.dim,
            Tone::Key => self.key,
            Tone::Str => self.string,
            Tone::Num => self.number,
            Tone::Bool => self.boolean,
            Tone::Info => self.info,
            Tone::Warn => self.warn,
            Tone::Error => self.error,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: 0x000000,
            foreground: 0x00ffff,
            header: 0xffffff,
            accent: 0xffa500,
            selection_bg: 0x00ffff,
            selection_fg: 0x000000,
            border: 0x1e90ff,
            pending: 0xffa500,
            completed: 0x778899,
            error: 0xff4500,
            killing: 0x9370db,
            dim: 0x778899,
            key: 0x1e90ff,
            string: 0x98fb98,
            number: 0xffa500,
            boolean: 0xdda0dd,
            info: 0x00ff7f,
            warn: 0xffd700,
        }
    }
}
