//! Which cursors travel to the device as a name, and which as an image.
//!
//! The device draws the cursor itself, where its own pointer is. For the
//! shapes every system has (an arrow, an I-beam, a hand, the resize arrows)
//! it shows its own cursor, which looks at home there, when it is told the
//! shape's name instead of an image (ADR 0019; the set of names is the own
//! client's, ADR 0018).
//!
//! An application says its cursor in one of two ways. One that names the
//! shape is passed on by name when the name is in the set, and otherwise
//! as the image the host's cursor theme has for it. One that hands over an
//! image, as most still do, has usually read that image from the host's
//! cursor theme: the image is held against the theme's, and when it is one
//! of them, pixel for pixel, its name goes instead.
//!
//! None of this is on a frame's path. The theme's images are read once, on
//! a thread of their own; a cursor is looked up when the compositor reports
//! a change, on the thread that hears the pipeline's bus.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// The names the device has a cursor of its own for (ADR 0018, "名前で送る
/// 集合"), each with the files a cursor theme may keep the shape in: the
/// name itself, and the older names from before the shapes had common ones.
const SENT_AS_NAMES: [(&str, &[&str]); 18] = [
    ("default", &["default", "left_ptr", "arrow"]),
    ("text", &["text", "xterm", "ibeam"]),
    ("vertical-text", &["vertical-text"]),
    ("pointer", &["pointer", "hand2", "hand1", "hand"]),
    (
        "ew-resize",
        &["ew-resize", "sb_h_double_arrow", "h_double_arrow"],
    ),
    ("e-resize", &["e-resize", "right_side"]),
    ("w-resize", &["w-resize", "left_side"]),
    ("col-resize", &["col-resize", "split_h"]),
    (
        "ns-resize",
        &["ns-resize", "sb_v_double_arrow", "v_double_arrow"],
    ),
    ("n-resize", &["n-resize", "top_side"]),
    ("s-resize", &["s-resize", "bottom_side"]),
    ("row-resize", &["row-resize", "split_v"]),
    ("nesw-resize", &["nesw-resize", "fd_double_arrow"]),
    ("ne-resize", &["ne-resize", "top_right_corner"]),
    ("sw-resize", &["sw-resize", "bottom_left_corner"]),
    ("nwse-resize", &["nwse-resize", "bd_double_arrow"]),
    ("nw-resize", &["nw-resize", "top_left_corner"]),
    ("se-resize", &["se-resize", "bottom_right_corner"]),
];

/// Whether a cursor of this name is told to the device by its name.
pub fn travels_as_name(name: &str) -> bool {
    SENT_AS_NAMES.iter().any(|(known, _)| *known == name)
}

/// One image of a cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    /// Four bytes a pixel: red, green, blue, alpha, the colours multiplied by alpha.
    pub rgba: Vec<u8>,
}

/// The largest side of an image that is read: what may travel to a device.
const LARGEST: u32 = 256;

/// Reads the images of a cursor file in the format X11's cursor themes use
/// (Xcursor): for each size the file has, the first image of that size.
/// A file that is not one, or is cut short, has none.
pub fn images(file: &[u8]) -> Vec<Image> {
    let number = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(file.get(at..at + 4)?.try_into().ok()?))
    };
    let mut found: Vec<(u32, Image)> = Vec::new();
    let mut read = || -> Option<()> {
        if file.get(..4)? != b"Xcur" {
            return None;
        }
        let (header, entries) = (number(4)? as usize, number(12)?);
        for entry in 0..entries.min(4096) as usize {
            let at = header + entry * 12;
            let (kind, size, position) = (number(at)?, number(at + 4)?, number(at + 8)? as usize);
            // Only images; and of an animated cursor only its first picture.
            if kind != 0xfffd_0002 || found.iter().any(|(known, _)| *known == size) {
                continue;
            }
            let chunk = number(position)? as usize;
            let (width, height) = (number(position + 16)?, number(position + 20)?);
            let (hotspot_x, hotspot_y) = (number(position + 24)?, number(position + 28)?);
            if width == 0 || height == 0 || width > LARGEST || height > LARGEST {
                continue;
            }
            let length = (width * height * 4) as usize;
            let pixels = file.get(position + chunk..position + chunk + length)?;
            // Stored blue, green, red, alpha; told red, green, blue, alpha.
            let rgba = pixels
                .chunks_exact(4)
                .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
                .collect();
            found.push((
                size,
                Image {
                    width,
                    height,
                    hotspot_x,
                    hotspot_y,
                    rgba,
                },
            ));
        }
        Some(())
    };
    let _ = read();
    found.into_iter().map(|(_, image)| image).collect()
}

fn fingerprint(image: &Image) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (image.width, image.height, image.hotspot_x, image.hotspot_y).hash(&mut hasher);
    image.rgba.hash(&mut hasher);
    hasher.finish()
}

