//! The pipeline, written down the way `scripts/m1-step3-host.py` and
//! `scripts/m1_encoder.py` write it.
//!
//! The figures of the gate verdict were measured with exactly these
//! elements set exactly this way, and the quality figures with the same
//! encoder. This program must not differ from them by a typing mistake, so
//! the description is text, as there, and a test holds it against what the
//! script builds.
//!
//! It comes in two halves, because this program puts the encoder's half in
//! and takes it out while the compositor's half runs: the gate
//! (`crate::gate`) sits between them.

/// How much colour the stream keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chroma {
    /// One colour sample for every four pixels.
    C420,
    /// One for every pixel: what was decided (ADR 0002).
    C444,
}

/// What the stream says about its colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    /// sRGB, stored as BT.709 brightness and colour differences in the
    /// usual 16 to 235: what any window holds.
    Srgb,
    /// Nothing said; GStreamer then calls it BT.709 video.
    Default,
}

/// The size of the encoder's rate-control buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vbv {
    /// One frame's worth of the bit rate: every frame, key frames too, is
    /// held to about the size of an average one.
    Frame,
    /// Left to the encoder.
    Default,
    /// This many kbit.
    Kbit(u32),
}

/// What the pipeline is made with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// kbit a second.
    pub bitrate: u32,
    /// Frames from one key frame to the next; -1 for none but the first and
    /// those asked for.
    pub gop: i32,
    pub vbv: Vbv,
    pub chroma: Chroma,
    pub preset: String,
    pub colour: Colour,
    /// The DRM render node for the compositor, when it is not to pick one.
    pub render_node: Option<String>,
}

impl Settings {
    /// What was decided on 2026-11-16 (ADR 0002) and measured at the gate,
    /// for a picture of this size and pace.
    pub fn decided(width: u32, height: u32, fps: u32) -> Self {
        Settings {
            width,
            height,
            fps,
            bitrate: 30_000,
            gop: -1,
            vbv: Vbv::Frame,
            chroma: Chroma::C444,
            preset: "p1".to_string(),
            colour: Colour::Srgb,
            render_node: None,
        }
    }

    /// The rate-control buffer in kbit, or `None` to leave it to the encoder.
    pub fn rate_buffer(&self) -> Option<u32> {
        match self.vbv {
            Vbv::Default => None,
            Vbv::Kbit(kbit) => Some(kbit),
            // As the script rounds it: to the nearest, a half to the even one.
            Vbv::Frame => {
                let frame =
                    (f64::from(self.bitrate) / f64::from(self.fps.max(1))).round_ties_even();
                Some((frame as u32).max(1))
            }
        }
    }

    /// The compositor and the frames it makes, which stay on the graphics
    /// card. The element is called "compositor".
    pub fn compositor_side(&self) -> String {
        let node = self
            .render_node
            .as_ref()
            .map_or(String::new(), |node| format!(" render-node={node}"));
        format!(
            "waylanddisplaysrc name=compositor{node} ! video/x-raw(memory:CUDAMemory),width={},height={},framerate={}/1",
            self.width, self.height, self.fps
        )
    }

    /// From frames on the graphics card to whole access units of H.265, NAL
    /// units behind start codes, parameter sets with every key frame. The
    /// colour conversion is called "convert", the encoder "enc" and the way
    /// out "net".
    pub fn encoder_side(&self) -> String {
        let format = match self.chroma {
            Chroma::C420 => "NV12",
            Chroma::C444 => "Y444",
        };
        let colour = match self.colour {
            Colour::Srgb => ",colorimetry=2:3:7:1",
            Colour::Default => "",
        };
        let buffer = self
            .rate_buffer()
            .map_or(String::new(), |kbit| format!(" vbv-buffer-size={kbit}"));
        format!(
            "cudaconvert name=convert ! video/x-raw(memory:CUDAMemory),format={format}{colour} ! \
             nvcudah265enc name=enc preset={} tune=ultra-low-latency zero-reorder-delay=true b-frames=0 \
             rate-control=cbr bitrate={} gop-size={}{buffer} ! \
             h265parse config-interval=-1 ! \
             video/x-h265,stream-format=byte-stream,alignment=au ! \
             appsink name=net emit-signals=true sync=false async=false",
            self.preset, self.bitrate, self.gop
        )
    }

