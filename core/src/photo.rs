//! Profile photos (#94).
//!
//! A photo is decoded, turned upright, cut to a centred square, shrunk and saved again as JPEG
//! before MDK uploads it. MDK uploads files as they are, and a phone photo carries its GPS
//! location in its metadata; saving it again keeps the pixels only. Contacts' photos get the
//! same treatment, smaller, and are kept in the data folder for avatars.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits, Rgb, RgbImage};
use sha2::{Digest, Sha256};

/// The side of an uploaded photo, in pixels.
pub const UPLOAD_SIDE: u32 = 512;
/// The side of an avatar, in pixels: three times the largest avatar on screen.
pub const AVATAR_SIDE: u32 = 288;
/// The largest photo the core reads, in bytes, as MDK's limit for profile images.
pub const MAX_PHOTO_BYTES: usize = 10 * 1024 * 1024;
/// The largest photo the core decodes, in pixels on a side, and the memory it may take.
const MAX_SIDE: u32 = 16_384;
const MAX_DECODE_BYTES: u64 = 512 * 1024 * 1024;
const JPEG_QUALITY: u8 = 85;

#[derive(Debug, thiserror::Error)]
pub enum PhotoError {
    #[error("the photo is larger than {MAX_PHOTO_BYTES} bytes")]
    TooLarge,
    #[error("the photo can't be read: {0}")]
    Unreadable(#[from] image::ImageError),
    #[error("the photo can't be read: {0}")]
    Io(#[from] std::io::Error),
}

/// `photo` as a centred square JPEG of at most `side` pixels, upright, with nothing but its
/// pixels. Transparent parts become white.
pub fn square_jpeg(photo: &[u8], side: u32) -> Result<Vec<u8>, PhotoError> {
    if photo.len() > MAX_PHOTO_BYTES {
        return Err(PhotoError::TooLarge);
    }
    let mut reader = ImageReader::new(Cursor::new(photo)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);

    let short = image.width().min(image.height());
    let left = (image.width() - short) / 2;
    let top = (image.height() - short) / 2;
    let mut square = image.crop_imm(left, top, short, short);
    if short > side {
        square = square.resize_exact(side, side, FilterType::Lanczos3);
    }
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY).encode_image(&on_white(&square))?;
    Ok(jpeg)
}

/// The image's pixels without transparency, laid over white.
fn on_white(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let blend =
            |c: u8| ((u16::from(c) * u16::from(a) + 255 * (255 - u16::from(a))) / 255) as u8;
        Rgb([blend(r), blend(g), blend(b)])
    })
}

/// A JPEG as the interface shows it: a `data:` URL, which the web view's policy allows.
pub fn data_url(jpeg: &[u8]) -> String {
    format!("data:image/jpeg;base64,{}", BASE64.encode(jpeg))
}

/// Avatars kept in the data folder, by the address of the photo they were made from.
pub struct Avatars {
    dir: PathBuf,
}

