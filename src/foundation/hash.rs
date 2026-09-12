use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha1::{Digest, Sha1};

pub fn sha1_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

pub fn sha1_bytes(bytes: &[u8]) -> String {
    hex(&Sha1::digest(bytes))
}

pub fn md5_file(path: &Path) -> io::Result<String> {
    use md5::Md5;
    let mut file = File::open(path)?;
    let mut hasher = Md5::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_known_value() {
        assert_eq!(
            sha1_bytes(b"hello world"),
            "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed"
        );
    }

    #[test]
    fn hashes_file() {
        let dir = std::env::temp_dir().join("crustcore-hash-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hello.txt");
        std::fs::write(&path, b"hello world").unwrap();
        assert_eq!(
            sha1_file(&path).unwrap(),
            "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed"
        );
    }
}