    /// Both halves as one pipeline: what the script starts.
    pub fn whole(&self) -> String {
        format!("{} ! {}", self.compositor_side(), self.encoder_side())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pipeline_of_the_gate_verdict_is_written_as_the_script_wrote_it() {
        let settings = Settings::decided(3440, 1440, 85);
        assert_eq!(settings.rate_buffer(), Some(353));
        assert_eq!(
            settings.whole(),
            "waylanddisplaysrc name=compositor ! \
             video/x-raw(memory:CUDAMemory),width=3440,height=1440,framerate=85/1 ! \
             cudaconvert name=convert ! video/x-raw(memory:CUDAMemory),format=Y444,colorimetry=2:3:7:1 ! \
             nvcudah265enc name=enc preset=p1 tune=ultra-low-latency zero-reorder-delay=true b-frames=0 \
             rate-control=cbr bitrate=30000 gop-size=-1 vbv-buffer-size=353 ! \
             h265parse config-interval=-1 ! \
             video/x-h265,stream-format=byte-stream,alignment=au ! \
             appsink name=net emit-signals=true sync=false async=false"
        );
    }

    #[test]
    fn what_can_be_chosen_shows_in_the_description() {
        let mut settings = Settings::decided(1720, 720, 60);
        settings.chroma = Chroma::C420;
        settings.colour = Colour::Default;
        settings.vbv = Vbv::Default;
        settings.gop = 120;
        settings.render_node = Some("/dev/dri/renderD129".into());
        assert!(settings
            .compositor_side()
            .starts_with("waylanddisplaysrc name=compositor render-node=/dev/dri/renderD129 ! "));
        let encoder = settings.encoder_side();
        assert!(encoder.contains("video/x-raw(memory:CUDAMemory),format=NV12 ! nvcudah265enc"));
        assert!(encoder.contains("gop-size=120 ! h265parse"));
        assert!(!encoder.contains("vbv-buffer-size"));
        settings.vbv = Vbv::Kbit(500);
        assert!(settings
            .encoder_side()
            .contains("gop-size=120 vbv-buffer-size=500 ! "));
    }

    #[test]
    fn one_frame_s_worth_of_the_bit_rate_is_rounded_as_the_script_rounds_it() {
        let buffer = |bitrate, fps| {
            let mut settings = Settings::decided(1, 1, fps);
            settings.bitrate = bitrate;
            settings.rate_buffer()
        };
        assert_eq!(buffer(30_000, 85), Some(353));
        assert_eq!(buffer(30_000, 60), Some(500));
        // Halves go to the even number, as Python's round does.
        assert_eq!(buffer(5, 2), Some(2));
        assert_eq!(buffer(7, 2), Some(4));
        // Never nothing.
        assert_eq!(buffer(10, 85), Some(1));
    }

    /// Holds the description against the script itself, where there is a
    /// Python to ask. The script is what was measured: if the two differ,
    /// this program is wrong.
    #[test]
    fn the_encoder_is_the_one_the_script_builds() {
        let scripts = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts");
        let ask = "import sys; sys.path.insert(0, sys.argv[1]); import m1_encoder as e; \
                   [print(e.encoder(b, f, g, e.rate_buffer(v, b, f), c, 'p1', k)) for (b, f, g, v, c, k) in \
                   [(30000, 85, -1, 'frame', '444', 'srgb'), (20000, 60, 120, 'default', '420', 'default'), (30000, 85, -1, '500', '444', 'srgb')]]";
        let Ok(output) = std::process::Command::new("python3")
            .args(["-I", "-c", ask, scripts])
            .output()
        else {
            eprintln!("no python3 here: the description was not held against the script");
            return;
        };
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let theirs = String::from_utf8(output.stdout).unwrap();
        let theirs: Vec<&str> = theirs.lines().collect();
        let way_out = " ! video/x-h265,stream-format=byte-stream,alignment=au ! appsink name=net emit-signals=true sync=false async=false";
        let mut ours = vec![Settings::decided(3440, 1440, 85)];
        let mut second = Settings::decided(3440, 1440, 60);
        second.bitrate = 20_000;
        second.gop = 120;
        second.vbv = Vbv::Default;
        second.chroma = Chroma::C420;
        second.colour = Colour::Default;
        ours.push(second);
        let mut third = Settings::decided(3440, 1440, 85);
        third.vbv = Vbv::Kbit(500);
        ours.push(third);
        assert_eq!(theirs.len(), ours.len());
        for (theirs, ours) in theirs.iter().zip(&ours) {
            assert_eq!(format!("{theirs}{way_out}"), ours.encoder_side());
        }
    }
}
