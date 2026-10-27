//! XMP packets as text: the check made before one is carried, and the joining and splitting of JPEG's extended XMP.
//!
//! JPEG holds at most 65,502 bytes of XMP in one `APP1` segment. A larger packet is split, by the XMP specification
//! (Part 3, 1.1.3.1), into a *standard* packet that names a GUID in `xmpNote:HasExtendedXMP`, and an *extended* one
//! carried in numbered `APP1` chunks under that GUID — the MD5 of the extended packet, as 32 upper-case hex digits.
//! Every other format holds a packet of any size whole, so [`Metadata::xmp`](crate::image::Metadata::xmp) is always one
//! whole packet: joined here on the way in, split again on the way out to a JPEG.
//!
//! **No XML parser is used**, so there is no entity expansion, no DTD and no recursion to bound: joining moves the
//! extended packet's `rdf:RDF` contents, as text, in front of the standard packet's `</rdf:RDF>`, and splitting cuts
//! the packet's top-level properties out of their descriptions, as text, by walking its tags (comments, CDATA and quoted
//! values passed over) — keeping the gain-map declaration and the largest share of the rest that fits in the standard
//! part, as the Pixel camera does. Both work on the `rdf:` and `xmpNote:` prefixes every XMP writer uses; a packet that
//! spells them otherwise is carried as its standard part alone.

use std::ops::Range;

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

/// The namespaces of the properties a reader must find in the standard packet, which therefore never move into extended
/// XMP: `hdrgm:` (Adobe's and Ultra HDR's gain-map declaration) and the Google container directory, `Container:` with
/// the `Item:` entries inside it, that locates the gain map. Matched by URI, whatever prefix the packet binds to them.
const STANDARD_NAMESPACES: [&str; 3] = [
    "http://ns.adobe.com/hdr-gain-map/1.0/",
    "http://ns.google.com/photos/1.0/container/",
    "http://ns.google.com/photos/1.0/container/item/",
];

/// The namespace of `xmpNote:HasExtendedXMP`.
const XMP_NOTE: &str = "http://ns.adobe.com/xmp/note/";

/// `packet` as JPEG must hold it once it exceeds [`MAX_STANDARD_LEN`]: a standard packet naming a GUID, and the extended
/// packet that GUID is the MD5 of, as the XMP specification lays them out.
///
/// **As Adobe's writer and the Pixel camera do, the largest top-level properties move and the rest stay**: every
/// property of the packet's `rdf:Description`s — an element or an attribute — is a candidate, and they move into the
/// extended packet largest first (ties by their text, so the choice does not depend on where they sit) until the
/// standard packet, its reference included, fits one segment. A property in [`STANDARD_NAMESPACES`] never moves, since a
/// reader that looks for a gain map reads the standard packet only; a stale `xmpNote:HasExtendedXMP` is dropped from
/// both. Nothing is rewritten: a description keeps its text, less the properties that went to the other part (one whose
/// every property went is left out of that part), and its namespace declarations go to both. The standard packet keeps
/// the packet's own `<?xpacket?>`, `x:xmpmeta` and `rdf:RDF` wrapper, with the reference as a last description; the
/// extended packet has the same `x:xmpmeta` and `rdf:RDF` start tags, without the `<?xpacket?>` (which the specification
/// leaves out of extended XMP). [`join`] puts the moved descriptions back at the end of the `rdf:RDF`, so a packet once
/// split and joined splits and joins again byte for byte the same.
///
/// A packet whose `rdf:RDF` is not a run of elements this can walk (text between them, a default namespace, a tag left
/// open), or whose properties that must stay are too large alone, is split whole instead: every property goes into the
/// extended packet and the standard one holds only the reference — no metadata is lost, but a reader of the standard
/// packet alone no longer finds a gain map there, which the caller checks for.
pub(super) fn split(packet: &[u8]) -> (Vec<u8>, String, Vec<u8>) {
    std::str::from_utf8(packet)
        .ok()
        .and_then(split_by_property)
        .unwrap_or_else(|| split_whole(packet))
}

