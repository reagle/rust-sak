use std::io::{BufRead, Cursor, Seek, Write};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use ::image::codecs::jpeg::JpegEncoder;
use ::image::codecs::png::PngEncoder;
use ::image::codecs::tiff::{TiffDecoder, TiffEncoder};
use ::image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader};

use super::blocks::{self, ToWrite};
use super::container;
use super::error::{ImageError, Result};
use super::icc;
use super::info::ImageInfo;
use super::riff;
use super::{Embedded, EncodeOptions, ImageFormat, Metadata};

/// What a decode should be routed to: one of the formats [`ImageFormat`] names, or camera RAW.
///
/// This is what lets [`decode_file`](super::decode_file) and [`decode_bytes`](super::decode_bytes) open everything
/// the build can open, instead of RAW being a parallel set of entry points a caller has to know to reach for. It is
/// deliberately **not** a variant added to [`ImageFormat`], which is the encode target as well as the decode source
/// — see the note on [`RawFormat`](super::RawFormat) for why no RAW format belongs there.
#[derive(Debug, Clone, Copy)]
pub(super) enum DecodeFormat {
    /// One of the eight formats with an encoder to match.
    Image(ImageFormat),
    /// Camera RAW. Carries no format: [`raw_dispatch::decode_raw`](super::raw::dispatch::decode_raw) works from the
    /// bytes alone, and which RAW format it is only matters for reporting.
    #[cfg(feature = "image-raw")]
    Raw,
}

impl DecodeFormat {
    /// Resolves the format from a path's extension.
    ///
    /// A native extension wins: `.tiff` is an ordinary TIFF even though nearly every RAW format is a TIFF
    /// container underneath, which is the same call [`RawFormat::from_extension`](super::RawFormat::from_extension)
    /// makes from the other side.
    pub(super) fn from_path(path: &Path) -> Option<Self> {
        if let Some(format) = ImageFormat::from_path(path) {
            return Some(DecodeFormat::Image(format));
        }

        #[cfg(feature = "image-raw")]
        if super::RawFormat::from_path(path).is_some() {
            return Some(DecodeFormat::Raw);
        }

        None
    }

    /// Resolves the format from leading magic bytes.
    ///
    /// **This finds far less RAW than [`from_path`](DecodeFormat::from_path) does, and cannot do better.** The
    /// TIFF-based RAW formats — NEF, ARW, CR2, PEF, DNG — open with the same four bytes as an ordinary TIFF, so
    /// they are claimed as TIFF here and there is no byte to tell them apart; a file name is the only thing that
    /// does. What this does pick up is the containers carrying a signature of their own (CR3, RAF, RW2, ORF),
    /// which no native format matches and which therefore used to be refused outright.
    pub(super) fn from_magic(bytes: &[u8]) -> Option<Self> {
        if let Some(format) = ImageFormat::from_magic(bytes) {
            return Some(DecodeFormat::Image(format));
        }

        #[cfg(feature = "image-raw")]
        if super::RawFormat::from_magic(bytes).is_some() {
            return Some(DecodeFormat::Raw);
        }

        None
    }
}

/// Decodes `bytes` through whichever codec `format` names. The one decode path.
pub(super) fn decode_any(bytes: &[u8], format: DecodeFormat) -> Result<DynamicImage> {
    match format {
        DecodeFormat::Image(format) => decode_with_format(bytes, format),
        #[cfg(feature = "image-raw")]
        DecodeFormat::Raw => super::raw::dispatch::decode_raw(bytes),
    }
}

