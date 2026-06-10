use std::io::{Cursor, Seek, SeekFrom, Write};

use anyhow::{Result, ensure};
use binrw::{BinResult, BinWrite, BinWriterExt, Endian};
use plist::{Dictionary, Value};

pub struct DsStoreSpec<'a> {
    pub window_width: u32,
    pub window_height: u32,
    pub icon_size: u32,
    pub app_name: &'a str,
    pub app_x: u32,
    pub app_y: u32,
    pub applications_x: u32,
    pub applications_y: u32,
    /// Raw macOS Alias record bytes for the background image, if any.
    pub background_alias: Option<Vec<u8>>,
}

// ─── Public entry point ──────────────────────────────────────────────────────

pub fn build(spec: &DsStoreSpec<'_>) -> Result<Vec<u8>> {
    let records = make_records(spec)?;
    assemble(&records)
}

// ─── Record model ────────────────────────────────────────────────────────────

/// One B-tree leaf record: keyed on `(filename, structure_id)`, carrying a
/// typed payload. This is the only part of a `.DS_Store` that varies between
/// volumes; everything else is a fixed buddy-allocator scaffold (see
/// `assemble`).
struct Record {
    filename: String,
    structure_id: [u8; 4],
    data_type: [u8; 4],
    payload: Payload,
}

enum Payload {
    Long(u32),
    Blob(Vec<u8>),
}

impl BinWrite for Record {
    type Args<'a> = ();

    fn write_options<W: Write + Seek>(
        &self,
        w: &mut W,
        endian: Endian,
        _: Self::Args<'_>,
    ) -> BinResult<()> {
        // [u32 length in UTF-16 code units][filename UTF-16BE]
        let utf16: Vec<u16> = self.filename.encode_utf16().collect();
        (utf16.len() as u32).write_options(w, endian, ())?;
        utf16.write_options(w, endian, ())?;

        self.structure_id.write_options(w, endian, ())?;
        self.data_type.write_options(w, endian, ())?;

        match &self.payload {
            Payload::Long(v) => v.write_options(w, endian, ())?,
            Payload::Blob(bytes) => {
                // "blob" payloads are framed with a big-endian length prefix.
                (bytes.len() as u32).write_options(w, endian, ())?;
                bytes.write_options(w, endian, ())?;
            },
        }
        Ok(())
    }
}

fn make_records(spec: &DsStoreSpec<'_>) -> Result<Vec<Record>> {
    let bwsp_bytes = build_bwsp_plist(spec.window_width, spec.window_height)?;
    let icvp_bytes = build_icvp_plist(spec)?;
    let app_filename = format!("{}.app", spec.app_name);

    let mut records = vec![
        // Window-level records keyed on "." (volume root)
        Record {
            filename: ".".into(),
            structure_id: *b"bwsp",
            data_type: *b"blob",
            payload: Payload::Blob(bwsp_bytes),
        },
        Record {
            filename: ".".into(),
            structure_id: *b"icvp",
            data_type: *b"blob",
            payload: Payload::Blob(icvp_bytes),
        },
        Record {
            filename: ".".into(),
            structure_id: *b"vSrn",
            data_type: *b"long",
            payload: Payload::Long(1),
        },
        // Icon positions
        Record {
            filename: app_filename,
            structure_id: *b"Iloc",
            data_type: *b"blob",
            payload: Payload::Blob(iloc_bytes(spec.app_x, spec.app_y)),
        },
        Record {
            filename: "Applications".into(),
            structure_id: *b"Iloc",
            data_type: *b"blob",
            payload: Payload::Blob(iloc_bytes(spec.applications_x, spec.applications_y)),
        },
    ];

    // Sort: TN1150 filename collation, then structureId lexicographic.
    // For typical ASCII filenames a case-folded byte comparison is sufficient.
    records.sort_by(|a, b| {
        let fa = tn1150_key(&a.filename);
        let fb = tn1150_key(&b.filename);
        fa.cmp(&fb)
            .then_with(|| a.structure_id.cmp(&b.structure_id))
    });

    Ok(records)
}

