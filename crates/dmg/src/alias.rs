use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;

const MAC_EPOCH_OFFSET: u32 = 2_082_844_800; // secs between 1904-01-01 and 1970-01-01
const RECORD_SIZE: u16 = 158;

pub fn build(vol_name: &str, mount_point: &Path, file_path: &Path) -> Result<Vec<u8>> {
    use std::os::unix::fs::MetadataExt;

    let parent_path = file_path.parent().unwrap_or(mount_point);
    let file_meta = std::fs::metadata(file_path)?;
    let parent_meta = std::fs::metadata(parent_path)?;

    let file_ino = file_meta.ino() as u32;
    let parent_ino = parent_meta.ino() as u32;

    let to_mac = |t: SystemTime| {
        t.duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as u32 + MAC_EPOCH_OFFSET)
            .unwrap_or(MAC_EPOCH_OFFSET)
    };

    let file_crtime = file_meta
        .created()
        .or_else(|_| file_meta.modified())
        .map(to_mac)
        .unwrap_or(0);

    let vol_crtime = std::fs::metadata(mount_point)
        .ok()
        .and_then(|m| m.created().ok())
        .map(to_mac)
        .unwrap_or(0);

    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("bg.tiff");

    let mut buf = vec![0u8; RECORD_SIZE as usize];

    // 0x00: userType (4)
    // 0x04: total size (2)
    put_u16(&mut buf[4..6], RECORD_SIZE);
    // 0x06: version = 2
    put_u16(&mut buf[6..8], 2);
    // 0x08: aliasKind = 0 (file)
    // 0x0A: volumeName pascal string (27 bytes: 1-byte len + up to 26 chars)
    let vn = &vol_name[..vol_name.len().min(26)];
    buf[10] = vn.len() as u8;
    buf[11..11 + vn.len()].copy_from_slice(vn.as_bytes());
    // 0x25: volCreationDate (4)
    put_u32(&mut buf[0x25..0x29], vol_crtime);
    // 0x29: volSig = 0xBD2F (HFS+)
    put_u16(&mut buf[0x29..0x2B], 0xBD2F);
    // 0x2B: volType = 1 (local fixed disk)
    put_u16(&mut buf[0x2B..0x2D], 1);
    // 0x2D: parentDirID (4)
    put_u32(&mut buf[0x2D..0x31], parent_ino);
    // 0x31: fileName pascal string (63 bytes: 1-byte len + up to 62 chars)
    let fn_ = &file_name[..file_name.len().min(62)];
    buf[0x31] = fn_.len() as u8;
    buf[0x32..0x32 + fn_.len()].copy_from_slice(fn_.as_bytes());
    // 0x70: fileNumber (4)
    put_u32(&mut buf[0x70..0x74], file_ino);
    // 0x74: fileCreationDate (4)
    put_u32(&mut buf[0x74..0x78], file_crtime);
    // 0x78: fileType — 'TIFF' if .tiff, else 0
    let ft = if file_name.ends_with(".tiff") || file_name.ends_with(".tif") {
        0x54494646u32
    } else {
        0
    };
    put_u32(&mut buf[0x78..0x7C], ft);
    // 0x7C: fileCreator = 0
    // 0x80: nlvlFrom = 0
    // 0x82: nlvlTo = 0
    // 0x84: volAttributes = 0
    // 0x88: volFSID = 0
    // 0x8A: reserved (6) = 0

    // Extension: type 0x0002 (dirIDs), 4 bytes = parentDirID
    // [u16 type][u16 length][u32 dirID]
    put_u16(&mut buf[0x90..0x92], 0x0002);
    put_u16(&mut buf[0x92..0x94], 4);
    put_u32(&mut buf[0x94..0x98], parent_ino);

    // End of extensions
    put_u16(&mut buf[0x98..0x9A], 0xFFFF);

    // 0x9A-0x9D: zeros (pad to RECORD_SIZE=158)

    Ok(buf)
}

fn put_u16(b: &mut [u8], v: u16) {
    b[0] = (v >> 8) as u8;
    b[1] = v as u8;
}

fn put_u32(b: &mut [u8], v: u32) {
    b[0] = (v >> 24) as u8;
    b[1] = (v >> 16) as u8;
    b[2] = (v >> 8) as u8;
    b[3] = v as u8;
}