/// Decodes `bytes` known to be in `format`, routing native formats through the `image` crate and `avif`/`heif`/`webp`
/// through their dedicated codecs. Shared by every decode entry point.
pub(super) fn decode_with_format(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage> {
    match format {
        ImageFormat::Avif => Ok(::avif::decode(bytes)?),
        ImageFormat::Heif => Ok(::heif::decode(bytes)?),
        ImageFormat::WebP => Ok(::webp::decode(bytes)?),
        native => {
            let image_format = native
                .to_image_format()
                .expect("native formats map to image::ImageFormat");
            Ok(::image::load_from_memory_with_format(bytes, image_format)?)
        }
    }
}

/// Reads `bytes` known to be in `format` and returns its header metadata **without decoding the pixels**,
/// routing native formats through the `image` crate's streaming decoder and `avif`/`heif`/`webp` through their
/// dedicated crates' `probe` helpers. Shared by every probe entry point.
pub(super) fn probe_with_format(bytes: &[u8], format: ImageFormat) -> Result<ImageInfo> {
    match format {
        ImageFormat::Avif => {
            let info = ::avif::probe(bytes)?;
            Ok(ImageInfo {
                format,
                width: info.width,
                height: info.height,
                color_type: info.color_type,
                bit_depth: avif_bit_depth(info.bit_depth),
                color_profile: container_profile(bytes, format),
            })
        }
        ImageFormat::Heif => {
            let info = ::heif::probe(bytes)?;
            Ok(ImageInfo {
                format,
                width: info.width,
                height: info.height,
                color_type: info.color_type,
                bit_depth: heif_bit_depth(info.bit_depth),
                color_profile: container_profile(bytes, format),
            })
        }
        ImageFormat::WebP => {
            // WebP is always 8 bits per channel.
            let info = ::webp::probe(bytes)?;
            Ok(ImageInfo {
                format,
                width: info.width,
                height: info.height,
                color_type: info.color_type,
                bit_depth: 8,
                color_profile: container_profile(bytes, format),
            })
        }
        native => probe_native(Cursor::new(bytes), native),
    }
}

/// The color profile the `avif`/`heif`/`webp` container in `bytes` declares. `bytes` is taken to be the whole file,
/// so running out of it means there is no profile to find; [`probe_file`](super::probe_file), which may hold only a
/// prefix, checks for that itself before trusting this.
fn container_profile(bytes: &[u8], format: ImageFormat) -> Option<String> {
    container::color_profile(bytes, format).unwrap_or_default()
}

/// Reads the metadata of a native-format image from `reader` **without decoding the pixels**, pulling only as much
/// of it as the header needs.
///
/// Shared by [`probe_with_format`], which reads from a byte slice, and [`probe_file`](super::probe_file), which reads
/// straight from the file so only the header comes off disk.
///
/// # Panics
///
/// If `format` is one of the formats with a dedicated codec (`avif`/`heif`/`webp`), which have no `image` crate
/// decoder to read from.
pub(super) fn probe_native(reader: impl BufRead + Seek, format: ImageFormat) -> Result<ImageInfo> {
    let image_format = format
        .to_image_format()
        .expect("native formats map to image::ImageFormat");
    // Parses the header only; `read_image` (never called here) is what decodes pixels.
    let mut decoder = native_decoder(reader, image_format)?;

    let (width, height) = decoder.dimensions();
    let color_type = decoder.color_type();
    let bit_depth = (color_type.bits_per_pixel() / color_type.channel_count() as u16) as u8;
    // Every native decoder has the profile in hand once the header is parsed (JPEG `APP2`, PNG `iCCP`, TIFF tag
    // 34675). A profile that fails to read is reported as no profile: it is a label, not the image.
    let color_profile = decoder
        .icc_profile()
        .ok()
        .flatten()
        .and_then(|profile| icc::description(&profile));

    Ok(ImageInfo {
        format,
        width,
        height,
        color_type,
        bit_depth,
        color_profile,
    })
}

/// The largest ICC profile read or embedded. The JPEG `APP2` scheme tops out just under 16 MiB (255 segments of 65,519
/// bytes), real profiles are kilobytes to a few megabytes, and the bytes come from an untrusted file — a PNG's `iCCP`
/// is compressed, so its size is not bounded by the file's — so anything larger is treated as no profile.
pub(super) const MAX_ICC_PROFILE_LEN: usize = 16 * 1024 * 1024;

/// The ICC profile bytes `profile`, if they can be one: at least the 128-byte header, the `acsp` signature where the
/// header puts it, and no larger than [`MAX_ICC_PROFILE_LEN`]. Nothing else is interpreted.
pub(super) fn usable_profile(profile: &[u8]) -> Option<Vec<u8>> {
    let usable = profile.len() >= 128 && profile.len() <= MAX_ICC_PROFILE_LEN && &profile[36..40] == b"acsp";
    usable.then(|| profile.to_vec())
}

/// Reads the embedded ICC profile of `bytes` known to be in `format`, without decoding the pixels. Shared by
/// [`icc_profile_bytes`](super::icc_profile_bytes); [`icc_profile_file`](super::icc_profile_file) reaches the same two
/// paths itself, so it can read only a header from disk.
pub(super) fn icc_profile_with_format(bytes: &[u8], format: ImageFormat) -> Result<Option<Vec<u8>>> {
    if format.to_image_format().is_some() {
        return icc_profile_native(Cursor::new(bytes), format);
    }
    // A container that ends early or cannot be walked declares nothing this can use; see `container_profile`.
    Ok(container::icc_profile(bytes, format)
        .unwrap_or_default()
        .and_then(usable_profile))
}

/// Reads the embedded ICC profile of a native-format image from `reader`, parsing only its header — the same decoder
/// [`probe_native`] asks for the profile's name.
///
/// # Panics
///
/// If `format` is one of the formats with a dedicated codec (`avif`/`heif`/`webp`).
pub(super) fn icc_profile_native(reader: impl BufRead + Seek, format: ImageFormat) -> Result<Option<Vec<u8>>> {
    let image_format = format
        .to_image_format()
        .expect("native formats map to image::ImageFormat");
    let mut decoder = native_decoder(reader, image_format)?;
    // A profile that fails to read is no profile, as in `probe_native`: the header was fine, so the image is.
    Ok(decoder
        .icc_profile()
        .ok()
        .flatten()
        .and_then(|profile| usable_profile(&profile)))
}

/// The `image` crate's decoder for a native format, with its header parsed and no pixels decoded.
///
/// TIFF is built directly rather than through [`ImageReader`], whose limits hand the `tiff` crate a decoding buffer
/// the size of the image's own pixels and nothing more. The `tiff` crate applies that same bound to every tag value it
/// reads, so through `ImageReader` a profile larger than about a twenty-fourth of the pixel data (any profile at all,
/// on a small image) fails to read and is reported as absent. Built directly, it keeps the `tiff` crate's own default
/// bound of 256 MiB per value.
fn native_decoder<'a, R: BufRead + Seek + 'a>(
    reader: R,
    format: ::image::ImageFormat,
) -> Result<Box<dyn ImageDecoder + 'a>> {
    if format == ::image::ImageFormat::Tiff {
        return Ok(Box::new(TiffDecoder::new(reader)?));
    }
    Ok(Box::new(ImageReader::with_format(reader, format).into_decoder()?))
}