/// The 16-byte `Iloc` blob payload: icon centre `(x, y)` followed by the
/// reserved `FF FF FF FF FF FF 00 00` trailer. The 4-byte length prefix is
/// added by [`Record`]'s blob serializer.
fn iloc_bytes(x: u32, y: u32) -> Vec<u8> {
    let mut b = Vec::with_capacity(16);
    b.extend_from_slice(&x.to_be_bytes());
    b.extend_from_slice(&y.to_be_bytes());
    b.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
    b.extend_from_slice(&0xFFFF_0000u32.to_be_bytes());
    b
}

/// Rough TN1150 sort key: lower-case bytes with period sorted before letters.
fn tn1150_key(s: &str) -> Vec<u8> {
    s.chars().map(|c| c.to_ascii_lowercase() as u8).collect()
}

// ─── plist builders ──────────────────────────────────────────────────────────

fn build_bwsp_plist(width: u32, height: u32) -> Result<Vec<u8>> {
    // WindowBounds: outer bounds including title bar (+22 px)
    let bounds = format!("{{{{100, 100}}, {{{width}, {}}}}}", height + 22);

    let mut d = Dictionary::new();
    d.insert("ContainerShowSidebar".into(), Value::Boolean(false));
    d.insert("ShowPathbar".into(), Value::Boolean(false));
    d.insert("ShowSidebar".into(), Value::Boolean(false));
    d.insert("ShowStatusBar".into(), Value::Boolean(false));
    d.insert("ShowTabView".into(), Value::Boolean(false));
    d.insert("ShowToolbar".into(), Value::Boolean(false));
    d.insert("SidebarWidth".into(), Value::Integer(0.into()));
    d.insert("WindowBounds".into(), Value::String(bounds));

    plist_to_bytes(Value::Dictionary(d))
}

fn build_icvp_plist(spec: &DsStoreSpec<'_>) -> Result<Vec<u8>> {
    let mut d = Dictionary::new();
    d.insert("viewOptionsVersion".into(), Value::Integer(1.into()));
    d.insert("iconSize".into(), Value::Real(spec.icon_size as f64));
    d.insert("textSize".into(), Value::Real(12.0));
    d.insert("gridSpacing".into(), Value::Real(100.0));
    d.insert("gridOffsetX".into(), Value::Real(0.0));
    d.insert("gridOffsetY".into(), Value::Real(0.0));
    d.insert("labelOnBottom".into(), Value::Boolean(true));
    d.insert("showIconPreview".into(), Value::Boolean(true));
    d.insert("showItemInfo".into(), Value::Boolean(false));
    d.insert("arrangeBy".into(), Value::String("none".into()));

    if let Some(alias_bytes) = &spec.background_alias {
        d.insert("backgroundType".into(), Value::Integer(2.into()));
        d.insert(
            "backgroundImageAlias".into(),
            Value::Data(alias_bytes.clone()),
        );
    } else {
        d.insert("backgroundType".into(), Value::Integer(1.into()));
        d.insert("backgroundColorRed".into(), Value::Real(1.0));
        d.insert("backgroundColorGreen".into(), Value::Real(1.0));
        d.insert("backgroundColorBlue".into(), Value::Real(1.0));
    }

    plist_to_bytes(Value::Dictionary(d))
}

fn plist_to_bytes(v: Value) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    v.to_writer_binary(&mut out)?;
    Ok(out)
}

// ─── Binary file assembly ────────────────────────────────────────────────────

// The `.DS_Store` format is a buddy-allocated 2 GiB address space holding a
// B-tree. Reproducing a full allocator is unnecessary: Finder only reads the
// `DSDB` master block → B-tree, and the allocation never changes as long as the
// three fixed blocks below stay put. So we lay out a byte-for-byte copy of the
// canonical empty store (as shipped by appdmg / written by Finder) and stamp
// our records into the single leaf page.
//
// Block addresses pack `(offset & ~0x1F) | log2(size)` in their low 5 bits, and
// a block at address `a` lives at file offset `(a & ~0x1F) + 4` (the leading
// 4-byte magic is outside the allocator's address space). The three blocks:
//
//   block 0  addr 0x200B → file 0x2004, 2048 B  buddy bookkeeping block
//   block 1  addr 0x0045 → file 0x0044,   32 B  DSDB master block
//   block 2  addr 0x100C → file 0x1004, 4096 B  B-tree leaf page (our records)

