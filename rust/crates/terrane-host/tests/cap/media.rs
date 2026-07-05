use std::fs;

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn media_cli_probes_and_transforms_tiny_png_through_blob_cas() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let input = home.join("in.png");
    let output = home.join("thumb.jpg");
    fs::write(&input, tiny_png()).unwrap();

    let (ok, _, err) = terrane(home, &["app", "add", "gallery", "Gallery"]);
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &[
            "blob",
            "put",
            "gallery",
            "photo.png",
            "image/png",
            input.to_str().unwrap(),
        ],
    );
    assert!(ok, "blob put failed: {err}");

    let (ok, out, err) = terrane(home, &["media", "info", "gallery", "photo.png"]);
    assert!(ok, "media info failed: {err}");
    assert!(out.contains("\"kind\":\"image\""), "info out: {out}");
    assert!(out.contains("\"width\":2"), "info out: {out}");

    let (ok, out, err) = terrane(
        home,
        &[
            "media",
            "transform",
            "gallery",
            "photo.png",
            r#"[{"op":"thumbnail","size":1}]"#,
            "__thumb__/photo.jpg",
        ],
    );
    assert!(ok, "media transform failed: {err}");
    assert!(out.contains("media.transformed"), "transform out: {out}");
    assert!(out.contains("blob.stored"), "transform out: {out}");

    let (ok, out, err) = terrane(home, &["blob", "verify", "gallery", "__thumb__/photo.jpg"]);
    assert!(ok, "blob verify failed: {err}");
    assert!(out.contains("ok "), "verify out: {out}");
    let (ok, _, err) = terrane(
        home,
        &[
            "blob",
            "get",
            "gallery",
            "__thumb__/photo.jpg",
            output.to_str().unwrap(),
        ],
    );
    assert!(ok, "blob get failed: {err}");
    assert!(fs::read(output).unwrap().starts_with(&[0xff, 0xd8]));
}

#[test]
fn media_transform_rejects_png_dimensions_over_pixel_budget() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let input = home.join("bomb.png");
    fs::write(&input, png_with_dimensions(9000, 8000)).unwrap();
    terrane(home, &["app", "add", "gallery", "Gallery"]);
    let (ok, _, err) = terrane(
        home,
        &[
            "blob",
            "put",
            "gallery",
            "bomb.png",
            "image/png",
            input.to_str().unwrap(),
        ],
    );
    assert!(ok, "blob put failed: {err}");

    let (ok, out, err) = terrane(
        home,
        &[
            "media",
            "transform",
            "gallery",
            "bomb.png",
            r#"[{"op":"thumbnail","size":1}]"#,
            "__thumb__/bomb.jpg",
        ],
    );
    assert!(!ok, "pixel bomb should fail: {out}");
    assert!(
        err.contains("decoded image exceeds") || out.contains("decoded image exceeds"),
        "out: {out}, err: {err}"
    );
}

fn tiny_png() -> Vec<u8> {
    let image = image::RgbImage::from_fn(2, 2, |x, y| {
        if (x + y) % 2 == 0 {
            image::Rgb([255, 0, 0])
        } else {
            image::Rgb([0, 128, 255])
        }
    });
    let mut out = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

fn png_with_dimensions(width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    push_chunk(&mut out, b"IHDR", &ihdr);
    push_chunk(&mut out, b"IDAT", &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
    push_chunk(&mut out, b"IEND", &[]);
    out
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(kind.len() + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}