/// Maps the `avif` crate's bit-depth enum to bits per channel. Both enums are `#[non_exhaustive]`, so the
/// catch-all keeps this compiling against future codec variants (defaulting to 8-bit).
fn avif_bit_depth(depth: ::avif::BitDepth) -> u8 {
    match depth {
        ::avif::BitDepth::Eight => 8,
        ::avif::BitDepth::Ten => 10,
        ::avif::BitDepth::Twelve => 12,
        _ => 8,
    }
}

/// Maps the `heif` crate's bit-depth enum to bits per channel. See [`avif_bit_depth`] for the catch-all rationale.
fn heif_bit_depth(depth: ::heif::BitDepth) -> u8 {
    match depth {
        ::heif::BitDepth::Eight => 8,
        ::heif::BitDepth::Ten => 10,
        ::heif::BitDepth::Twelve => 12,
        _ => 8,
    }
}

/// Resolves the caller-supplied options against the encode `format`: `None` yields the format's defaults, while
/// `Some(options)` for a different format is rejected as an [`ImageError::FormatMismatch`]. Shared by both encoders.
pub(super) fn resolve_options(format: ImageFormat, options: Option<EncodeOptions>) -> Result<EncodeOptions> {
    let options = options.unwrap_or_else(|| EncodeOptions::default_for(format));
    let actual = options.format();
    if actual == format {
        Ok(options)
    } else {
        Err(ImageError::FormatMismatch {
            expected: format,
            options: actual,
        })
    }
}