/// The images of the host's cursor themes, for the shapes that travel by name.
#[derive(Debug, Default)]
pub struct Themes {
    /// By what an image looks like, to the name of its shape.
    names: HashMap<u64, Vec<(Image, &'static str)>>,
}

/// Where cursor themes are kept: what `XCURSOR_PATH` says, or the usual places.
pub fn theme_directories() -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os("XCURSOR_PATH").filter(|path| !path.is_empty()) {
        return std::env::split_paths(&path).collect();
    }
    let mut directories = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        directories.push(home.join(".icons"));
        directories.push(home.join(".local/share/icons"));
    }
    directories.push(PathBuf::from("/usr/share/icons"));
    directories.push(PathBuf::from("/usr/share/pixmaps"));
    directories
}

impl Themes {
    /// Reads, from every theme in these directories, the images of the
    /// shapes that travel by name. A shape that several names share goes by
    /// the first of them in the set.
    pub fn read(directories: &[PathBuf]) -> Self {
        let mut themes = Themes::default();
        for directory in directories {
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            for cursors in entries.filter_map(|entry| Some(entry.ok()?.path().join("cursors"))) {
                themes.read_theme(&cursors);
            }
        }
        themes
    }

    fn read_theme(&mut self, cursors: &Path) {
        for (name, files) in SENT_AS_NAMES {
            for file in files {
                let Ok(bytes) = std::fs::read(cursors.join(file)) else {
                    continue;
                };
                for image in images(&bytes) {
                    let known = self.names.entry(fingerprint(&image)).or_default();
                    if !known.iter().any(|(other, _)| *other == image) {
                        known.push((image, name));
                    }
                }
            }
        }
    }

    /// The name of the shape this image is, when it is, pixel for pixel,
    /// one of a theme's images for a shape that travels by name.
    pub fn name_of(&self, image: &Image) -> Option<&'static str> {
        self.names
            .get(&fingerprint(image))?
            .iter()
            .find(|(known, _)| known == image)
            .map(|(_, name)| *name)
    }

    /// How many images are known.
    pub fn len(&self) -> usize {
        self.names.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// The host's cursor themes, read once on a thread of their own. Until they
/// have been read, no image is known and every image travels as it is.
#[derive(Debug)]
pub struct Book {
    directories: Vec<PathBuf>,
    themes: std::sync::OnceLock<Themes>,
}

impl Book {
    /// Starts reading the themes in these directories.
    pub fn open(directories: Vec<PathBuf>) -> std::sync::Arc<Self> {
        let book = std::sync::Arc::new(Book {
            directories,
            themes: std::sync::OnceLock::new(),
        });
        let reader = book.clone();
        let read = move || {
            let _ = reader.themes.set(Themes::read(&reader.directories));
        };
        // Without a thread to read them on, they are read here and now.
        if let Err(error) = std::thread::Builder::new()
            .name("istesso-host-cursors".into())
            .spawn(read.clone())
        {
            let _ = error;
            read();
        }
        book
    }

    /// The name this image goes by, if it is a theme's.
    pub fn name_of(&self, image: &Image) -> Option<&'static str> {
        self.themes.get()?.name_of(image)
    }

    /// The theme's image for a shape that does not travel by name.
    pub fn image_of(&self, name: &str, side: u32) -> Option<Image> {
        image_of(&self.directories, name, side)
    }

    /// Whether the themes have been read.
    pub fn is_read(&self) -> bool {
        self.themes.get().is_some()
    }
}

