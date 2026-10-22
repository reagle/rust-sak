use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use super::ImageFormat;
use super::container;
use super::dispatch::{icc_profile_native, usable_profile};
use super::error::{ImageError, Result};

/// How much of an AVIF, HEIF or WebP file is read before looking for its profile; the file is re-read whole only when
/// the profile lies further in. The same bound [`probe_file`](super::probe_file) starts with.
const HEADER_PREFIX: u64 = 64 * 1024;

/// Returns the exact bytes of the ICC colour profile the image at `path` embeds, **without decoding the pixels**,
/// selecting the format from the file extension.
///
/// The file form of [`icc_profile_bytes`](super::icc_profile_bytes), with the same answers. Native formats are read
/// through their decoder straight from the file, so only the header comes off disk; for AVIF, HEIF and WebP a bounded
/// prefix is read and the file is read whole only when the profile lies past it.
///
/// # Errors
///
/// Returns [`ImageError::UnknownExtension`] if the path has no recognized image extension,
/// [`ImageError::Io`](super::ImageError::Io) if the file cannot be read, and the codec's own error if a native format's
/// header is malformed.
pub fn icc_profile_file(path: impl AsRef<Path>) -> Result<Option<Vec<u8>>> {
    let path = path.as_ref();
    let format = ImageFormat::from_path(path).ok_or(ImageError::UnknownExtension)?;

    if format.to_image_format().is_some() {
        return icc_profile_native(BufReader::new(File::open(path)?), format);
    }

    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    file.by_ref().take(HEADER_PREFIX).read_to_end(&mut bytes)?;

    // A prefix shorter than the cap is the whole file; otherwise running out of it says nothing about the file.
    if (bytes.len() as u64) == HEADER_PREFIX && container::icc_profile(&bytes, format).is_err() {
        file.read_to_end(&mut bytes)?;
    }

    Ok(container::icc_profile(&bytes, format)
        .unwrap_or_default()
        .and_then(usable_profile))
}
