//! XMP packets as text: the check made before one is carried, and the joining and splitting of JPEG's extended XMP.
//!
//! JPEG holds at most 65,502 bytes of XMP in one `APP1` segment. A larger packet is split, by the XMP specification
//! (Part 3, 1.1.3.1), into a *standard* packet that names a GUID in `xmpNote:HasExtendedXMP`, and an *extended* one
//! carried in numbered `APP1` chunks under that GUID — the MD5 of the extended packet, as 32 upper-case hex digits.
//! Every other format holds a packet of any size whole, so [`Metadata::xmp`](crate::image::Metadata::xmp) is always one
//! whole packet: joined here on the way in, split again on the way out to a JPEG.
//!
//! **No XML parser is used**, so there is no entity expansion, no DTD and no recursion to bound: joining moves the
//! extended packet's `rdf:RDF` contents, as text, in front of the standard packet's `</rdf:RDF>`, and splitting wraps
//! the packet unchanged. Both work on the `rdf:` and `xmpNote:` prefixes every XMP writer uses; a packet that spells
//! them otherwise is carried as its standard part alone.

use super::MAX_BLOCK_LEN;

/// The most XMP one JPEG `APP1` segment holds, by the XMP specification.
pub(super) const MAX_STANDARD_LEN: usize = 65_502;

/// Whether `packet` can be carried as XMP: present, within [`MAX_BLOCK_LEN`], and UTF-8, as XMP in every container
/// this module reads must be.
pub(super) fn plausible(packet: &[u8]) -> bool {
    !packet.is_empty() && packet.len() <= MAX_BLOCK_LEN && std::str::from_utf8(packet).is_ok()
}

/// The standard packet `standard` with the extended packet `extended` (whose GUID is `guid`) joined into it: the
/// now-dangling `xmpNote:HasExtendedXMP` reference removed (with its `rdf:Description`, if that held nothing else), then
/// the extended packet's `rdf:RDF` contents moved in front of `standard`'s `</rdf:RDF>` — or put in place of its
/// contents, if nothing else was left there. `None` when either is not the shape that allows it, so the caller keeps
/// `standard` alone.
///
/// Replacing an emptied standard part is what makes a packet [`split`] for a JPEG come back with exactly the `rdf:RDF`
/// contents it went out with, and a second round trip byte for byte the same as the first.
///
/// The namespaces the extended packet declares on its `x:xmpmeta` and `rdf:RDF` — which its descriptions may rely on —
/// are declared on the standard packet's `rdf:RDF` too, where it does not already bind them; a prefix the two bind to
/// different namespaces, or a default namespace, makes the join refuse rather than change what a name means.
pub(super) fn join(standard: &[u8], guid: &str, extended: &[u8]) -> Option<Vec<u8>> {
    let standard = std::str::from_utf8(standard).ok()?;
    let extended = std::str::from_utf8(extended).ok()?;

    let (body_at, body) = rdf_body(extended)?;
    let carried = declarations(&extended[..body_at])?;
    let without = without_reference(standard, guid)?;
    let (start, _) = rdf_body(&without)?;
    let present = declarations(&without[..start])?;
    let mut added = String::new();
    for (prefix, quote, uri) in carried {
        match present.iter().find(|(bound, ..)| *bound == prefix) {
            Some((_, _, bound_uri)) if *bound_uri == uri => {}
            Some(_) => return None,
            None => added.push_str(&format!(" xmlns:{prefix}={quote}{uri}{quote}")),
        }
    }
    // Inside the `rdf:RDF` start tag, before its `>`.
    let without = format!("{}{}{}", &without[..start - 1], added, &without[start - 1..]);
    let (start, remaining) = rdf_body(&without)?;
    let (from, to) = if remaining.trim().is_empty() {
        (start, start + remaining.len())
    } else {
        (start + remaining.len(), start + remaining.len())
    };

    let mut joined = String::with_capacity(without.len() + body.len());
    joined.push_str(&without[..from]);
    joined.push_str(body);
    joined.push_str(&without[to..]);
    (joined.len() <= MAX_BLOCK_LEN).then(|| joined.into_bytes())
}