impl Avatars {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("avatars"),
        }
    }

    /// The avatar made from the photo at `url`, if this device has one.
    pub fn get(&self, url: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(url)).ok()
    }

    pub fn put(&self, url: &str, avatar: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.path(url), avatar)
    }

    /// Forgets every avatar, as when the account is deleted.
    pub fn clear(&self) -> std::io::Result<()> {
        match std::fs::remove_dir_all(&self.dir) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn path(&self, url: &str) -> PathBuf {
        self.dir.join(format!(
            "{}.jpg",
            hex::encode(Sha256::digest(url.as_bytes()))
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgba, RgbaImage};

    const RED: Rgb<u8> = Rgb([255, 0, 0]);
    const BLUE: Rgb<u8> = Rgb([0, 0, 255]);

    /// A JPEG of `width` by `height`, red on the left half and blue on the right.
    fn halves(width: u32, height: u32) -> Vec<u8> {
        let image = RgbImage::from_fn(width, height, |x, _| if x < width / 2 { RED } else { BLUE });
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode_image(&image)
            .unwrap();
        jpeg
    }

    /// `jpeg` with an Exif segment that holds an orientation and the start of a GPS record, as
    /// phones write them.
    fn with_exif(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        // TIFF, big-endian: one IFD with Orientation (0x0112), and a GPSInfo pointer (0x8825).
        let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec();
        tiff.extend_from_slice(&2u16.to_be_bytes());
        tiff.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1]);
        tiff.extend_from_slice(&orientation.to_be_bytes());
        tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&[0x88, 0x25, 0, 4, 0, 0, 0, 1, 0, 0, 0, 0]);
        tiff.extend_from_slice(&[0, 0, 0, 0]);
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let mut out = jpeg[..2].to_vec(); // the start-of-image marker
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    fn decode(jpeg: &[u8]) -> RgbImage {
        image::load_from_memory_with_format(jpeg, ImageFormat::Jpeg)
            .unwrap()
            .to_rgb8()
    }

    fn near(pixel: &Rgb<u8>, colour: Rgb<u8>) -> bool {
        pixel
            .0
            .iter()
            .zip(colour.0)
            .all(|(a, b)| a.abs_diff(b) < 60)
    }

    #[test]
    fn a_photo_becomes_a_centred_square_of_the_size_asked() {
        let square = decode(&square_jpeg(&halves(1200, 600), 512).unwrap());
        assert_eq!(square.dimensions(), (512, 512));
        // The centre of a wide photo: red on the left, blue on the right.
        assert!(near(square.get_pixel(20, 256), RED));
        assert!(near(square.get_pixel(490, 256), BLUE));
        // A small photo isn't made larger.
        assert_eq!(
            decode(&square_jpeg(&halves(100, 80), 512).unwrap()).dimensions(),
            (80, 80)
        );
    }

    #[test]
    fn metadata_is_dropped_and_the_photo_turned_upright() {
        let photo = with_exif(&halves(400, 200), 6);
        assert!(photo.windows(4).any(|w| w == b"Exif"));
        let jpeg = square_jpeg(&photo, 200).unwrap();
        assert!(
            !jpeg.windows(4).any(|w| w == b"Exif"),
            "the Exif segment is still there"
        );
        // Orientation 6 turns the photo a quarter clockwise: the left half ends up on top.
        let square = decode(&jpeg);
        assert!(near(square.get_pixel(100, 10), RED));
        assert!(near(square.get_pixel(100, 190), BLUE));
    }

    #[test]
    fn transparent_parts_become_white() {
        let image = RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 0]));
        let mut png = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let square = decode(&square_jpeg(&png, 10).unwrap());
        assert!(near(square.get_pixel(5, 5), Rgb([255, 255, 255])));
    }

    #[test]
    fn what_isnt_a_photo_is_refused() {
        assert!(matches!(
            square_jpeg(b"not a photo", 512),
            Err(PhotoError::Unreadable(_) | PhotoError::Io(_))
        ));
        assert!(matches!(
            square_jpeg(&vec![0; MAX_PHOTO_BYTES + 1], 512),
            Err(PhotoError::TooLarge)
        ));
    }

    #[test]
    fn avatars_are_kept_by_the_photos_address() {
        let data = tempfile::tempdir().unwrap();
        let avatars = Avatars::new(data.path());
        assert_eq!(avatars.get("https://media.example/a.jpg"), None);
        avatars
            .put("https://media.example/a.jpg", b"avatar")
            .unwrap();
        assert_eq!(
            avatars.get("https://media.example/a.jpg").as_deref(),
            Some(&b"avatar"[..])
        );
        assert_eq!(avatars.get("https://media.example/b.jpg"), None);
        avatars.clear().unwrap();
        avatars.clear().unwrap();
        assert_eq!(avatars.get("https://media.example/a.jpg"), None);
        assert!(data_url(b"x").starts_with("data:image/jpeg;base64,"));
    }
}
