use super::{Frame, PageKey};
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;

const SLOTS: u8 = 128;

pub(super) struct ThumbnailCache(PathBuf);

impl ThumbnailCache {
    pub(super) fn new(root: PathBuf) -> Self {
        Self(root)
    }

    fn slot(&self, key: [u8; 32]) -> PathBuf {
        self.0.join(format!("{:02x}.bin", key[0] % SLOTS))
    }

    pub(super) fn read(&self, key: [u8; 32], size: PageKey) -> Option<Frame> {
        let mut file = File::open(self.slot(key)).ok()?;
        let mut stored_key = [0; 32];
        file.read_exact(&mut stored_key).ok()?;
        if stored_key != key {
            return None;
        }
        let mut dimensions = [0; 8];
        file.read_exact(&mut dimensions).ok()?;
        let width = u32::from_le_bytes(dimensions[..4].try_into().ok()?);
        let height = u32::from_le_bytes(dimensions[4..].try_into().ok()?);
        if width == 0 || height == 0 || width > size.width || height > size.height {
            return None;
        }
        let bytes = u64::from(width) * u64::from(height) * 4;
        let mut pixels = Vec::with_capacity(bytes as usize);
        file.take(bytes + 1).read_to_end(&mut pixels).ok()?;
        (pixels.len() as u64 == bytes).then_some(Frame {
            width: width as i32,
            height: height as i32,
            pixels,
        })
    }

    pub(super) fn write(&self, key: [u8; 32], frame: &Frame) {
        // Directly mapped slots put a hard bound on storage across PDFs.
        let _ = crate::data::atomic_write(&self.slot(key), |file| {
            file.write_all(&key)?;
            file.write_all(&(frame.width as u32).to_le_bytes())?;
            file.write_all(&(frame.height as u32).to_le_bytes())?;
            file.write_all(&frame.pixels)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_cache_checks_identity_and_replaces_a_colliding_slot() {
        let scratch = crate::data::Scratch::new();
        let cache = ThumbnailCache::new(scratch.0.join("previews"));
        let size = PageKey { session: 1, index: 3, width: 8, height: 8 };
        let frame = Frame { width: 2, height: 2, pixels: vec![41; 16] };
        let key = [0; 32];
        let mut other = key;
        other[0] = SLOTS;
        cache.write(key, &frame);
        assert_eq!(cache.read(key, size).unwrap().pixels, frame.pixels);
        assert!(cache.read(other, size).is_none());
        cache.write(other, &frame);
        assert!(cache.read(key, size).is_none());
        assert_eq!(cache.read(other, size).unwrap().pixels, frame.pixels);
    }
}