/// Total file length, matching the canonical empty store.
const FILE_LEN: usize = 15364;
/// One past the last byte usable by the 4 KiB leaf page (file 0x1004 + 0x1000).
const LEAF_BLOCK_END: u64 = 0x2004;

fn assemble(records: &[Record]) -> Result<Vec<u8>> {
    let count = records.len() as u32;
    let mut cur = Cursor::new(vec![0u8; FILE_LEN]);

    // ── File header + buddy-allocator header ─────────────────────────────────
    put(&mut cur, 0x00, &[0x0000_0001])?; // magic prefix
    cur.seek(SeekFrom::Start(0x04))?;
    cur.write_all(b"Bud1")?;
    put(
        &mut cur,
        0x08,
        &[
            0x0000_2000, // bookkeeping block offset
            0x0000_0800, // bookkeeping block size
            0x0000_2000, // bookkeeping block offset (redundant copy)
            0x0000_100C, // root B-tree block address
        ],
    )?;
    put(&mut cur, 0x2C, &[0x0000_0800, 0x0000_0800])?; // allocator-size copies

    // ── DSDB master block @ file 0x44 ────────────────────────────────────────
    put(
        &mut cur,
        0x44,
        &[
            2,           // root node block number (block 2 = leaf)
            0,           // tree height (0 = single leaf, no interior nodes)
            count,       // total record count
            1,           // total node count
            0x0000_1000, // page size
        ],
    )?;

    // ── B-tree leaf page @ file 0x1004 ───────────────────────────────────────
    cur.seek(SeekFrom::Start(0x1004))?;
    cur.write_be(&0u32)?; // P: child block number (0 in a leaf)
    cur.write_be(&count)?; // record count in this node
    for r in records {
        cur.write_be(r)?;
    }
    let leaf_used = cur.position() - 0x1004;
    ensure!(
        cur.position() <= LEAF_BLOCK_END,
        "DS_Store records ({leaf_used} bytes) overflow the 4 KiB B-tree leaf page"
    );

    // ── Buddy-allocator bookkeeping block @ file 0x2004 ──────────────────────
    write_bookkeeping(&mut cur)?;

    Ok(cur.into_inner())
}

/// Writes the buddy-allocator bookkeeping block: block-address table, the lone
/// `DSDB` table-of-contents entry, and the 32-bucket free list. None of this
/// depends on the records, so it is identical in every store we emit.
fn write_bookkeeping<W: Write + Seek>(w: &mut W) -> BinResult<()> {
    w.seek(SeekFrom::Start(0x2004))?;
    w.write_be(&3u32)?; // number of allocated blocks
    w.write_be(&0u32)?; // unused

    // Block-address table, padded to 256 entries; only the three blocks used.
    let mut table = [0u32; 256];
    table[0] = 0x0000_200B; // bookkeeping block: offset 0x2000, size 2^11
    table[1] = 0x0000_0045; // DSDB master block: offset 0x0040, size 2^5
    table[2] = 0x0000_100C; // B-tree leaf page:  offset 0x1000, size 2^12
    for entry in table {
        w.write_be(&entry)?;
    }

    // Table of contents: a single "DSDB" → block 1 entry.
    w.write_be(&1u32)?; // TOC entry count
    w.write_all(&[4])?; // name length
    w.write_all(b"DSDB")?;
    w.write_be(&1u32)?; // block number

    // Free list: 32 power-of-two buckets describing the otherwise-empty 2 GiB
    // address space. Verbatim from the canonical empty store.
    w.write_all(&FREE_LIST)?;
    Ok(())
}

