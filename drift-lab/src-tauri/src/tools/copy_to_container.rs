//! Primitive — drop a local file into a running container.
//!
//! Uses Docker's `PUT /containers/{id}/archive` endpoint via bollard. The
//! body must be a tar stream, so we build a single-entry tarball in memory.
//! For installing pre-built profilers (py-spy static binary, async-profiler
//! tarball) this is much more reliable than `pip install` inside an image
//! that may not have a compiler toolchain.

use std::path::PathBuf;

use anyhow::{Context, Result};
use bollard::container::UploadToContainerOptions;
use serde::{Deserialize, Serialize};

use super::ToolManifest;
use crate::docker;

pub const NAME: &str = "copy_to_container";
pub const DESCRIPTION: &str =
    "Copy a local file into a container at the given destination directory. The file is \
     packaged as a single-entry tarball and uploaded via the Docker archive API.";
pub const PARAMETERS: &str = r#"{
  "type": "object",
  "properties": {
    "container_id": { "type": "string" },
    "src_path": { "type": "string", "description": "Absolute path on the host." },
    "dest_dir": { "type": "string", "description": "Directory inside the container (e.g. /usr/local/bin)." },
    "mode": {
      "type": "integer",
      "description": "Unix mode for the destination file (default 0o755)."
    }
  },
  "required": ["container_id", "src_path", "dest_dir"]
}"#;

#[derive(Debug, Deserialize)]
pub struct Args {
    pub container_id: String,
    pub src_path: String,
    pub dest_dir: String,
    pub mode: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub bytes_copied: u64,
    pub dest_path: String,
}

pub fn manifest() -> ToolManifest {
    ToolManifest {
        name: NAME,
        description: DESCRIPTION,
        parameters: PARAMETERS,
    }
}

pub async fn run(args: Args) -> Result<Output> {
    let docker = docker::connect().context("docker connect")?;

    let src = PathBuf::from(&args.src_path);
    let bytes = std::fs::read(&src).with_context(|| format!("read {}", src.display()))?;
    let bytes_copied = bytes.len() as u64;
    let filename = src
        .file_name()
        .and_then(|s| s.to_str())
        .context("source path has no filename")?
        .to_string();

    let tar = build_single_entry_tar(&filename, &bytes, args.mode.unwrap_or(0o755))?;

    let opts = UploadToContainerOptions {
        path: args.dest_dir.clone(),
        no_overwrite_dir_non_dir: "false".to_string(),
    };

    docker
        .upload_to_container(&args.container_id, Some(opts), tar.into())
        .await
        .context("upload_to_container")?;

    let dest_path = format!("{}/{filename}", args.dest_dir.trim_end_matches('/'));
    Ok(Output {
        bytes_copied,
        dest_path,
    })
}

/// Hand-rolled minimal POSIX tar (ustar) entry. We avoid pulling in the
/// `tar` crate just for this — the format is well-documented and stable.
fn build_single_entry_tar(name: &str, contents: &[u8], mode: u32) -> Result<Vec<u8>> {
    if name.len() > 100 {
        anyhow::bail!("filename too long for ustar: {name}");
    }
    let mut header = [0u8; 512];

    // name (100), mode (8), uid (8), gid (8), size (12), mtime (12), chksum (8),
    // typeflag (1), linkname (100), magic (6), version (2), uname (32), gname (32),
    // devmajor (8), devminor (8), prefix (155), pad (12)
    header[..name.len()].copy_from_slice(name.as_bytes());
    write_octal(&mut header[100..108], mode as u64, 7);
    write_octal(&mut header[108..116], 0, 7); // uid
    write_octal(&mut header[116..124], 0, 7); // gid
    write_octal(&mut header[124..136], contents.len() as u64, 11);
    write_octal(&mut header[136..148], 0, 11); // mtime (epoch — Docker doesn't care)
    // chksum field starts as spaces during checksum computation
    for b in &mut header[148..156] {
        *b = b' ';
    }
    header[156] = b'0'; // typeflag = regular file
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");

    let chksum: u32 = header.iter().map(|&b| b as u32).sum();
    write_octal(&mut header[148..155], chksum as u64, 6);
    header[155] = 0;

    let mut out = Vec::with_capacity(512 + contents.len().div_ceil(512) * 512 + 1024);
    out.extend_from_slice(&header);
    out.extend_from_slice(contents);
    let pad = (512 - contents.len() % 512) % 512;
    out.extend(std::iter::repeat(0u8).take(pad));
    // Two trailing zero blocks marking end of archive.
    out.extend(std::iter::repeat(0u8).take(1024));
    Ok(out)
}