/// Returns `image` converted to a colour type the `format` encoder accepts, or `None` when it already accepts the
/// image as it stands (so the common case allocates nothing).
///
/// Every codec accepts a fixed set of colour types and answers `Unsupported` for the rest, which would otherwise make
/// an ordinary picture — a developed camera RAW is 16-bit, a grayscale PNG has no colour — unwritable in some formats.
/// The conversion loses as little as the target forces: depth is narrowed only where the encoder cannot hold it, alpha
/// is kept wherever the format has an alpha channel, and grayscale is expanded to colour only where the format has no
/// grayscale representation. So a 32-float picture written as PNG becomes 16-bit rather than 8-bit, and a
/// grayscale-with-alpha picture written as TIFF — which has no grayscale-with-alpha at any depth — becomes RGBA at its
/// original depth rather than being narrowed twice.
///
/// **BMP and JPEG are deliberately absent.** Both implement the `image` crate's own
/// [`ImageEncoder::make_compatible_img`](::image::ImageEncoder::make_compatible_img) hook, which runs inside
/// `write_with_encoder` *after* this; converting here as well would convert twice, and for JPEG to a worse result —
/// its hook sends `La8` to luma, keeping grayscale, where a generic narrowing would send it to luma-alpha first.
pub(super) fn make_compatible(image: &DynamicImage, format: ImageFormat) -> Option<DynamicImage> {
    use DynamicImage::{
        ImageLuma8, ImageLumaA8, ImageRgb8, ImageRgb16, ImageRgb32F, ImageRgba8, ImageRgba16, ImageRgba32F,
    };

    match format {
        // Their own `make_compatible_img` already covers everything — see the note above.
        ImageFormat::Bmp | ImageFormat::Jpeg => None,
        // 8- and 16-bit, with or without alpha, grayscale or colour; only the 32-float types need narrowing, and 16 is
        // as far as they have to come down.
        ImageFormat::Png | ImageFormat::Avif | ImageFormat::Heif => match image {
            ImageRgb32F(_) => Some(ImageRgb16(image.to_rgb16())),
            ImageRgba32F(_) => Some(ImageRgba16(image.to_rgba16())),
            _ => None,
        },
        // Every depth including 32-float, but no grayscale-with-alpha at any depth: alpha is what forces the move to
        // RGBA, so the depth comes along rather than being given up with it.
        ImageFormat::Tiff => match image {
            ImageLumaA8(_) => Some(ImageRgba8(image.to_rgba8())),
            DynamicImage::ImageLumaA16(_) => Some(ImageRgba16(image.to_rgba16())),
            _ => None,
        },
        // `Rgb8`/`Rgba8` only — the narrowest of the eight. Everything else is 8-bit colour, keeping alpha where the
        // source had it.
        ImageFormat::Gif => match image {
            ImageRgb8(_) | ImageRgba8(_) => None,
            other if other.color().has_alpha() => Some(ImageRgba8(other.to_rgba8())),
            other => Some(ImageRgb8(other.to_rgb8())),
        },
        // libwebp is 8-bit, which is a property of the codec rather than of this dispatch; the colour layout survives
        // untouched, so grayscale stays grayscale.
        ImageFormat::WebP => match image {
            ImageLuma8(_) | ImageLumaA8(_) | ImageRgb8(_) | ImageRgba8(_) => None,
            DynamicImage::ImageLuma16(_) => Some(ImageLuma8(image.to_luma8())),
            DynamicImage::ImageLumaA16(_) => Some(ImageLumaA8(image.to_luma_alpha8())),
            ImageRgb16(_) | ImageRgb32F(_) => Some(ImageRgb8(image.to_rgb8())),
            ImageRgba16(_) | ImageRgba32F(_) => Some(ImageRgba8(image.to_rgba8())),
            // `DynamicImage` is `#[non_exhaustive]`: a variant added later keeps its alpha and comes down to 8 bits.
            other if other.color().has_alpha() => Some(ImageRgba8(other.to_rgba8())),
            other => Some(ImageRgb8(other.to_rgb8())),
        },
    }
}