/// The `xmlns:prefix` declarations in `tags` — the start tags in front of an `rdf:RDF`'s contents — as `(prefix, quote,
/// value as written)`; `None` if one is not well-formed or a default namespace (`xmlns=`) is declared.
fn declarations(tags: &str) -> Option<Vec<(&str, char, &str)>> {
    let mut found = Vec::new();
    let mut rest = tags;
    while let Some(at) = rest.find("xmlns") {
        let after = &rest[at + "xmlns".len()..];
        let Some(named) = after.strip_prefix(':') else {
            // `xmlns=`, a default namespace: moving descriptions under it would change their unprefixed names.
            if after.trim_start().starts_with('=') {
                return None;
            }
            rest = after;
            continue;
        };
        let name_end = named.find(|c: char| c == '=' || c.is_whitespace())?;
        let prefix = &named[..name_end];
        let value = named[name_end..].trim_start().strip_prefix('=')?.trim_start();
        let quote = value.chars().next().filter(|quote| matches!(quote, '"' | '\''))?;
        let close = value[1..].find(quote)?;
        found.push((prefix, quote, &value[1..1 + close]));
        rest = &value[1 + close + 1..];
    }
    Some(found)
}

/// The GUID the standard packet's `xmpNote:HasExtendedXMP` names, element or attribute: the 32 hex digits that follow
/// the property's name, past the `>` or `="` between them.
pub(super) fn extended_guid(packet: &[u8]) -> Option<&str> {
    let at = find(packet, b"HasExtendedXMP")? + b"HasExtendedXMP".len();
    let rest = &packet[at..];
    let start = rest
        .iter()
        .position(|byte| !matches!(byte, b'>' | b'=' | b'"' | b'\'' | b' ' | b'\t' | b'\r' | b'\n'))?;
    let guid = rest.get(start..start + 32)?;
    guid.iter()
        .all(u8::is_ascii_hexdigit)
        .then(|| std::str::from_utf8(guid).ok())
        .flatten()
}

/// Where the `rdf:RDF` element's contents start, and the contents: what lies between its start tag and its end tag.
fn rdf_body(packet: &str) -> Option<(usize, &str)> {
    let open = packet.find("<rdf:RDF")?;
    let start = open + packet[open..].find('>')? + 1;
    let end = packet.rfind("</rdf:RDF>")?;
    Some((start, packet.get(start..end)?))
}

/// `packet` with its `xmpNote:HasExtendedXMP` naming `guid` removed, whether written as an element or as an attribute.
/// An element whose `rdf:Description` is left empty takes the description, and the whitespace before it, with it.
/// `None` if the reference is not there in either form.
fn without_reference(packet: &str, guid: &str) -> Option<String> {
    const NAME: &str = "xmpNote:HasExtendedXMP";
    const OPEN: &str = "<rdf:Description";
    const CLOSE: &str = "</rdf:Description>";

    let element = format!("<{NAME}>{guid}</{NAME}>");
    if let Some(at) = packet.find(&element) {
        let after = at + element.len();
        // The description around it, if it holds nothing else.
        let emptied = (|| {
            let open = packet[..at].rfind(OPEN)?;
            let tag_end = open + packet[open..at].find('>')?;
            let close = after + packet[after..].find(CLOSE)?;
            let empty = !packet[..=tag_end].ends_with("/>")
                && packet[tag_end + 1..at].trim().is_empty()
                && packet[after..close].trim().is_empty();
            empty.then(|| (packet[..open].trim_end().len(), close + CLOSE.len()))
        })();
        let (from, to) = emptied.unwrap_or((at, after));
        return Some(format!("{}{}", &packet[..from], &packet[to..]));
    }
    for quote in ['"', '\''] {
        let attribute = format!("{NAME}={quote}{guid}{quote}");
        if let Some(at) = packet.find(&attribute) {
            // The whitespace that separated it from what came before goes with it.
            let start = packet[..at].trim_end().len();
            return Some(format!("{}{}", &packet[..start], &packet[at + attribute.len()..]));
        }
    }
    None
}

/// `packet` as JPEG must hold it once it exceeds [`MAX_STANDARD_LEN`]: a standard packet naming a GUID, and the extended
/// packet that GUID is the MD5 of, as the XMP specification lays them out.
///
/// The extended packet is the whole of `packet`, without its `<?xpacket?>` wrapper (which the specification leaves out
/// of extended XMP); the standard packet holds only the reference. Every property is kept and nothing in the packet is
/// rewritten — choosing which properties are important enough to stay in the standard part, as Adobe's own writer does,
/// would mean interpreting the XMP, which belongs to the caller.
pub(super) fn split(packet: &[u8]) -> (Vec<u8>, String, Vec<u8>) {
    let extended = unwrapped(packet).to_vec();
    let guid = format!("{:X}", md5::compute(&extended));
    let standard = format!(
        "<?xpacket begin='\u{feff}' id='W5M0MpCehiHzreSzNTczkc9d'?>\n\
         <x:xmpmeta xmlns:x='adobe:ns:meta/'>\n\
         <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>\n \
         <rdf:Description rdf:about='' xmlns:xmpNote='http://ns.adobe.com/xmp/note/'>\n  \
         <xmpNote:HasExtendedXMP>{guid}</xmpNote:HasExtendedXMP>\n \
         </rdf:Description>\n\
         </rdf:RDF>\n\
         </x:xmpmeta>\n\
         <?xpacket end='w'?>"
    );
    (standard.into_bytes(), guid, extended)
}