/// [`split`] by property; `None` where it must split whole.
fn split_by_property(packet: &str) -> Option<(Vec<u8>, String, Vec<u8>)> {
    let (start, body) = rdf_body(packet)?;
    let end = start + body.len();
    let outer = declarations(&packet[..start])?;
    let descriptions = descriptions(body, &outer)?;

    // What the standard packet would weigh as each property leaves it: a description goes too, once nothing is left in
    // it. A reference description is added, its GUID always 32 digits.
    let mut size = packet.len() + reference("").len() + 32;
    let mut left: Vec<usize> = Vec::with_capacity(descriptions.len());
    let leaves = |size: &mut usize, left: &mut [usize], at: usize, span: &Range<usize>| {
        *size -= span.len();
        left[at] -= 1;
        if left[at] == 0 {
            let description = &descriptions[at];
            *size -= description.span.len() - description.properties.iter().map(|p| p.span.len()).sum::<usize>();
        }
    };
    let mut fate: Vec<Vec<Fate>> = Vec::with_capacity(descriptions.len());
    for (at, description) in descriptions.iter().enumerate() {
        left.push(description.properties.len());
        fate.push(vec![Fate::Stays; description.properties.len()]);
        for (index, property) in description.properties.iter().enumerate() {
            if property.stale {
                fate[at][index] = Fate::Dropped;
                leaves(&mut size, &mut left, at, &property.span);
            }
        }
    }
    let mut movable: Vec<(usize, usize)> = descriptions
        .iter()
        .enumerate()
        .flat_map(|(at, description)| {
            let movable = description.properties.iter().enumerate();
            movable
                .filter(|(_, property)| !property.stale && !property.standard)
                .map(move |(index, _)| (at, index))
        })
        .collect();
    let span = |&(at, index): &(usize, usize)| descriptions[at].properties[index].span.clone();
    movable.sort_by(|a, b| {
        let (a, b) = (span(a), span(b));
        b.len().cmp(&a.len()).then_with(|| body[a].cmp(&body[b]))
    });
    for candidate in &movable {
        if size <= MAX_STANDARD_LEN {
            break;
        }
        fate[candidate.0][candidate.1] = Fate::Moves;
        leaves(&mut size, &mut left, candidate.0, &span(candidate));
    }
    if size > MAX_STANDARD_LEN {
        return None;
    }

    // The standard part: the body less what moved or was dropped, then the reference.
    let mut removed: Vec<Range<usize>> = Vec::new();
    let mut moved = String::new();
    for ((description, fates), left) in descriptions.iter().zip(&fate).zip(&left) {
        let properties = description.properties.iter().zip(fates);
        if *left == 0 && !description.properties.is_empty() {
            removed.push(description.span.clone());
        } else {
            removed.extend(
                properties
                    .clone()
                    .filter(|(_, fate)| **fate != Fate::Stays)
                    .map(|(p, _)| p.span.clone()),
            );
        }
        if fates.contains(&Fate::Moves) {
            let others: Vec<_> = properties
                .filter(|(_, fate)| **fate != Fate::Moves)
                .map(|(p, _)| p.span.clone())
                .collect();
            moved.push_str(&without(body, description.span.clone(), &others));
        }
    }
    let kept = without(body, 0..body.len(), &removed);
    let wrapper = packet[..start]
        .find("<x:xmpmeta")
        .unwrap_or_else(|| packet[..start].find("<rdf:RDF").unwrap_or(0));
    let closing = if wrapper < packet[..start].find("<rdf:RDF")? {
        "</rdf:RDF>\n</x:xmpmeta>"
    } else {
        "</rdf:RDF>"
    };
    let extended = format!("{}{moved}{closing}", &packet[wrapper..start]).into_bytes();
    let guid = format!("{:X}", md5::compute(&extended));
    // The reference goes last, after any whitespace before `</rdf:RDF>`, which `join` takes away with it: the moved
    // descriptions then come back straight after the kept ones, as a further split finds and leaves them.
    let standard = format!("{}{kept}{}{}", &packet[..start], reference(&guid), &packet[end..]);
    debug_assert_eq!(standard.len(), size, "the standard packet weighs what was reckoned");
    (standard.len() <= MAX_STANDARD_LEN).then(|| (standard.into_bytes(), guid, extended))
}

/// The description that names the extended packet `guid`, with the whitespace that sets it apart.
fn reference(guid: &str) -> String {
    format!(
        "\n <rdf:Description rdf:about='' xmlns:xmpNote='{XMP_NOTE}'>\n  \
         <xmpNote:HasExtendedXMP>{guid}</xmpNote:HasExtendedXMP>\n </rdf:Description>"
    )
}

