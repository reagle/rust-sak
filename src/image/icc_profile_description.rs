use super::icc;

/// Returns the human-readable description an ICC `profile` carries — `Display P3`, `sRGB IEC61966-2.1` — the same text
/// [`ImageInfo::color_profile`](super::ImageInfo::color_profile) reports for a file embedding it.
///
/// Only the `desc` tag is read: the ICC v2 ASCII form and the v4 multi-localized form (English preferred). The text is
/// trimmed and capped at 64 characters, since it is whatever the profile's author wrote. `None` for anything that is
/// not a well-formed profile with a non-empty description; no input panics.
///
/// ```
/// use rust_sak::image::icc_profile_description;
///
/// assert_eq!(icc_profile_description(b"not a profile"), None);
/// ```
pub fn icc_profile_description(profile: &[u8]) -> Option<String> {
    icc::description(profile)
}