/// Writes a run of big-endian `u32`s starting at absolute file offset `off`.
fn put<W: Write + Seek>(w: &mut W, off: u64, vals: &[u32]) -> BinResult<()> {
    w.seek(SeekFrom::Start(off))?;
    for v in vals {
        w.write_be(v)?;
    }
    Ok(())
}

/// The 32-bucket buddy free list of an empty `.DS_Store`, lifted byte-for-byte
/// from the canonical template. Each bucket is `[u32 count][u32 offset; count]`.
#[rustfmt::skip]
const FREE_LIST: [u8; 231] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02,
    0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x60, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x02, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x02,
    0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x28, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x80, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x04, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x20, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x08, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x01, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x40, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00,
];

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> DsStoreSpec<'static> {
        DsStoreSpec {
            window_width: 600,
            window_height: 400,
            icon_size: 80,
            app_name: "MyApp",
            app_x: 150,
            app_y: 200,
            applications_x: 450,
            applications_y: 200,
            background_alias: None,
        }
    }

    #[test]
    fn header_and_block_table_match_canonical_store() {
        let bytes = build(&spec()).unwrap();
        assert_eq!(bytes.len(), FILE_LEN);

        // Magic + Bud1 signature.
        assert_eq!(&bytes[0..4], &[0x00, 0x00, 0x00, 0x01]);
        assert_eq!(&bytes[4..8], b"Bud1");

        // Bookkeeping block: count = 3, then the three known block addresses.
        assert_eq!(read_u32(&bytes, 0x2004), 3);
        assert_eq!(read_u32(&bytes, 0x200C), 0x0000_200B);
        assert_eq!(read_u32(&bytes, 0x2010), 0x0000_0045);
        assert_eq!(read_u32(&bytes, 0x2014), 0x0000_100C);

        // DSDB → block 1 in the table of contents (after the 256-entry table).
        assert_eq!(read_u32(&bytes, 0x240C), 1); // TOC count
        assert_eq!(bytes[0x2410], 4);
        assert_eq!(&bytes[0x2411..0x2415], b"DSDB");
        assert_eq!(read_u32(&bytes, 0x2415), 1);
    }

    #[test]
    fn record_count_is_consistent_across_dsdb_and_leaf() {
        let bytes = build(&spec()).unwrap();
        let n = make_records(&spec()).unwrap().len() as u32;
        assert_eq!(read_u32(&bytes, 0x4C), n); // DSDB total record count
        assert_eq!(read_u32(&bytes, 0x1004), 0); // leaf P = 0
        assert_eq!(read_u32(&bytes, 0x1008), n); // leaf record count
    }

    #[test]
    fn records_are_sorted_and_parseable() {
        let bytes = build(&spec()).unwrap();
        let n = read_u32(&bytes, 0x1008) as usize;

        let mut pos = 0x100C;
        let mut names = Vec::new();
        for _ in 0..n {
            let name_len = read_u32(&bytes, pos) as usize;
            pos += 4;
            let name: String = (0..name_len)
                .map(|i| {
                    let cu = u16::from_be_bytes([bytes[pos + i * 2], bytes[pos + i * 2 + 1]]);
                    char::from_u32(cu as u32).unwrap()
                })
                .collect();
            pos += name_len * 2;
            let structure_id = &bytes[pos..pos + 4];
            let data_type = &bytes[pos + 4..pos + 8];
            pos += 8;
            match data_type {
                b"long" => pos += 4,
                b"blob" => pos += 4 + read_u32(&bytes, pos) as usize,
                other => panic!("unexpected data type {other:?}"),
            }
            let _ = structure_id;
            names.push(name);
        }

        // Five records, and the leaf stayed within its page.
        assert_eq!(names.len(), 5);
        assert!(pos as u64 <= LEAF_BLOCK_END);

        // Collation: "." before "Applications" before "MyApp.app".
        let mut sorted = names.clone();
        sorted.sort_by_key(|s| tn1150_key(s));
        assert_eq!(names, sorted);
    }

    fn read_u32(b: &[u8], off: usize) -> u32 {
        u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }
}
