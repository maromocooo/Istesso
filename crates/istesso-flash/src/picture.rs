//! What the test application paints: a choice between the pictures of
//! [`istesso_measure::picture`].

pub use istesso_measure::picture::*;

/// What the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Picture {
    /// Scrolling text with a square in the middle that shows the shade
    Text,
    /// The whole window in the shade: the easiest picture there is
    Grey,
    /// Something like a photograph in motion, with grain: hard on a relay
    Hard,
}

impl Picture {
    /// The name written into a log.
    pub fn name(self) -> &'static str {
        match self {
            Picture::Text => "text",
            Picture::Grey => "grey",
            Picture::Hard => "hard",
        }
    }
}