/// `text[within]` without the ranges `removed` (which lie inside it and do not overlap).
fn without(text: &str, within: Range<usize>, removed: &[Range<usize>]) -> String {
    let mut removed = removed.to_vec();
    removed.sort_by_key(|range| range.start);
    let mut out = String::with_capacity(within.len());
    let mut at = within.start;
    for range in removed {
        out.push_str(&text[at..range.start]);
        at = range.end;
    }
    out.push_str(&text[at..within.end]);
    out
}

/// Where one property of a split goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fate {
    Stays,
    Moves,
    Dropped,
}

/// One top-level element of an `rdf:RDF`, as offsets into its contents: an `rdf:Description` and its properties, or
/// any other element, which is its own one property.
struct Description {
    /// The element and the whitespace in front of it.
    span: Range<usize>,
    properties: Vec<Property>,
}

/// One property of a [`Description`]: an attribute or a child element, with the whitespace in front of it.
struct Property {
    span: Range<usize>,
    /// In one of the [`STANDARD_NAMESPACES`]: it never moves.
    standard: bool,
    /// An `xmpNote:HasExtendedXMP` left from an earlier split: it is dropped.
    stale: bool,
}

/// The top-level elements of `body`, an `rdf:RDF`'s contents, whose outer start tags declare `outer`; `None` if
/// anything but whitespace, comments and elements is there, or a tag is not closed.
fn descriptions(body: &str, outer: &[(&str, char, &str)]) -> Option<Vec<Description>> {
    const DESCRIPTION: &str = "<rdf:Description";

    let mut found = Vec::new();
    let mut after = 0;
    for (at, end) in elements(body, 0, body.len())? {
        let span = body[..at].trim_end().len().max(after)..end;
        after = end;
        let named = body[at..]
            .strip_prefix(DESCRIPTION)
            .is_some_and(|rest| rest.starts_with(is_tag_break));
        if !named {
            found.push(Description {
                properties: vec![Property {
                    span: span.clone(),
                    standard: false,
                    stale: false,
                }],
                span,
            });
            continue;
        }
        let tag_end = tag_end(body, at)?;
        let tag = &body[at..=tag_end];
        let own = declarations(tag)?;
        let mut properties = Vec::new();
        for (range, name) in attributes(tag) {
            if name.starts_with("xmlns") || name.starts_with("rdf:") || name.starts_with("xml:") {
                continue;
            }
            properties.push(property(name, at + range.start..at + range.end, &[&own, outer]));
        }
        if !tag.ends_with("/>") {
            let close = end - "</rdf:Description>".len();
            if !body[close..end].starts_with("</rdf:Description") {
                return None;
            }
            let mut inner = tag_end + 1;
            for (child, child_end) in elements(body, tag_end + 1, close)? {
                let child_tag = &body[child..=tag_end_within(body, child, child_end)?];
                let theirs = declarations(child_tag).unwrap_or_default();
                let name = &child_tag[1..child_tag.find(is_tag_break)?];
                let range = body[..child].trim_end().len().max(inner)..child_end;
                inner = child_end;
                properties.push(property(name, range, &[&theirs, &own, outer]));
            }
        }
        found.push(Description { span, properties });
    }
    Some(found)
}

/// The [`Property`] named `name` at `span`, its prefix looked up in `scopes`, innermost first.
fn property(name: &str, span: Range<usize>, scopes: &[&[(&str, char, &str)]]) -> Property {
    let (prefix, local) = name.split_once(':').unwrap_or(("", name));
    let uri = scopes
        .iter()
        .find_map(|scope| scope.iter().find(|(bound, ..)| *bound == prefix).map(|(.., uri)| *uri));
    Property {
        span,
        standard: uri.is_some_and(|uri| STANDARD_NAMESPACES.contains(&uri)),
        stale: uri == Some(XMP_NOTE) && local == "HasExtendedXMP",
    }
}

/// What ends a tag's name.
fn is_tag_break(c: char) -> bool {
    c.is_whitespace() || c == '/' || c == '>'
}