/// Serializes AVIF encodes against each other.
///
/// The bundled SVT-AV1 encoder keeps per-encode global state, so two concurrent encodes with *different* options can
/// read each other's settings. Holding this for the duration of the AVIF arm is what makes [`encode_into`] safe to
/// call from several threads at once. An AVIF encode runs for tens to hundreds of milliseconds, so the lock costs
/// nothing measurable next to the work it guards.
///
/// A poisoned lock is recovered rather than propagated: the codec state is reset by the next encode either way, and a
/// panic in one caller must not make the encoder permanently unusable — the same reasoning as the `PoisonError`
/// handling in [`memo`](crate::memo) and [`o11y`](crate::o11y).
static AVIF_ENCODE: Mutex<()> = Mutex::new(());

/// Encodes `image` straight into `writer` using the codec selected by `options`, embedding what `metadata` carries
/// where the format can hold it, and reports what was embedded.
///
/// Every codec here except BMP, GIF and TIFF takes a plain [`Write`], so the encoded bytes go to the destination as
/// they are produced rather than being accumulated first. The three that need `Write + Seek` are the only ones that
/// buffer, into a `Cursor` of their own — which keeps that requirement from reaching the caller's sink. JPEG, PNG and
/// WebP also buffer, but only when there is something to splice into the finished file: EXIF, XMP or IPTC (and, for
/// WebP, an ICC profile), whose segments or chunks are inserted once the codec has finished.
///
/// Embedding never fails the encode. A block the format cannot hold, or one too large for it, is left out and
/// reported `false` in the returned [`Embedded`].
pub(super) fn encode_into<W: Write>(
    image: &DynamicImage,
    options: EncodeOptions,
    metadata: &Metadata,
    writer: &mut W,
) -> Result<Embedded> {
    // Ahead of the `match`, so no codec below is reachable without it. See [`make_compatible`] for what it converts.
    let converted = make_compatible(image, options.format());
    let image = converted.as_ref().unwrap_or(image);

    let profile = metadata.icc_profile.as_deref();
    let blocks = ToWrite::of(metadata);
    let mut embedded = Embedded::default();

    match options {
        // These need `Seek` as well, which a bare `Write` cannot offer, so they encode into a rewindable buffer and
        // the bytes are handed on from there. Done inline rather than through a second entry point that called
        // straight back into this one: the two were mutually recursive, and the compatibility step above had to be
        // stated at both ends to cover whichever was entered first.
        EncodeOptions::Bmp | EncodeOptions::Gif => {
            let format = options
                .format()
                .to_image_format()
                .expect("bmp and gif map to image::ImageFormat");

            let mut seekable = Cursor::new(Vec::new());
            image.write_to(&mut seekable, format)?;
            writer.write_all(&seekable.into_inner())?;
        }
        // Through its encoder rather than `write_to`, which is the same encoder with no way to hand it a profile.
        EncodeOptions::Tiff => {
            let mut seekable = Cursor::new(Vec::new());
            let mut encoder = TiffEncoder::new(&mut seekable);
            embedded.icc_profile = offer_profile(&mut encoder, profile, usize::MAX);
            image.write_with_encoder(encoder)?;
            writer.write_all(&seekable.into_inner())?;
        }
        EncodeOptions::Jpeg { quality } => {
            let encode = |sink: &mut dyn Write| -> Result<bool> {
                let mut encoder = JpegEncoder::new_with_quality(sink, quality);
                let icc = offer_profile(&mut encoder, profile, JPEG_MAX_ICC_PROFILE_LEN);
                image.write_with_encoder(encoder)?;
                Ok(icc)
            };
            embedded.icc_profile = spliced(writer, ImageFormat::Jpeg, blocks, &mut embedded, encode)?;
        }
        EncodeOptions::Png { compression, filter } => {
            let encode = |sink: &mut dyn Write| -> Result<bool> {
                let mut encoder = PngEncoder::new_with_quality(sink, compression.into(), filter.into());
                let icc = offer_profile(&mut encoder, profile, usize::MAX);
                image.write_with_encoder(encoder)?;
                Ok(icc)
            };
            embedded.icc_profile = spliced(writer, ImageFormat::Png, blocks, &mut embedded, encode)?;
        }
        EncodeOptions::Avif {
            quality,
            speed,
            threads,
        } => {
            let mut encoder = ::avif::AvifEncoder::new(writer).with_quality(quality).with_speed(speed);
            if let Some(threads) = threads {
                encoder = encoder.with_threads(threads);
            }
            // Held across the encode itself, not just the encoder's construction: the global state the codec keeps is
            // read as it runs.
            let _guard = AVIF_ENCODE.lock().unwrap_or_else(PoisonError::into_inner);
            image.write_with_encoder(encoder)?;
        }
        EncodeOptions::Heif {
            quality,
            preset,
            chroma,
        } => {
            let encoder = ::heif::HeifEncoder::new(writer)
                .with_quality(quality)
                .with_preset(preset)
                .with_chroma(chroma);
            image.write_with_encoder(encoder)?;
        }
        EncodeOptions::WebP {
            quality,
            quality_alpha,
            compression,
            lossless,
            threads,
        } => {
            fn encoder<S: Write>(sink: S, options: (u8, u8, u8, bool, bool)) -> ::webp::WebpEncoder<S> {
                let (quality, quality_alpha, compression, lossless, threads) = options;
                ::webp::WebpEncoder::new(sink)
                    .with_quality(quality)
                    .with_quality_alpha(quality_alpha)
                    .with_compression(compression)
                    .with_lossless(lossless)
                    .with_threads(threads)
            }
            let settings = (quality, quality_alpha, compression, lossless, threads);
            let chunks = riff::Chunks {
                icc: profile.filter(|profile| profile.len() <= MAX_ICC_PROFILE_LEN),
                exif: blocks.exif,
                xmp: blocks.xmp,
            };
            if chunks.is_empty() {
                // Nothing to add, so nothing to rewrite: streamed exactly as before.
                image.write_with_encoder(encoder(writer, settings))?;
            } else {
                let mut plain = Vec::new();
                image.write_with_encoder(encoder(&mut plain, settings))?;
                match riff::with_chunks(&plain, chunks) {
                    Some(tagged) => {
                        writer.write_all(&tagged)?;
                        embedded.icc_profile = chunks.icc.is_some();
                        embedded.exif = chunks.exif.is_some();
                        embedded.xmp = chunks.xmp.is_some();
                    }
                    // The encoder's own output, written as it stands, rather than no file at all.
                    None => writer.write_all(&plain)?,
                }
            }
        }
    }

    Ok(embedded)
}

