//! SNAP's GeoTIFF writer stores band names in a DIMAP XML header (private TIFF tag 65000),
//! which GDAL ignores. Read it so SNAP-made files keep their band names.
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const TAG: u16 = 65000;
const MAX_XML: u32 = 256 << 20;

/// 0-based band index -> (name, description).
pub(crate) type Names = HashMap<usize, (String, Option<String>)>;

/// Empty when the file is not a classic TIFF or has no DIMAP header.
pub(crate) fn read(path: &Path) -> Names {
    read_xml(path).map(|x| parse(&x)).unwrap_or_default()
}

fn read_xml(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = [0u8; 8];
    f.read_exact(&mut h).ok()?;
    let be = match &h[..4] {
        b"MM\0*" => true,
        b"II*\0" => false,
        _ => return None, // not classic TIFF (BigTIFF has a different layout)
    };
    let u16_of = |b: &[u8]| {
        if be {
            u16::from_be_bytes([b[0], b[1]])
        } else {
            u16::from_le_bytes([b[0], b[1]])
        }
    };
    let u32_of = |b: &[u8]| {
        let a = [b[0], b[1], b[2], b[3]];
        if be {
            u32::from_be_bytes(a)
        } else {
            u32::from_le_bytes(a)
        }
    };
    f.seek(SeekFrom::Start(u32_of(&h[4..]).into())).ok()?;
    let mut n = [0u8; 2];
    f.read_exact(&mut n).ok()?;
    let mut entries = vec![0u8; usize::from(u16_of(&n)) * 12];
    f.read_exact(&mut entries).ok()?;
    let e = entries.chunks_exact(12).find(|e| u16_of(e) == TAG)?;
    let (count, offset) = (u32_of(&e[4..]), u32_of(&e[8..]));
    if count <= 4 || count > MAX_XML {
        return None;
    }
    f.seek(SeekFrom::Start(offset.into())).ok()?;
    let mut buf = vec![0u8; count as usize];
    f.read_exact(&mut buf).ok()?;
    // SNAP writes ISO-8859-1.
    Some(buf.iter().map(|&b| b as char).collect())
}

fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let rest = &s[s.find(&format!("<{name}>"))? + name.len() + 2..];
    Some(rest[..rest.find(&format!("</{name}>"))?].trim())
}

// ponytail: plain string scan, no XML crate; entities (&amp;) are left as-is.
fn parse(xml: &str) -> Names {
    let mut out = Names::new();
    for blk in xml.split("<Spectral_Band_Info>").skip(1) {
        let blk = blk.split("</Spectral_Band_Info>").next().unwrap_or(blk);
        let (Some(i), Some(name)) = (tag(blk, "BAND_INDEX"), tag(blk, "BAND_NAME")) else {
            continue;
        };
        let Ok(i) = i.parse() else { continue };
        let desc = tag(blk, "BAND_DESCRIPTION").filter(|d| !d.is_empty());
        out.insert(i, (name.into(), desc.map(Into::into)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = "<Dimap_Document><Image_Interpretation>\
        <Spectral_Band_Info><BAND_INDEX>0</BAND_INDEX><BAND_DESCRIPTION>Reflectance in band B1</BAND_DESCRIPTION><BAND_NAME>B1</BAND_NAME></Spectral_Band_Info>\
        <Spectral_Band_Info><BAND_INDEX>1</BAND_INDEX><BAND_DESCRIPTION></BAND_DESCRIPTION><BAND_NAME>B2</BAND_NAME></Spectral_Band_Info>\
        </Image_Interpretation></Dimap_Document>";

    #[test]
    fn parses_names_and_descriptions() {
        let n = parse(XML);
        assert_eq!(n[&0], ("B1".into(), Some("Reflectance in band B1".into())));
        assert_eq!(n[&1], ("B2".into(), None));
        assert_eq!(n.len(), 2);
    }

    /// Minimal classic TIFF whose only IFD entry is tag 65000.
    fn tiff(be: bool, xml: &str) -> Vec<u8> {
        let w16 = |v: u16| if be { v.to_be_bytes() } else { v.to_le_bytes() };
        let w32 = |v: u32| if be { v.to_be_bytes() } else { v.to_le_bytes() };
        let mut b = if be {
            b"MM\0*".to_vec()
        } else {
            b"II*\0".to_vec()
        };
        b.extend(w32(8));
        b.extend(w16(1));
        b.extend(w16(TAG));
        b.extend(w16(2));
        b.extend(w32(xml.len() as u32));
        b.extend(w32(8 + 2 + 12 + 4));
        b.extend(w32(0));
        b.extend(xml.as_bytes());
        b
    }

    #[test]
    fn reads_tag_in_both_byte_orders() {
        let dir = std::env::temp_dir().join(format!("snap-rs-dimap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (be, name) in [(true, "be.tif"), (false, "le.tif")] {
            let p = dir.join(name);
            std::fs::write(&p, tiff(be, XML)).unwrap();
            assert_eq!(read(&p).len(), 2, "{name}");
        }
        std::fs::write(dir.join("no.tif"), b"not a tiff at all").unwrap();
        assert!(read(&dir.join("no.tif")).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