fn write_octal(field: &mut [u8], value: u64, digits: usize) {
    let s = format!("{:0>width$o}", value, width = digits);
    let bytes = s.as_bytes();
    field[..bytes.len()].copy_from_slice(bytes);
    if bytes.len() < field.len() {
        field[bytes.len()] = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_octal_pads_with_leading_zeros() {
        let mut buf = [0u8; 8];
        write_octal(&mut buf, 0o755, 7);
        assert_eq!(&buf[..7], b"0000755");
        assert_eq!(buf[7], 0); // null terminator
    }

    #[test]
    fn write_octal_with_size() {
        let mut buf = [0u8; 12];
        write_octal(&mut buf, 1024, 11);
        assert_eq!(&buf[..11], b"00000002000");
        assert_eq!(buf[11], 0);
    }

    #[test]
    fn tar_has_512_byte_header_and_data() {
        let tar = build_single_entry_tar("hello.txt", b"hello", 0o644).unwrap();
        // 512 header + 512 padded data block + 1024 trailer = 2048
        assert_eq!(tar.len(), 2048);
        // ustar magic at offset 257..263
        assert_eq!(&tar[257..263], b"ustar\0");
        // Filename at offset 0
        assert_eq!(&tar[..9], b"hello.txt");
        // Data starts at byte 512
        assert_eq!(&tar[512..517], b"hello");
        // Padding is zero
        assert!(tar[517..1024].iter().all(|&b| b == 0));
    }

    #[test]
    fn tar_size_field_matches_payload() {
        let tar = build_single_entry_tar("a.bin", b"ABCDE", 0o755).unwrap();
        // Size field at 124..136, octal of 5 = "00000000005"
        assert_eq!(&tar[124..135], b"00000000005");
    }

    #[test]
    fn tar_typeflag_is_regular_file() {
        let tar = build_single_entry_tar("a.bin", b"x", 0o755).unwrap();
        assert_eq!(tar[156], b'0');
    }

    #[test]
    fn tar_checksum_is_correct() {
        let tar = build_single_entry_tar("a", b"x", 0o644).unwrap();
        // Recompute the checksum the same way the writer does, using a header
        // copy with the chksum field reset to spaces.
        let mut hdr = [0u8; 512];
        hdr.copy_from_slice(&tar[..512]);
        for b in &mut hdr[148..156] {
            *b = b' ';
        }
        let expected: u32 = hdr.iter().map(|&b| b as u32).sum();
        // Parse stored checksum (octal, 6 digits + null + space).
        let stored = std::str::from_utf8(&tar[148..154]).unwrap().trim_end_matches('\0');
        let stored = u32::from_str_radix(stored, 8).unwrap();
        assert_eq!(stored, expected);
    }

    #[test]
    fn tar_pads_data_to_512_byte_boundary() {
        // 600-byte payload → padded to 1024 → total 512 + 1024 + 1024 = 2560
        let payload = vec![0xAB; 600];
        let tar = build_single_entry_tar("big.bin", &payload, 0o644).unwrap();
        assert_eq!(tar.len(), 2560);
    }

    #[test]
    fn tar_rejects_filenames_over_100_bytes() {
        let long = "a".repeat(101);
        let err = build_single_entry_tar(&long, b"x", 0o644).unwrap_err();
        assert!(err.to_string().contains("filename too long"));
    }

    #[test]
    fn tar_trailer_is_two_zero_blocks() {
        let tar = build_single_entry_tar("a.bin", b"x", 0o644).unwrap();
        // Last 1024 bytes must be all-zero (the EOF marker).
        let trailer = &tar[tar.len() - 1024..];
        assert!(trailer.iter().all(|&b| b == 0));
    }
}
