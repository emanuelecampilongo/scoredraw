use std::{env, fs, path::PathBuf};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resources = Vec::new();
    append_resource(&mut resources, 0, 0, &[]);
    let mut next_image = 1;
    for (path, id) in [
        ("assets/scoredraw.ico", 101),
        ("assets/scoredraw-pdf.ico", 102),
    ] {
        println!("cargo:rerun-if-changed={path}");
        let icon = fs::read(path).expect("read icon");
        append_icon_group(&mut resources, &icon, id, &mut next_image);
    }

    let path = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("scoredraw.res");
    fs::write(&path, resources).expect("write Windows icon resources");
    println!("cargo:rustc-link-arg={}", path.display());
}

fn append_icon_group(resources: &mut Vec<u8>, icon: &[u8], group_id: u16, next_image: &mut u16) {
    // An ICO directory contains 32-bit file offsets. RT_GROUP_ICON uses 16-bit resource IDs.
    let count = u16::from_le_bytes(icon[4..6].try_into().unwrap()) as usize;
    let mut group = icon[..6].to_vec();
    let (entries, _) = icon[6..6 + 16 * count].as_chunks::<16>();
    for entry in entries {
        let length = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        append_resource(resources, 3, *next_image, &icon[offset..offset + length]);
        group.extend_from_slice(&entry[..12]);
        group.extend_from_slice(&next_image.to_le_bytes());
        *next_image += 1;
    }
    append_resource(resources, 14, group_id, &group);
}

fn append_resource(bytes: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    // A .res record has a DWORD-aligned header and payload. These resources use ordinal names.
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&32_u32.to_le_bytes());
    for value in [0xffff, kind, 0xffff, id] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&0_u32.to_le_bytes()); // Data version
    let memory_flags: u16 = if kind == 0 { 0 } else { 0x1030 };
    bytes.extend_from_slice(&memory_flags.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes()); // Language neutral
    bytes.extend_from_slice(&[0; 8]); // Version and characteristics
    bytes.extend_from_slice(data);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
}