/// Encodes through `encode` straight into `writer` when there are no `blocks` to splice into `format`, or into a buffer
/// they are spliced into before it is written; records in `embedded` which blocks were written, and passes on what
/// `encode` answered (whether the codec took the ICC profile).
///
/// A splice that cannot walk the encoder's output — not expected, the output being this crate's own — writes that
/// output as it stands and reports the blocks unwritten, rather than failing the encode.
fn spliced<W: Write>(
    writer: &mut W,
    format: ImageFormat,
    blocks: ToWrite<'_>,
    embedded: &mut Embedded,
    encode: impl FnOnce(&mut dyn Write) -> Result<bool>,
) -> Result<bool> {
    if blocks.is_empty() {
        return encode(writer);
    }
    let mut plain = Vec::new();
    let icc = encode(&mut plain)?;
    match blocks::splice(&plain, format, blocks) {
        Some((out, wrote)) => {
            writer.write_all(&out)?;
            embedded.exif = wrote.exif;
            embedded.xmp = wrote.xmp;
            embedded.iptc = wrote.iptc;
            embedded.iso_gain_map = wrote.iso_gain_map;
            embedded.gain_map = wrote.gain_map;
        }
        None => writer.write_all(&plain)?,
    }
    Ok(icc)
}

/// The largest profile the `image` crate's JPEG encoder splits across `APP2` segments: 255 segments of 65,519 bytes.
/// Larger, and it refuses the whole encode rather than the profile, so a profile over it is never handed to it.
const JPEG_MAX_ICC_PROFILE_LEN: usize = 255 * (65_533 - 14);

/// Hands `profile` to `encoder` if there is one no larger than `limit` (nor [`MAX_ICC_PROFILE_LEN`]), and answers
/// whether the encoder took it.
fn offer_profile(encoder: &mut impl ImageEncoder, profile: Option<&[u8]>, limit: usize) -> bool {
    match profile {
        Some(profile) if profile.len() <= limit.min(MAX_ICC_PROFILE_LEN) => {
            encoder.set_icc_profile(profile.to_vec()).is_ok()
        }
        _ => false,
    }
}