/// `packet` without its leading `<?xpacket begin…?>` and trailing `<?xpacket end…?>` instructions and the padding
/// around them.
fn unwrapped(packet: &[u8]) -> &[u8] {
    let mut packet = packet;
    if packet.starts_with(b"<?xpacket")
        && let Some(end) = find(packet, b"?>")
    {
        packet = &packet[end + 2..];
    }
    if let Some(trailer) = rfind(packet, b"<?xpacket") {
        packet = &packet[..trailer];
    }
    packet.trim_ascii()
}

/// Whether `needle` occurs anywhere in `packet`.
pub(super) fn contains(packet: &[u8], needle: &[u8]) -> bool {
    find(packet, needle).is_some()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: &str = "1B4BA161EEAB8BCF7AE8692FF5AAD065";

    fn standard(reference: &str) -> String {
        format!(
            "<?xpacket begin='\u{feff}' id='W5M0MpCehiHzreSzNTczkc9d'?>\n<x:xmpmeta xmlns:x='adobe:ns:meta/'>\n\
             <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>\n{reference}\n\
             <rdf:Description rdf:about='' xmlns:xmp='http://ns.adobe.com/xap/1.0/'><xmp:Rating>3</xmp:Rating>\
             </rdf:Description>\n</rdf:RDF>\n</x:xmpmeta>\n<?xpacket end='w'?>"
        )
    }

    const EXTENDED: &str = "<x:xmpmeta xmlns:x='adobe:ns:meta/'>\n<rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>\n\
        <rdf:Description rdf:about='' xmlns:dc='http://purl.org/dc/elements/1.1/'><dc:description>long</dc:description>\
        </rdf:Description>\n</rdf:RDF>\n</x:xmpmeta>\n";

    #[test]
    fn an_element_reference_is_joined_and_removed() {
        let main = standard(&format!(
            "<rdf:Description rdf:about='' xmlns:xmpNote='http://ns.adobe.com/xmp/note/'>\
             <xmpNote:HasExtendedXMP>{GUID}</xmpNote:HasExtendedXMP></rdf:Description>"
        ));
        let joined = String::from_utf8(join(main.as_bytes(), GUID, EXTENDED.as_bytes()).unwrap()).unwrap();

        assert!(!joined.contains("HasExtendedXMP"), "{joined}");
        assert!(joined.contains("<dc:description>long</dc:description>"));
        assert!(joined.contains("<xmp:Rating>3</xmp:Rating>"));
        // The moved description sits inside the one `rdf:RDF`, before its end tag.
        assert_eq!(joined.matches("<rdf:RDF").count(), 1);
        assert!(joined.find("long").unwrap() < joined.find("</rdf:RDF>").unwrap());
        assert!(joined.ends_with("<?xpacket end='w'?>"));
    }

    #[test]
    fn an_attribute_reference_is_joined_and_removed() {
        for quote in ['"', '\''] {
            let main = standard(&format!(
                "<rdf:Description rdf:about='' xmlns:xmpNote='http://ns.adobe.com/xmp/note/'\n  \
                 xmpNote:HasExtendedXMP={quote}{GUID}{quote}/>"
            ));
            let joined = String::from_utf8(join(main.as_bytes(), GUID, EXTENDED.as_bytes()).unwrap()).unwrap();
            assert!(!joined.contains("HasExtendedXMP"), "{joined}");
            assert!(
                joined.contains("xmlns:xmpNote='http://ns.adobe.com/xmp/note/'/>"),
                "{joined}"
            );
        }
    }

    #[test]
    fn a_packet_that_does_not_name_the_guid_is_not_joined() {
        let main = standard("");
        assert_eq!(join(main.as_bytes(), GUID, EXTENDED.as_bytes()), None);
        assert_eq!(join(b"\xff\xfe", GUID, EXTENDED.as_bytes()), None);
        assert_eq!(join(main.as_bytes(), GUID, b"no rdf here"), None);
    }

    #[test]
    fn an_emptied_description_goes_with_its_reference() {
        let main = standard(&format!(
            "<rdf:Description rdf:about='' xmlns:xmpNote='http://ns.adobe.com/xmp/note/'>\n  \
             <xmpNote:HasExtendedXMP>{GUID}</xmpNote:HasExtendedXMP>\n </rdf:Description>"
        ));
        let joined = String::from_utf8(join(main.as_bytes(), GUID, EXTENDED.as_bytes()).unwrap()).unwrap();
        assert!(!joined.contains("xmpNote"), "{joined}");
        assert_eq!(joined.matches("<rdf:Description").count(), 2, "{joined}");
    }

    #[test]
    fn a_split_packet_names_the_md5_of_its_extended_part_and_joins_back() {
        let packet = standard("").into_bytes();
        let (main, guid, extended) = split(&packet);

        assert_eq!(guid, format!("{:X}", md5::compute(&extended)));
        assert_eq!(guid.len(), 32);
        assert!(!extended.starts_with(b"<?xpacket") && extended.ends_with(b"</x:xmpmeta>"));
        assert!(main.len() < MAX_STANDARD_LEN && plausible(&main));

        // The `rdf:RDF` contents come back exactly; only the wrapper around them is the split's own.
        let joined = join(&main, &guid, &extended).unwrap();
        let original = std::str::from_utf8(&packet).unwrap();
        let text = std::str::from_utf8(&joined).unwrap();
        assert_eq!(rdf_body(text).unwrap().1, rdf_body(original).unwrap().1);
        assert!(!text.contains("HasExtendedXMP"));

        // And a second round trip changes nothing at all.
        let (main, guid, extended) = split(&joined);
        assert_eq!(join(&main, &guid, &extended).unwrap(), joined);
    }

    #[test]
    fn namespaces_the_extended_packet_declares_outside_its_descriptions_come_along() {
        // As the review's x-extns repro: `dc` is declared only on the extended packet's `x:xmpmeta`.
        let extended = "<x:xmpmeta xmlns:x='adobe:ns:meta/' xmlns:dc='http://purl.org/dc/elements/1.1/'>\
            <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>\
            <rdf:Description rdf:about=''><dc:description>long</dc:description></rdf:Description></rdf:RDF></x:xmpmeta>";
        let main = standard(&format!(
            "<rdf:Description rdf:about='' xmlns:xmpNote='http://ns.adobe.com/xmp/note/'>\
             <xmpNote:HasExtendedXMP>{GUID}</xmpNote:HasExtendedXMP></rdf:Description>"
        ));
        let joined = String::from_utf8(join(main.as_bytes(), GUID, extended.as_bytes()).unwrap()).unwrap();
        let rdf_tag = &joined[joined.find("<rdf:RDF").unwrap()..];
        let rdf_tag = &rdf_tag[..rdf_tag.find('>').unwrap()];
        assert!(
            rdf_tag.contains("xmlns:dc='http://purl.org/dc/elements/1.1/'"),
            "{joined}"
        );
        // Declared once, and the joined packet is well-formed XML with every prefix bound.
        assert_eq!(joined.matches("xmlns:dc=").count(), 1);

        // A prefix bound to another namespace, or a default namespace, refuses the join.
        let clash = extended.replace("purl.org/dc/elements/1.1/", "example.com/other/");
        let main_with_dc = main.replace("<rdf:RDF ", "<rdf:RDF xmlns:dc='http://purl.org/dc/elements/1.1/' ");
        assert_eq!(join(main_with_dc.as_bytes(), GUID, clash.as_bytes()), None);
        let default = extended.replace("xmlns:dc=", "xmlns=");
        assert_eq!(join(main.as_bytes(), GUID, default.as_bytes()), None);
    }

    #[test]
    fn the_guid_is_read_from_either_form() {
        let element = format!("<xmpNote:HasExtendedXMP>{GUID}</xmpNote:HasExtendedXMP>");
        let attribute = format!("xmpNote:HasExtendedXMP = \"{GUID}\"");
        assert_eq!(extended_guid(element.as_bytes()), Some(GUID));
        assert_eq!(extended_guid(attribute.as_bytes()), Some(GUID));
        assert_eq!(
            extended_guid(b"<xmpNote:HasExtendedXMP>not-hex</xmpNote:HasExtendedXMP>"),
            None
        );
        assert_eq!(extended_guid(b"<xmpNote:HasExtendedXMP>1B4B"), None);
        assert_eq!(extended_guid(b"nothing"), None);
    }

    #[test]
    fn plausibility_is_utf8_and_size() {
        assert!(plausible(b"<x:xmpmeta/>"));
        assert!(!plausible(b""));
        assert!(!plausible(b"\xc3\x28"));
    }
}