/// The elements of `text[from..to]`, as `(start, end)`: comments and processing instructions between them are passed
/// over; `None` for anything else that is not whitespace, or an element not closed before `to`.
fn elements(text: &str, from: usize, to: usize) -> Option<Vec<(usize, usize)>> {
    let mut found = Vec::new();
    let mut at = from;
    loop {
        at += text[at..to].len() - text[at..to].trim_start().len();
        let rest = &text[at..to];
        if rest.is_empty() {
            return Some(found);
        } else if rest.starts_with("<!--") {
            at += rest.find("-->")? + 3;
        } else if rest.starts_with("<?") {
            at += rest.find("?>")? + 2;
        } else if rest.starts_with('<') && !rest.starts_with("</") {
            let end = element_end(&text[..to], at)?;
            found.push((at, end));
            at = end;
        } else {
            return None;
        }
    }
}

/// Where the element whose start tag begins at `at` ends: past its end tag, or past its own `/>`.
fn element_end(text: &str, at: usize) -> Option<usize> {
    let mut depth = 0_usize;
    let mut at = at;
    loop {
        let rest = &text[at..];
        if rest.starts_with("<!--") {
            at += rest.find("-->")? + 3;
        } else if rest.starts_with("<![CDATA[") {
            at += rest.find("]]>")? + 3;
        } else if rest.starts_with("<?") {
            at += rest.find("?>")? + 2;
        } else if rest.starts_with("</") {
            at += rest.find('>')? + 1;
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(at);
            }
        } else if rest.starts_with('<') {
            let close = tag_end(text, at)?;
            let empty = text[..close].ends_with('/');
            at = close + 1;
            if !empty {
                depth += 1;
            } else if depth == 0 {
                return Some(at);
            }
        } else {
            at += rest.find('<')?;
        }
    }
}

/// The `>` that closes the tag starting at `at`, past any quoted attribute value.
fn tag_end(text: &str, at: usize) -> Option<usize> {
    let mut quote = None;
    for (index, &byte) in text.as_bytes()[at..].iter().enumerate() {
        match (quote, byte) {
            (None, b'"' | b'\'') => quote = Some(byte),
            (Some(open), _) if byte == open => quote = None,
            (None, b'>') => return Some(at + index),
            _ => {}
        }
    }
    None
}

/// [`tag_end`], within an element that ends at `end`.
fn tag_end_within(text: &str, at: usize, end: usize) -> Option<usize> {
    tag_end(&text[..end], at)
}

/// The attributes of the start tag `tag`, as `(range in tag, with the whitespace in front of it; name)`.
fn attributes(tag: &str) -> Vec<(Range<usize>, &str)> {
    let mut found = Vec::new();
    let Some(mut at) = tag.find(is_tag_break) else {
        return found;
    };
    loop {
        let before = at;
        at += tag[at..].len() - tag[at..].trim_start().len();
        let rest = &tag[at..];
        let Some(name_end) = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .filter(|_| !rest.starts_with(['/', '>']))
        else {
            return found;
        };
        let name = &rest[..name_end];
        let Some(value) = rest[name_end..].trim_start().strip_prefix('=').map(str::trim_start) else {
            return found;
        };
        let Some(quote) = value.chars().next().filter(|quote| matches!(quote, '"' | '\'')) else {
            return found;
        };
        let Some(close) = value[1..].find(quote) else {
            return found;
        };
        let end = tag.len() - value.len() + 1 + close + 1;
        found.push((before..end, name));
        at = end;
    }
}