/// The image the first theme in these directories has for a shape that
/// does not travel by name, nearest to `side` pixels: for an application
/// that names such a shape. `None` when no theme has it.
pub fn image_of(directories: &[PathBuf], name: &str, side: u32) -> Option<Image> {
    // A name is one file's name, never a way out of the theme's directory.
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return None;
    }
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        let mut themes: Vec<PathBuf> = entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .collect();
        themes.sort();
        for theme in themes {
            let Ok(bytes) = std::fs::read(theme.join("cursors").join(name)) else {
                continue;
            };
            let nearest = images(&bytes)
                .into_iter()
                .min_by_key(|image| image.width.abs_diff(side));
            if nearest.is_some() {
                return nearest;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Size, width, height, hot spot, the pixels' first byte.
    type Picture = (u32, u32, u32, (u32, u32), u8);

    /// A cursor file with these images.
    fn file(pictures: &[Picture]) -> Vec<u8> {
        let mut file = b"Xcur".to_vec();
        let put = |file: &mut Vec<u8>, number: u32| file.extend_from_slice(&number.to_le_bytes());
        put(&mut file, 16);
        put(&mut file, 0x1_0000);
        put(&mut file, pictures.len() as u32);
        let mut position = 16 + pictures.len() as u32 * 12;
        for (size, width, height, _, _) in pictures {
            put(&mut file, 0xfffd_0002);
            put(&mut file, *size);
            put(&mut file, position);
            position += 36 + width * height * 4;
        }
        for (size, width, height, (x, y), first) in pictures {
            for number in [36, 0xfffd_0002, *size, 1, *width, *height, *x, *y, 0] {
                put(&mut file, number);
            }
            for pixel in 0..width * height {
                // Blue, green, red, alpha, as the file keeps them.
                file.extend_from_slice(&[*first, 2, 3, (pixel % 251) as u8]);
            }
        }
        file
    }

    fn place(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("ihl-cursor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn theme(icons: &Path, theme: &str, cursor: &str, bytes: &[u8]) {
        let cursors = icons.join(theme).join("cursors");
        std::fs::create_dir_all(&cursors).unwrap();
        std::fs::write(cursors.join(cursor), bytes).unwrap();
    }

    #[test]
    fn the_names_of_the_client_s_set_travel_as_names_and_no_others() {
        for name in [
            "default",
            "text",
            "pointer",
            "ew-resize",
            "nwse-resize",
            "row-resize",
        ] {
            assert!(travels_as_name(name), "{name}");
        }
        for name in ["crosshair", "grab", "not-allowed", "wait", "", "left_ptr"] {
            assert!(!travels_as_name(name), "{name}");
        }
        assert_eq!(SENT_AS_NAMES.len(), 18);
    }

    #[test]
    fn a_cursor_file_is_read_one_image_a_size_with_its_hot_spot_and_colours_in_order() {
        let read = images(&file(&[
            (24, 2, 3, (1, 2), 9),
            (48, 4, 4, (0, 0), 7),
            // The second picture of an animated cursor of the same size.
            (24, 2, 3, (1, 2), 5),
        ]));
        assert_eq!(read.len(), 2);
        assert_eq!((read[0].width, read[0].height), (2, 3));
        assert_eq!((read[0].hotspot_x, read[0].hotspot_y), (1, 2));
        assert_eq!(read[0].rgba.len(), 2 * 3 * 4);
        // Kept blue 9, green 2, red 3; told red, green, blue, alpha.
        assert_eq!(read[0].rgba[..4], [3, 2, 9, 0]);
        assert_eq!(read[1].rgba[..3], [3, 2, 7]);
    }

    #[test]
    fn what_is_not_a_cursor_file_or_is_cut_short_or_too_large_has_no_images() {
        assert!(images(b"").is_empty());
        assert!(images(b"not a cursor file at all").is_empty());
        let whole = file(&[(24, 2, 2, (0, 0), 1)]);
        assert!(images(&whole[..whole.len() - 1]).is_empty());
        assert!(images(&file(&[(512, 257, 1, (0, 0), 1)])).is_empty());
        // A file that says it has more images than it could hold is read as far as it goes.
        let mut lying = whole.clone();
        lying[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(images(&lying).len() <= 1);
    }

    #[test]
    fn an_image_that_is_a_theme_s_goes_by_the_name_of_its_shape() {
        let icons = place("names");
        theme(
            &icons,
            "Adwaita",
            "left_ptr",
            &file(&[(24, 3, 3, (1, 1), 10), (48, 6, 6, (2, 2), 10)]),
        );
        theme(&icons, "Adwaita", "xterm", &file(&[(24, 3, 3, (1, 1), 20)]));
        theme(&icons, "Other", "hand2", &file(&[(24, 3, 3, (1, 1), 30)]));
        theme(
            &icons,
            "Other",
            "crosshair",
            &file(&[(24, 3, 3, (1, 1), 40)]),
        );
        let themes = Themes::read(&[icons.join("missing"), icons.clone()]);
        assert_eq!(themes.len(), 4);
        let arrow = images(&file(&[(48, 6, 6, (2, 2), 10)])).remove(0);
        assert_eq!(themes.name_of(&arrow), Some("default"));
        let beam = images(&file(&[(24, 3, 3, (1, 1), 20)])).remove(0);
        assert_eq!(themes.name_of(&beam), Some("text"));
        let hand = images(&file(&[(24, 3, 3, (1, 1), 30)])).remove(0);
        assert_eq!(themes.name_of(&hand), Some("pointer"));
        // A shape outside the set has no name to go by, though a theme has it.
        let cross = images(&file(&[(24, 3, 3, (1, 1), 40)])).remove(0);
        assert_eq!(themes.name_of(&cross), None);
        // One pixel, or the hot spot, different: not the theme's image.
        let mut near = beam.clone();
        near.rgba[5] ^= 1;
        assert_eq!(themes.name_of(&near), None);
        let mut moved = beam;
        moved.hotspot_x = 0;
        assert_eq!(themes.name_of(&moved), None);
        assert!(Themes::read(&[]).is_empty());
    }

    #[test]
    fn a_shape_outside_the_set_goes_as_the_theme_s_image_of_the_nearest_size() {
        let icons = place("images");
        theme(
            &icons,
            "Adwaita",
            "crosshair",
            &file(&[(24, 24, 24, (12, 12), 1), (48, 48, 48, (24, 24), 1)]),
        );
        let directories = [icons.clone()];
        assert_eq!(image_of(&directories, "crosshair", 24).unwrap().width, 24);
        assert_eq!(image_of(&directories, "crosshair", 40).unwrap().width, 48);
        assert_eq!(image_of(&directories, "grab", 24), None);
        // A name is a file's name in the theme and nothing else.
        std::fs::write(icons.join("secret"), file(&[(24, 2, 2, (0, 0), 1)])).unwrap();
        assert_eq!(image_of(&directories, "../../secret", 24), None);
        assert_eq!(image_of(&directories, "", 24), None);
    }
}