/// [`split`] whole: an extended packet that is all of `packet` but its `<?xpacket?>` wrapper, and a standard packet that
/// holds only the reference to it.
fn split_whole(packet: &[u8]) -> (Vec<u8>, String, Vec<u8>) {
    let extended = unwrapped(packet).to_vec();
    let guid = format!("{:X}", md5::compute(&extended));
    let standard = format!(
        "<?xpacket begin='\u{feff}' id='W5M0MpCehiHzreSzNTczkc9d'?>\n\
         <x:xmpmeta xmlns:x='adobe:ns:meta/'>\n\
         <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>{}\n\
         </rdf:RDF>\n\
         </x:xmpmeta>\n\
         <?xpacket end='w'?>",
        reference(&guid)
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

    /// A Pixel Ultra HDR packet as Lightroom leaves it: one description per namespace, the gain-map declaration and the
    /// container directory (under Lightroom's `GContainer` prefix, not `Container`), a 93 kB `GCamera:HdrPlusMakernote`,
    /// and `big` more bytes as an attribute beside `hdrgm:Version`.
    fn pixel(big: usize) -> String {
        format!(
            "<?xpacket begin='\u{feff}' id='W5M0MpCehiHzreSzNTczkc9d'?>\n<x:xmpmeta xmlns:x='adobe:ns:meta/'>\n\
             <rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'>\n\n \
             <rdf:Description rdf:about=''\n  xmlns:hdrgm='http://ns.adobe.com/hdr-gain-map/1.0/'\n  \
             xmlns:pad='urn:pad' pad:Pad='{pad}'>\n  <hdrgm:Version>1.0</hdrgm:Version>\n </rdf:Description>\n\n \
             <rdf:Description rdf:about=''\n  xmlns:GCamera='http://ns.google.com/photos/1.0/camera/'>\n  \
             <GCamera:HdrPlusMakernote>{makernote}</GCamera:HdrPlusMakernote>\n  \
             <GCamera:GFileMetadata>SgQIARAB</GCamera:GFileMetadata>\n </rdf:Description>\n\n \
             <!-- a comment -->\n <rdf:Description rdf:about='' xmlns:xmp='http://ns.adobe.com/xap/1.0/' \
             xmp:Rating='3'/>\n\n \
             <rdf:Description rdf:about=''\n  xmlns:GContainer='http://ns.google.com/photos/1.0/container/'\n  \
             xmlns:GItem='http://ns.google.com/photos/1.0/container/item/'>\n  <GContainer:Directory>\n   <rdf:Seq>\n    \
             <rdf:li rdf:parseType='Resource'>\n     <GContainer:Item GItem:Mime='image/jpeg' GItem:Semantic='Primary'/>\
             \n    </rdf:li>\n    <rdf:li rdf:parseType='Resource'>\n     <GContainer:Item rdf:parseType='Resource'>\n      \
             <GItem:Length>{length}</GItem:Length>\n      <GItem:Semantic>GainMap</GItem:Semantic>\n     \
             </GContainer:Item>\n    </rdf:li>\n   </rdf:Seq>\n  </GContainer:Directory>\n </rdf:Description>\n\
             </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end='w'?>",
            pad = "p".repeat(big),
            makernote = "A".repeat(93_282),
            length = 55_742,
        )
    }

    /// Every top-level property of `packet`'s descriptions, as text without the whitespace in front of it, sorted.
    fn properties(packet: &str) -> Vec<String> {
        let (_, body) = rdf_body(packet).unwrap();
        let mut found: Vec<String> = descriptions(body, &[])
            .unwrap()
            .iter()
            .flat_map(|description| {
                description
                    .properties
                    .iter()
                    .map(|p| body[p.span.clone()].trim().to_string())
            })
            .filter(|property| !property.contains("HasExtendedXMP"))
            .collect();
        found.sort();
        found
    }

    #[test]
    fn an_oversized_packet_keeps_what_readers_need_standard_and_moves_its_largest_property() {
        let packet = pixel(10);
        assert!(packet.len() > MAX_STANDARD_LEN);

        let (main, guid, extended) = split(packet.as_bytes());
        let (main, extended) = (
            std::str::from_utf8(&main).unwrap(),
            std::str::from_utf8(&extended).unwrap(),
        );

        // The standard packet fits one segment, names the extended one by its MD5, and keeps the gain-map declaration,
        // the directory and every small property, in the packet's own wrapper.
        assert!(main.len() <= MAX_STANDARD_LEN && plausible(main.as_bytes()));
        assert_eq!(guid, format!("{:X}", md5::compute(extended.as_bytes())));
        assert_eq!(extended_guid(main.as_bytes()), Some(guid.as_str()));
        for kept in [
            "<hdrgm:Version>1.0</hdrgm:Version>",
            "<GContainer:Directory>",
            "<GItem:Length>55742</GItem:Length>",
        ] {
            assert!(main.contains(kept) && !extended.contains(kept), "{kept}");
        }
        assert!(main.contains("pad:Pad='pppppppppp'") && main.contains("xmp:Rating='3'"));
        assert!(main.contains("<GCamera:GFileMetadata>SgQIARAB</GCamera:GFileMetadata>"));
        assert!(main.starts_with("<?xpacket begin=") && main.ends_with("<?xpacket end='w'?>"));
        // Only the makernote moved, in a description of its own namespace, declared.
        assert!(!main.contains("HdrPlusMakernote") && extended.contains(&"A".repeat(93_282)));
        assert!(extended.contains("xmlns:GCamera='http://ns.google.com/photos/1.0/camera/'"));
        assert!(!extended.contains("GFileMetadata") && !extended.contains("<?xpacket"));
        assert_eq!(extended.matches("<rdf:Description").count(), 1, "{extended}");

        // Joined, every property is back; split and joined again, nothing changes.
        let joined = join(main.as_bytes(), &guid, extended.as_bytes()).unwrap();
        let text = std::str::from_utf8(&joined).unwrap();
        assert_eq!(properties(text), properties(&packet));
        assert!(!text.contains("HasExtendedXMP"));
        let (main, guid, extended) = split(&joined);
        assert_eq!(join(&main, &guid, &extended).unwrap(), joined);
    }

    #[test]
    fn properties_move_largest_first_until_the_standard_packet_fits() {
        // Beside the makernote, an attribute too large to stay: both move, and the description they shared with
        // `hdrgm:Version` is split between the two parts.
        let packet = pixel(70_000);
        let (main, guid, extended) = split(packet.as_bytes());
        let (main, extended) = (
            std::str::from_utf8(&main).unwrap(),
            std::str::from_utf8(&extended).unwrap(),
        );

        assert!(main.len() <= MAX_STANDARD_LEN);
        assert!(main.contains("<hdrgm:Version>1.0</hdrgm:Version>") && !main.contains("pad:Pad"));
        assert!(extended.contains("pad:Pad=") && extended.contains("HdrPlusMakernote"));
        assert!(!extended.contains("hdrgm:Version"));
        let joined = join(main.as_bytes(), &guid, extended.as_bytes()).unwrap();
        assert_eq!(properties(std::str::from_utf8(&joined).unwrap()), properties(&packet));
    }

    #[test]
    fn a_stale_reference_is_dropped_from_both_parts() {
        let packet = pixel(10).replace(
            "<hdrgm:Version>",
            "<xmpNote:HasExtendedXMP xmlns:xmpNote='http://ns.adobe.com/xmp/note/'>0000</xmpNote:HasExtendedXMP>\
             <hdrgm:Version>",
        );
        let (main, guid, extended) = split(packet.as_bytes());
        let main = std::str::from_utf8(&main).unwrap();
        assert_eq!(
            main.matches("HasExtendedXMP>").count(),
            2,
            "only the new reference: {main}"
        );
        assert_eq!(extended_guid(main.as_bytes()), Some(guid.as_str()));
        assert!(!std::str::from_utf8(&extended).unwrap().contains("HasExtendedXMP"));
    }

    #[test]
    fn a_packet_that_cannot_be_walked_or_kept_standard_is_split_whole() {
        // Text where only elements belong; a default namespace; required properties alone too large.
        let required = pixel(10).replace(
            "<hdrgm:Version>1.0",
            &format!("<hdrgm:Version>1.0{}", " ".repeat(70_000)),
        );
        for packet in [
            pixel(10).replace("<!-- a comment -->", "stray text"),
            pixel(10).replace("<x:xmpmeta ", "<x:xmpmeta xmlns='urn:default' "),
            required,
        ] {
            let (main, guid, extended) = split(packet.as_bytes());
            let main = std::str::from_utf8(&main).unwrap();
            assert!(main.len() < 1_000 && !main.contains("hdrgm"), "{main}");
            assert_eq!(extended_guid(main.as_bytes()), Some(guid.as_str()));
            assert_eq!(extended, unwrapped(packet.as_bytes()));
            // And it joins back to the whole packet's `rdf:RDF` contents, but for the default namespace, which the
            // join refuses as ever.
            if let Some(joined) = join(main.as_bytes(), &guid, &extended) {
                let text = std::str::from_utf8(&joined).unwrap();
                assert_eq!(rdf_body(text).unwrap().1, rdf_body(&packet).unwrap().1);
            } else {
                assert!(packet.contains("xmlns='urn:default'"));
            }
        }
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
